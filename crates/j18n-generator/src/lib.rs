pub mod options;

pub use options::J18nOptions;

use futures::stream::{self, StreamExt};
use j18n_core::{GenerationMode, I18nData, I18nDefinition, J18nError, J18nResult};
use j18n_io::{
	content_hash_hex, detect_indentation, read_i18n_data, write_i18n_tree_map, I18nHashing, I18nHashingStore,
	DEFAULT_INDENT,
};
use j18n_translator::{create_extrapolated_values, restore_extrapolated_values, I18nTranslator};
use j18n_validator::TranslationValidator;
use serde_json::{Map, Value};
use std::collections::BTreeSet;
use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::Arc;
use tracing::{debug, info, warn};

pub fn target_identifier(definition: &I18nDefinition) -> String {
	format!("{}@{}", definition.id, definition.language)
}

#[derive(Clone, Debug)]
pub struct TargetCheckResult {
	pub target: I18nDefinition,
	pub missing_or_changed_keys: Vec<String>,
	pub stale_keys: Vec<String>,
}

impl TargetCheckResult {
	pub fn needs_sync(&self) -> bool {
		!self.missing_or_changed_keys.is_empty() || !self.stale_keys.is_empty()
	}
}

#[derive(Clone, Debug)]
pub struct CheckReport {
	pub reference_entries: usize,
	pub targets: Vec<TargetCheckResult>,
}

impl CheckReport {
	pub fn needs_sync(&self) -> bool {
		self.targets.iter().any(|target| target.needs_sync())
	}
}

struct SyncPlan {
	entries_to_translate: Vec<(String, String)>,
	stale_keys: Vec<String>,
}

fn compute_sync_plan(
	reference_data: &I18nData,
	target_data: &I18nData,
	changed_keys_since_last_hashing: &BTreeSet<String>,
	mode: GenerationMode,
) -> SyncPlan {
	let reference_entries = &reference_data.walked_tree_map;
	let entries_to_translate: Vec<(String, String)> = match mode {
		GenerationMode::Regenerate => reference_entries.clone(),
		GenerationMode::Sync => {
			let target_keys: BTreeSet<&str> = target_data.walked_tree_map.iter().map(|(k, _)| k.as_str()).collect();

			reference_entries
				.iter()
				.filter(|(key, _)| !target_keys.contains(key.as_str()) || changed_keys_since_last_hashing.contains(key))
				.cloned()
				.collect()
		}
	};
	let stale_keys: Vec<String> = match mode {
		GenerationMode::Regenerate => Vec::new(),
		GenerationMode::Sync => {
			let reference_keys: BTreeSet<&str> = reference_entries.iter().map(|(k, _)| k.as_str()).collect();

			target_data
				.walked_tree_map
				.iter()
				.filter(|(key, _)| !reference_keys.contains(key.as_str()))
				.map(|(key, _)| key.clone())
				.collect()
		}
	};

	SyncPlan {
		entries_to_translate,
		stale_keys,
	}
}

pub struct I18nGenerator;

impl I18nGenerator {
	pub async fn execute<T>(
		translator: &T,
		reference_i18n: &I18nDefinition,
		generate_i18n_for: &[I18nDefinition],
		mode: GenerationMode,
		options: &J18nOptions,
	) -> J18nResult<()>
	where
		T: I18nTranslator + ?Sized,
	{
		let reference_data = read_i18n_data(reference_i18n, &options.exclude_patterns).await?;
		let reference_hashing = I18nHashing::from_i18n_data(&reference_data);
		let store = I18nHashingStore::at(&options.hash_cache_location);

		debug!(
			"Scanned {} total entries from {} dict",
			reference_data.walked_tree_map.len(),
			reference_i18n.language
		);

		// Targets (languages) are processed concurrently. The hash cache is a
		// single shared file rewritten in place, so saves are serialized behind
		// a lock while the slow translation work runs fully in parallel.
		let save_lock = tokio::sync::Mutex::new(());
		let store = &store;
		let reference_data = &reference_data;
		let reference_hashing = &reference_hashing;
		let save_lock = &save_lock;

		let per_target: Vec<J18nResult<usize>> = stream::iter(generate_i18n_for.iter().map(|target| async move {
			let target_id = target_identifier(target);
			let cached_hashing = store.load(&target_id).await?;
			let changed_keys = cached_hashing.compute_changed_keys(reference_hashing);

			debug!(
				"{} keys changed since {} was last synced",
				changed_keys.len(),
				target.language
			);

			let translated = translate_into_target(
				translator,
				reference_i18n,
				reference_data,
				&changed_keys,
				target,
				mode,
				options,
			)
			.await?;

			{
				let _guard = save_lock.lock().await;

				store.save(&target_id, reference_hashing).await?;
			}

			Ok(translated)
		}))
		.buffer_unordered(generate_i18n_for.len().max(1))
		.collect()
		.await;

		let mut total_translated: usize = 0;

		for result in per_target {
			total_translated += result?;
		}

		info!(
			"{mode} complete: {} target(s), {total_translated} entries translated",
			generate_i18n_for.len()
		);

		Ok(())
	}

	/// Records the current reference hashes for each target into the hash
	/// cache, treating existing target files as authoritatively in-sync. Only
	/// reference keys that **also exist in the target file** are hashed —
	/// keys missing from the target are left out so a follow-up `sync`
	/// detects and translates them. The cache is loaded first so unrelated
	/// entries (other configs sharing the same cache file, or other
	/// namespaces' targets in the same file) are preserved; entries for the
	/// targets passed in are replaced.
	pub async fn baseline(
		reference_i18n: &I18nDefinition,
		generate_i18n_for: &[I18nDefinition],
		options: &J18nOptions,
	) -> J18nResult<usize> {
		let reference_data = read_i18n_data(reference_i18n, &options.exclude_patterns).await?;
		let store = I18nHashingStore::at(&options.hash_cache_location);

		debug!(
			"Baselining {} target(s) against {} reference entries from {}",
			generate_i18n_for.len(),
			reference_data.walked_tree_map.len(),
			reference_i18n.language,
		);

		for target in generate_i18n_for {
			let target_data = read_i18n_data(target, &options.exclude_patterns).await?;
			let target_keys: BTreeSet<&str> = target_data
				.walked_tree_map
				.iter()
				.map(|(key, _)| key.as_str())
				.collect();
			let mut hashing_for_target = I18nHashing::empty();

			for (key, value) in &reference_data.walked_tree_map {
				if target_keys.contains(key.as_str()) {
					hashing_for_target
						.json_key_to_hash_map
						.insert(key.clone(), content_hash_hex(value));
				}
			}

			debug!(
				"Baselined {} ({}/{} reference keys present in target)",
				target.language,
				hashing_for_target.json_key_to_hash_map.len(),
				reference_data.walked_tree_map.len(),
			);
			store.save(&target_identifier(target), &hashing_for_target).await?;
		}

		Ok(generate_i18n_for.len())
	}

	pub async fn check(
		reference_i18n: &I18nDefinition,
		generate_i18n_for: &[I18nDefinition],
		options: &J18nOptions,
	) -> J18nResult<CheckReport> {
		let reference_data = read_i18n_data(reference_i18n, &options.exclude_patterns).await?;
		let reference_hashing = I18nHashing::from_i18n_data(&reference_data);
		let store = I18nHashingStore::at(&options.hash_cache_location);
		let reference_entries = reference_data.walked_tree_map.len();

		debug!(
			"Scanned {} total entries from {} dict",
			reference_entries, reference_i18n.language
		);

		// Targets (languages) are checked concurrently. Every per-target step is
		// read-only (cache load + target file read), so no locking is needed;
		// `buffered` preserves the input ordering of the results.
		let store = &store;
		let reference_data = &reference_data;
		let reference_hashing = &reference_hashing;

		let target_results: Vec<J18nResult<TargetCheckResult>> =
			stream::iter(generate_i18n_for.iter().map(|target| async move {
				let target_id = target_identifier(target);
				let cached_hashing = store.load(&target_id).await?;
				let changed_keys = cached_hashing.compute_changed_keys(reference_hashing);
				let target_data = read_i18n_data(target, &options.exclude_patterns).await?;
				let plan = compute_sync_plan(reference_data, &target_data, &changed_keys, GenerationMode::Sync);

				debug!(
					"{}: {} key(s) need translation, {} stale key(s)",
					target.language,
					plan.entries_to_translate.len(),
					plan.stale_keys.len()
				);

				Ok(TargetCheckResult {
					target: target.clone(),
					missing_or_changed_keys: plan.entries_to_translate.into_iter().map(|(key, _)| key).collect(),
					stale_keys: plan.stale_keys,
				})
			}))
			.buffered(generate_i18n_for.len().max(1))
			.collect()
			.await;

		let mut targets = Vec::with_capacity(generate_i18n_for.len());

		for result in target_results {
			targets.push(result?);
		}

		Ok(CheckReport {
			reference_entries,
			targets,
		})
	}
}

#[allow(clippy::too_many_arguments)]
async fn translate_into_target<T>(
	translator: &T,
	reference_i18n: &I18nDefinition,
	reference_data: &j18n_core::I18nData,
	changed_keys_since_last_hashing: &BTreeSet<String>,
	target: &I18nDefinition,
	mode: GenerationMode,
	options: &J18nOptions,
) -> J18nResult<usize>
where
	T: I18nTranslator + ?Sized,
{
	let target_data = read_i18n_data(target, &options.exclude_patterns).await?;
	let plan = compute_sync_plan(reference_data, &target_data, changed_keys_since_last_hashing, mode);
	let entries_to_translate = plan.entries_to_translate;
	let entry_count = entries_to_translate.len();
	let total_characters: usize = entries_to_translate.iter().map(|(_, value)| value.len()).sum();
	let windowed_entries: Vec<Vec<(String, String)>> = entries_to_translate
		.chunks(options.batch_size)
		.map(|chunk| chunk.to_vec())
		.collect();

	if entry_count > 0 {
		info!(
			"Translating {entry_count} entries ({total_characters} characters) to {} in a total of {} batches...",
			target.language,
			windowed_entries.len()
		);
	}

	let total_batches = windowed_entries.len();
	let translated_count = Arc::new(AtomicUsize::new(0));
	let translated_batches: Vec<J18nResult<Vec<(String, String)>>> =
		stream::iter(windowed_entries.into_iter().map(|window| {
			let translated_count = Arc::clone(&translated_count);

			async move {
				let result =
					translate_batch_with_retries(translator, reference_i18n, target, window, options, total_batches)
						.await;
				let current = translated_count.fetch_add(1, Ordering::SeqCst) + 1;

				info!("Batch {current}/{total_batches} translated");

				result
			}
		}))
		.buffer_unordered(options.parallel_batches)
		.collect()
		.await;

	let translated_batches: Vec<Vec<(String, String)>> =
		translated_batches.into_iter().collect::<J18nResult<Vec<_>>>()?;

	debug!("Writing ({mode}) JSON to \"{}\"...", target.file.display());

	let initial_json_dict: Map<String, Value> = match mode {
		GenerationMode::Regenerate => reference_data.json_dict.clone(),
		GenerationMode::Sync => merge_json_objects(&reference_data.json_dict, &target_data.json_dict),
	};
	let indent = resolve_indent(target, reference_i18n, mode).await?;

	write_i18n_tree_map(
		target,
		indent.as_bytes(),
		&reference_data.json_dict,
		initial_json_dict,
		&translated_batches,
	)
	.await?;

	Ok(entry_count)
}

async fn translate_batch_with_retries<T>(
	translator: &T,
	from: &I18nDefinition,
	to: &I18nDefinition,
	batch: Vec<(String, String)>,
	options: &J18nOptions,
	total_batches: usize,
) -> J18nResult<Vec<(String, String)>>
where
	T: I18nTranslator + ?Sized,
{
	let max_attempts = options.retries_per_error.saturating_add(1);
	let mut last_error: Option<J18nError> = None;

	for attempt in 1..=max_attempts {
		match translate_batch(translator, from, to, batch.clone(), options).await {
			Ok(translated) => return Ok(translated),
			Err(error) => {
				if attempt < max_attempts {
					warn!(
						"Batch translation attempt {attempt}/{max_attempts} failed (out of {total_batches} total batches): {error}; retrying..."
					);
				}

				last_error = Some(error);
			}
		}
	}

	Err(last_error.expect("retry loop must run at least once"))
}

async fn translate_batch<T>(
	translator: &T,
	from: &I18nDefinition,
	to: &I18nDefinition,
	batch: Vec<(String, String)>,
	options: &J18nOptions,
) -> J18nResult<Vec<(String, String)>>
where
	T: I18nTranslator + ?Sized,
{
	let mut batch_keys: Vec<String> = Vec::with_capacity(batch.len());
	let mut batch_values: Vec<String> = Vec::with_capacity(batch.len());

	for (key, value) in batch {
		batch_keys.push(key);
		batch_values.push(value);
	}

	let extrapolated = create_extrapolated_values(&batch_values, &options.interpolation_patterns);
	let extrapolated_strings: Vec<String> = extrapolated.iter().map(|e| e.extrapolated_value.clone()).collect();
	let translated_extrapolated = translator
		.translate_values(&from.language, &to.language, extrapolated_strings)
		.await?;
	let translated_values = restore_extrapolated_values(&extrapolated, &translated_extrapolated)?;

	TranslationValidator::validate_translation(&batch_values, &translated_values, &options.interpolation_patterns)?;

	let mut translations: Vec<(String, String)> = Vec::with_capacity(batch_keys.len());

	for (index, key) in batch_keys.into_iter().enumerate() {
		let translated_value = translated_values[index].clone();

		translations.push((key, translated_value));
	}

	Ok(translations)
}

async fn resolve_indent(
	target: &I18nDefinition,
	reference: &I18nDefinition,
	mode: GenerationMode,
) -> J18nResult<String> {
	if mode == GenerationMode::Sync {
		if let Some(detected) = detect_indentation(&target.file).await? {
			return Ok(detected);
		}
	}

	if let Some(detected) = detect_indentation(&reference.file).await? {
		return Ok(detected);
	}

	Ok(DEFAULT_INDENT.to_string())
}

fn merge_json_objects(first: &Map<String, Value>, second: &Map<String, Value>) -> Map<String, Value> {
	let mut merged = first.clone();

	for (key, value) in second {
		merged.insert(key.clone(), value.clone());
	}

	merged
}

#[cfg(test)]
mod tests {
	use super::*;
	use async_trait::async_trait;
	use j18n_core::PathPattern;
	use j18n_io::{content_hash_hex, I18nHashing, I18nHashingStore};
	use regex::Regex;
	use std::collections::HashMap;
	use std::sync::Mutex;
	use tempfile::TempDir;
	use tokio::fs;

	#[derive(Default)]
	struct MockTranslator {
		captured: Mutex<Vec<(String, String, Vec<String>)>>,
		responses: Mutex<HashMap<String, String>>,
	}

	impl MockTranslator {
		fn with_response(self, input: &str, output: &str) -> Self {
			self.responses.lock().unwrap().insert(input.into(), output.into());
			self
		}

		fn captured_inputs(&self) -> Vec<String> {
			self.captured
				.lock()
				.unwrap()
				.iter()
				.flat_map(|(_, _, values)| values.iter().cloned())
				.collect()
		}
	}

	#[async_trait]
	impl I18nTranslator for MockTranslator {
		fn translator_id(&self) -> &str {
			"mock"
		}

		async fn translate_values(
			&self,
			from_language: &str,
			to_language: &str,
			values: Vec<String>,
		) -> J18nResult<Vec<String>> {
			self.captured
				.lock()
				.unwrap()
				.push((from_language.to_string(), to_language.to_string(), values.clone()));

			let responses = self.responses.lock().unwrap();
			let translated = values
				.iter()
				.map(|value| {
					responses
						.get(value)
						.cloned()
						.unwrap_or_else(|| format!("[{to_language}]{value}"))
				})
				.collect();

			Ok(translated)
		}
	}

	fn definition_in(dir: &TempDir, code: &str) -> I18nDefinition {
		let file = dir.path().join(format!("{code}.json"));
		let id = format!("{code}.json");

		I18nDefinition {
			file,
			id,
			language: code.to_string(),
		}
	}

	async fn read_json(path: &std::path::Path) -> Map<String, Value> {
		let raw = fs::read_to_string(path).await.unwrap();

		serde_json::from_str(&raw).unwrap()
	}

	fn default_options(dir: &TempDir) -> J18nOptions {
		J18nOptions {
			batch_size: 50,
			exclude_patterns: vec![],
			hash_cache_location: dir.path().join(".j18n-cache"),
			interpolation_patterns: vec![Regex::new(r"\{\{(.+?)\}\}").unwrap()],
			parallel_batches: 3,
			retries_per_error: 0,
		}
	}

	#[tokio::test]
	async fn sync_without_hash_cache_treats_every_key_as_changed() {
		let dir = TempDir::new().unwrap();
		let reference = definition_in(&dir, "en");
		let target = definition_in(&dir, "pt");

		fs::write(&reference.file, "{\n\t\"a\": \"A\",\n\t\"b\": \"B\"\n}\n")
			.await
			.unwrap();
		fs::write(&target.file, r#"{"a": "AA"}"#).await.unwrap();

		let translator = MockTranslator::default();

		I18nGenerator::execute(
			&translator,
			&reference,
			std::slice::from_ref(&target),
			GenerationMode::Sync,
			&default_options(&dir),
		)
		.await
		.unwrap();

		let mut inputs = translator.captured_inputs();

		inputs.sort();
		assert_eq!(inputs, vec!["A".to_string(), "B".to_string()]);
	}

	#[tokio::test]
	async fn sync_with_multiple_targets_writes_every_file_and_cache_section() {
		let dir = TempDir::new().unwrap();
		let reference = definition_in(&dir, "en");
		let pt = definition_in(&dir, "pt");
		let es = definition_in(&dir, "es");
		let fr = definition_in(&dir, "fr");

		fs::write(&reference.file, r#"{"a": "A", "b": "B"}"#).await.unwrap();

		let translator = MockTranslator::default();

		I18nGenerator::execute(
			&translator,
			&reference,
			&[pt.clone(), es.clone(), fr.clone()],
			GenerationMode::Sync,
			&default_options(&dir),
		)
		.await
		.unwrap();

		// Every target file is written with its own translations, proving the
		// targets ran concurrently without clobbering each other's output.
		for target in [&pt, &es, &fr] {
			let written = read_json(&target.file).await;

			assert_eq!(written["a"], format!("[{}]A", target.language));
			assert_eq!(written["b"], format!("[{}]B", target.language));
		}

		// Concurrent saves to the single shared cache file must not corrupt or
		// drop any target's section.
		let store = I18nHashingStore::at(&default_options(&dir).hash_cache_location);

		for target in [&pt, &es, &fr] {
			let hashing = store.load(&target_identifier(target)).await.unwrap();

			assert_eq!(hashing.json_key_to_hash_map.get("a").unwrap(), &content_hash_hex("A"));
			assert_eq!(hashing.json_key_to_hash_map.get("b").unwrap(), &content_hash_hex("B"));
		}
	}

	#[tokio::test]
	async fn sync_only_translates_missing_keys_when_hash_cache_matches() {
		let dir = TempDir::new().unwrap();
		let reference = definition_in(&dir, "en");
		let target = definition_in(&dir, "pt");

		fs::write(&reference.file, "{\n\t\"a\": \"A\",\n\t\"b\": \"B\"\n}\n")
			.await
			.unwrap();
		fs::write(&target.file, r#"{"a": "AA"}"#).await.unwrap();

		let store = I18nHashingStore::at(&default_options(&dir).hash_cache_location);
		let mut hashes = HashMap::new();

		hashes.insert("a".to_string(), content_hash_hex("A"));
		hashes.insert("b".to_string(), content_hash_hex("B"));
		store
			.save(
				&target_identifier(&target),
				&I18nHashing {
					json_key_to_hash_map: hashes,
				},
			)
			.await
			.unwrap();

		let translator = MockTranslator::default();

		I18nGenerator::execute(
			&translator,
			&reference,
			std::slice::from_ref(&target),
			GenerationMode::Sync,
			&default_options(&dir),
		)
		.await
		.unwrap();

		assert_eq!(translator.captured_inputs(), vec!["B".to_string()]);

		let written = read_json(&target.file).await;

		assert_eq!(written["a"], "AA");
		assert_eq!(written["b"], "[pt]B");
	}

	#[tokio::test]
	async fn regenerate_translates_every_key_overwriting_existing_translations() {
		let dir = TempDir::new().unwrap();
		let reference = definition_in(&dir, "en");
		let target = definition_in(&dir, "pt");

		fs::write(&reference.file, "{\n\t\"a\": \"A\",\n\t\"b\": \"B\"\n}\n")
			.await
			.unwrap();
		fs::write(&target.file, r#"{"a": "AA", "b": "BB"}"#).await.unwrap();

		let translator = MockTranslator::default();

		I18nGenerator::execute(
			&translator,
			&reference,
			std::slice::from_ref(&target),
			GenerationMode::Regenerate,
			&default_options(&dir),
		)
		.await
		.unwrap();

		let mut inputs = translator.captured_inputs();

		inputs.sort();
		assert_eq!(inputs, vec!["A".to_string(), "B".to_string()]);

		let written = read_json(&target.file).await;

		assert_eq!(written["a"], "[pt]A");
		assert_eq!(written["b"], "[pt]B");
	}

	#[tokio::test]
	async fn target_keys_absent_from_reference_are_pruned() {
		let dir = TempDir::new().unwrap();
		let reference = definition_in(&dir, "en");
		let target = definition_in(&dir, "pt");

		fs::write(&reference.file, "{\n\t\"keep\": \"K\"\n}\n").await.unwrap();
		fs::write(&target.file, r#"{"keep": "KK", "stale": "S"}"#)
			.await
			.unwrap();

		let translator = MockTranslator::default();

		I18nGenerator::execute(
			&translator,
			&reference,
			std::slice::from_ref(&target),
			GenerationMode::Sync,
			&default_options(&dir),
		)
		.await
		.unwrap();

		let written = read_json(&target.file).await;

		assert!(written.contains_key("keep"));
		assert!(!written.contains_key("stale"));
	}

	#[tokio::test]
	async fn hash_cache_is_written_after_run() {
		let dir = TempDir::new().unwrap();
		let reference = definition_in(&dir, "en");
		let target = definition_in(&dir, "pt");

		fs::write(&reference.file, r#"{"a": "A"}"#).await.unwrap();

		let translator = MockTranslator::default();

		I18nGenerator::execute(
			&translator,
			&reference,
			std::slice::from_ref(&target),
			GenerationMode::Sync,
			&default_options(&dir),
		)
		.await
		.unwrap();

		let store = I18nHashingStore::at(&default_options(&dir).hash_cache_location);

		assert!(store.file_path().is_file());

		let hashing = store.load(&target_identifier(&target)).await.unwrap();

		assert_eq!(hashing.json_key_to_hash_map.get("a").unwrap(), &content_hash_hex("A"));
	}

	#[tokio::test]
	async fn nested_keys_round_trip_through_dot_separated_paths() {
		let dir = TempDir::new().unwrap();
		let reference = definition_in(&dir, "en");
		let target = definition_in(&dir, "pt");

		fs::write(
			&reference.file,
			"{\n\t\"section\": {\n\t\t\"a\": \"A\",\n\t\t\"b\": \"B\"\n\t}\n}\n",
		)
		.await
		.unwrap();

		let translator = MockTranslator::default()
			.with_response("A", "AA-pt")
			.with_response("B", "BB-pt");

		I18nGenerator::execute(
			&translator,
			&reference,
			std::slice::from_ref(&target),
			GenerationMode::Regenerate,
			&default_options(&dir),
		)
		.await
		.unwrap();

		let written = read_json(&target.file).await;

		assert_eq!(written["section"]["a"], "AA-pt");
		assert_eq!(written["section"]["b"], "BB-pt");
	}

	#[tokio::test]
	async fn excluded_keys_are_not_translated_or_written_to_target() {
		let dir = TempDir::new().unwrap();
		let reference = definition_in(&dir, "en");
		let target = definition_in(&dir, "pt");

		fs::write(
			&reference.file,
			"{\n\t\"sample\": {\n\t\t\"x\": \"DEMO\"\n\t},\n\t\"real\": \"R\"\n}\n",
		)
		.await
		.unwrap();

		let translator = MockTranslator::default();
		let mut options = default_options(&dir);

		options.exclude_patterns = vec![PathPattern::parse("sample.**").unwrap()];

		I18nGenerator::execute(
			&translator,
			&reference,
			std::slice::from_ref(&target),
			GenerationMode::Regenerate,
			&options,
		)
		.await
		.unwrap();

		assert_eq!(translator.captured_inputs(), vec!["R".to_string()]);

		let written = read_json(&target.file).await;

		assert!(!written.contains_key("sample"));
		assert!(written.contains_key("real"));
	}

	#[tokio::test]
	async fn interpolations_are_extrapolated_and_restored() {
		let dir = TempDir::new().unwrap();
		let reference = definition_in(&dir, "en");
		let target = definition_in(&dir, "pt");

		fs::write(&reference.file, r#"{"greet": "Hi {{name}}!"}"#)
			.await
			.unwrap();

		let translator = MockTranslator::default().with_response("Hi [0]!", "Olá [0]!");

		I18nGenerator::execute(
			&translator,
			&reference,
			std::slice::from_ref(&target),
			GenerationMode::Regenerate,
			&default_options(&dir),
		)
		.await
		.unwrap();

		let written = read_json(&target.file).await;

		assert_eq!(written["greet"], "Olá {{name}}!");
	}

	#[tokio::test]
	async fn output_indentation_is_taken_from_reference_when_target_is_missing() {
		let dir = TempDir::new().unwrap();
		let reference = definition_in(&dir, "en");
		let target = definition_in(&dir, "pt");

		fs::write(&reference.file, "{\n  \"a\": \"A\"\n}\n").await.unwrap();

		let translator = MockTranslator::default();

		I18nGenerator::execute(
			&translator,
			&reference,
			std::slice::from_ref(&target),
			GenerationMode::Regenerate,
			&default_options(&dir),
		)
		.await
		.unwrap();

		let written = fs::read_to_string(&target.file).await.unwrap();

		assert!(
			written.contains("\n  \"a\""),
			"expected 2-space indent, got:\n{written}"
		);
	}

	#[tokio::test]
	async fn output_indentation_is_taken_from_existing_target_when_syncing() {
		let dir = TempDir::new().unwrap();
		let reference = definition_in(&dir, "en");
		let target = definition_in(&dir, "pt");

		fs::write(&reference.file, "{\n\t\"a\": \"A\",\n\t\"b\": \"B\"\n}\n")
			.await
			.unwrap();
		fs::write(&target.file, "{\n    \"a\": \"AA\"\n}\n").await.unwrap();

		let translator = MockTranslator::default();
		let store = I18nHashingStore::at(&default_options(&dir).hash_cache_location);
		let mut hashes = HashMap::new();

		hashes.insert("a".to_string(), content_hash_hex("A"));
		hashes.insert("b".to_string(), content_hash_hex("B"));
		store
			.save(
				&target_identifier(&target),
				&I18nHashing {
					json_key_to_hash_map: hashes,
				},
			)
			.await
			.unwrap();

		I18nGenerator::execute(
			&translator,
			&reference,
			std::slice::from_ref(&target),
			GenerationMode::Sync,
			&default_options(&dir),
		)
		.await
		.unwrap();

		let written = fs::read_to_string(&target.file).await.unwrap();

		assert!(
			written.contains("\n    \"a\""),
			"expected 4-space indent, got:\n{written}"
		);
	}

	#[tokio::test]
	async fn batch_size_controls_number_of_calls_to_translator() {
		let dir = TempDir::new().unwrap();
		let reference = definition_in(&dir, "en");
		let target = definition_in(&dir, "pt");

		fs::write(&reference.file, r#"{"a":"A","b":"B","c":"C","d":"D","e":"E"}"#)
			.await
			.unwrap();

		let translator = MockTranslator::default();
		let mut options = default_options(&dir);

		options.batch_size = 2;

		I18nGenerator::execute(
			&translator,
			&reference,
			std::slice::from_ref(&target),
			GenerationMode::Regenerate,
			&options,
		)
		.await
		.unwrap();

		let captured = translator.captured.lock().unwrap();

		assert_eq!(
			captured.len(),
			3,
			"5 entries with batch_size=2 should produce 3 batches"
		);
	}

	#[tokio::test]
	async fn baseline_writes_reference_hashes_for_each_target() {
		let dir = TempDir::new().unwrap();
		let reference = definition_in(&dir, "en");
		let pt = definition_in(&dir, "pt");
		let es = definition_in(&dir, "es");

		fs::write(&reference.file, r#"{"a": "A", "b": "B"}"#).await.unwrap();
		fs::write(&pt.file, r#"{"a": "AA", "b": "BB"}"#).await.unwrap();
		fs::write(&es.file, r#"{"a": "AAA", "b": "BBB"}"#).await.unwrap();

		let count = I18nGenerator::baseline(&reference, &[pt.clone(), es.clone()], &default_options(&dir))
			.await
			.unwrap();

		assert_eq!(count, 2);

		let store = I18nHashingStore::at(&default_options(&dir).hash_cache_location);
		let pt_hashing = store.load(&target_identifier(&pt)).await.unwrap();
		let es_hashing = store.load(&target_identifier(&es)).await.unwrap();

		assert_eq!(
			pt_hashing.json_key_to_hash_map.get("a").unwrap(),
			&content_hash_hex("A")
		);
		assert_eq!(
			pt_hashing.json_key_to_hash_map.get("b").unwrap(),
			&content_hash_hex("B")
		);
		assert_eq!(
			es_hashing.json_key_to_hash_map.get("a").unwrap(),
			&content_hash_hex("A")
		);
	}

	#[tokio::test]
	async fn baseline_skips_reference_keys_that_are_missing_from_target() {
		let dir = TempDir::new().unwrap();
		let reference = definition_in(&dir, "en");
		let pt = definition_in(&dir, "pt");

		fs::write(&reference.file, r#"{"a": "A", "b": "B", "c": "C"}"#)
			.await
			.unwrap();
		fs::write(&pt.file, r#"{"a": "AA", "b": "BB"}"#).await.unwrap();

		I18nGenerator::baseline(&reference, std::slice::from_ref(&pt), &default_options(&dir))
			.await
			.unwrap();

		let store = I18nHashingStore::at(&default_options(&dir).hash_cache_location);
		let pt_hashing = store.load(&target_identifier(&pt)).await.unwrap();

		assert_eq!(pt_hashing.json_key_to_hash_map.len(), 2);
		assert!(pt_hashing.json_key_to_hash_map.contains_key("a"));
		assert!(pt_hashing.json_key_to_hash_map.contains_key("b"));
		assert!(
			!pt_hashing.json_key_to_hash_map.contains_key("c"),
			"baseline must NOT write a hash for a key that is missing from the target",
		);
	}

	#[tokio::test]
	async fn baseline_writes_empty_hashing_when_target_file_does_not_exist() {
		let dir = TempDir::new().unwrap();
		let reference = definition_in(&dir, "en");
		let pt = definition_in(&dir, "pt");

		fs::write(&reference.file, r#"{"a": "A"}"#).await.unwrap();

		I18nGenerator::baseline(&reference, std::slice::from_ref(&pt), &default_options(&dir))
			.await
			.unwrap();

		let store = I18nHashingStore::at(&default_options(&dir).hash_cache_location);
		let pt_hashing = store.load(&target_identifier(&pt)).await.unwrap();

		assert!(
			pt_hashing.json_key_to_hash_map.is_empty(),
			"baseline against a non-existent target file should produce an empty hashing so sync translates everything",
		);
	}

	#[tokio::test]
	async fn baseline_merges_with_existing_cache_overriding_target_match_and_preserving_others() {
		let dir = TempDir::new().unwrap();
		let reference = definition_in(&dir, "en");
		let pt = definition_in(&dir, "pt");

		fs::write(&reference.file, r#"{"a": "A-NEW"}"#).await.unwrap();
		fs::write(&pt.file, r#"{"a": "AA"}"#).await.unwrap();

		let store = I18nHashingStore::at(&default_options(&dir).hash_cache_location);
		let mut stale_pt_hashes = HashMap::new();

		stale_pt_hashes.insert("a".to_string(), content_hash_hex("A-OLD"));
		store
			.save(
				&target_identifier(&pt),
				&I18nHashing {
					json_key_to_hash_map: stale_pt_hashes,
				},
			)
			.await
			.unwrap();

		let mut other_hashes = HashMap::new();

		other_hashes.insert("x".to_string(), content_hash_hex("X"));
		store
			.save(
				"unrelated/zh.json@Mandarin",
				&I18nHashing {
					json_key_to_hash_map: other_hashes,
				},
			)
			.await
			.unwrap();

		I18nGenerator::baseline(&reference, std::slice::from_ref(&pt), &default_options(&dir))
			.await
			.unwrap();

		let pt_hashing = store.load(&target_identifier(&pt)).await.unwrap();

		assert_eq!(
			pt_hashing.json_key_to_hash_map.get("a").unwrap(),
			&content_hash_hex("A-NEW"),
			"baseline must override the stale hash for the target it touches",
		);

		let unrelated = store.load("unrelated/zh.json@Mandarin").await.unwrap();

		assert_eq!(
			unrelated.json_key_to_hash_map.get("x").unwrap(),
			&content_hash_hex("X"),
			"baseline must preserve cache entries for targets it does not touch",
		);
	}

	#[tokio::test]
	async fn sync_after_baseline_translates_only_keys_missing_from_target() {
		let dir = TempDir::new().unwrap();
		let reference = definition_in(&dir, "en");
		let pt = definition_in(&dir, "pt");

		fs::write(&reference.file, r#"{"a": "A", "b": "B", "c": "C"}"#)
			.await
			.unwrap();
		fs::write(&pt.file, r#"{"a": "AA", "b": "BB"}"#).await.unwrap();

		I18nGenerator::baseline(&reference, std::slice::from_ref(&pt), &default_options(&dir))
			.await
			.unwrap();

		let translator = MockTranslator::default();

		I18nGenerator::execute(
			&translator,
			&reference,
			std::slice::from_ref(&pt),
			GenerationMode::Sync,
			&default_options(&dir),
		)
		.await
		.unwrap();

		assert_eq!(
			translator.captured_inputs(),
			vec!["C".to_string()],
			"sync after partial baseline must only translate the keys that were missing from the target",
		);
	}

	#[tokio::test]
	async fn check_after_baseline_reports_no_changes_when_target_keys_match_reference() {
		let dir = TempDir::new().unwrap();
		let reference = definition_in(&dir, "en");
		let pt = definition_in(&dir, "pt");

		fs::write(&reference.file, r#"{"a": "A", "b": "B"}"#).await.unwrap();
		fs::write(&pt.file, r#"{"a": "AA", "b": "BB"}"#).await.unwrap();

		I18nGenerator::baseline(&reference, std::slice::from_ref(&pt), &default_options(&dir))
			.await
			.unwrap();
		let report = I18nGenerator::check(&reference, std::slice::from_ref(&pt), &default_options(&dir))
			.await
			.unwrap();

		assert_eq!(report.targets.len(), 1);
		assert!(!report.targets[0].needs_sync());
	}

	#[tokio::test]
	async fn sync_after_baseline_does_not_call_translator_for_unchanged_keys() {
		let dir = TempDir::new().unwrap();
		let reference = definition_in(&dir, "en");
		let pt = definition_in(&dir, "pt");

		fs::write(&reference.file, r#"{"a": "A", "b": "B"}"#).await.unwrap();
		fs::write(&pt.file, r#"{"a": "AA", "b": "BB"}"#).await.unwrap();

		I18nGenerator::baseline(&reference, std::slice::from_ref(&pt), &default_options(&dir))
			.await
			.unwrap();

		let translator = MockTranslator::default();

		I18nGenerator::execute(
			&translator,
			&reference,
			std::slice::from_ref(&pt),
			GenerationMode::Sync,
			&default_options(&dir),
		)
		.await
		.unwrap();

		assert!(
			translator.captured_inputs().is_empty(),
			"baseline should leave the cache fully matching the reference; no translation calls expected, got {:?}",
			translator.captured_inputs()
		);
	}

	struct FlakyTranslator {
		calls: Mutex<usize>,
		fail_first_n: usize,
	}

	impl FlakyTranslator {
		fn new(fail_first_n: usize) -> Self {
			Self {
				calls: Mutex::new(0),
				fail_first_n,
			}
		}

		fn total_calls(&self) -> usize {
			*self.calls.lock().unwrap()
		}
	}

	#[async_trait]
	impl I18nTranslator for FlakyTranslator {
		fn translator_id(&self) -> &str {
			"flaky"
		}

		async fn translate_values(
			&self,
			_from_language: &str,
			to_language: &str,
			values: Vec<String>,
		) -> J18nResult<Vec<String>> {
			let mut calls = self.calls.lock().unwrap();

			*calls += 1;

			if *calls <= self.fail_first_n {
				return Err(j18n_core::J18nError::translator(format!(
					"simulated transient error on attempt {}",
					*calls
				)));
			}

			Ok(values.iter().map(|v| format!("[{to_language}]{v}")).collect())
		}
	}

	#[tokio::test]
	async fn translate_batch_retries_on_translator_error_and_eventually_succeeds() {
		let dir = TempDir::new().unwrap();
		let reference = definition_in(&dir, "en");
		let target = definition_in(&dir, "pt");

		fs::write(&reference.file, r#"{"a": "A"}"#).await.unwrap();

		let translator = FlakyTranslator::new(2);
		let mut options = default_options(&dir);

		options.retries_per_error = 3;

		I18nGenerator::execute(
			&translator,
			&reference,
			std::slice::from_ref(&target),
			GenerationMode::Sync,
			&options,
		)
		.await
		.unwrap();

		assert_eq!(
			translator.total_calls(),
			3,
			"translator should have been called 3 times: 2 failures + 1 success"
		);

		let written = read_json(&target.file).await;

		assert_eq!(written["a"], "[pt]A");
	}

	#[tokio::test]
	async fn translate_batch_gives_up_after_retries_per_error_attempts() {
		let dir = TempDir::new().unwrap();
		let reference = definition_in(&dir, "en");
		let target = definition_in(&dir, "pt");

		fs::write(&reference.file, r#"{"a": "A"}"#).await.unwrap();

		let translator = FlakyTranslator::new(usize::MAX);
		let mut options = default_options(&dir);

		options.retries_per_error = 2;

		let err = I18nGenerator::execute(
			&translator,
			&reference,
			std::slice::from_ref(&target),
			GenerationMode::Sync,
			&options,
		)
		.await
		.unwrap_err();

		assert!(matches!(err, J18nError::Translator(_)));
		assert_eq!(
			translator.total_calls(),
			3,
			"translator should have been called 3 times: 1 initial attempt + 2 retries"
		);
	}

	#[tokio::test]
	async fn translate_batch_with_zero_retries_only_calls_translator_once() {
		let dir = TempDir::new().unwrap();
		let reference = definition_in(&dir, "en");
		let target = definition_in(&dir, "pt");

		fs::write(&reference.file, r#"{"a": "A"}"#).await.unwrap();

		let translator = FlakyTranslator::new(usize::MAX);
		let options = default_options(&dir);

		assert_eq!(options.retries_per_error, 0);

		let err = I18nGenerator::execute(
			&translator,
			&reference,
			std::slice::from_ref(&target),
			GenerationMode::Sync,
			&options,
		)
		.await
		.unwrap_err();

		assert!(matches!(err, J18nError::Translator(_)));
		assert_eq!(
			translator.total_calls(),
			1,
			"with retries_per_error=0 the translator should be called exactly once"
		);
	}
}
