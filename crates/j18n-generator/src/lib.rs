pub mod options;

pub use options::J18nOptions;

use futures::stream::{self, StreamExt};
use j18n_core::{GenerationMode, I18nDefinition, J18nResult};
use j18n_io::{detect_indentation, read_i18n_data, write_i18n_tree_map, I18nHashing, I18nHashingCache, DEFAULT_INDENT};
use j18n_translator::{create_extrapolated_values, restore_extrapolated_values, I18nTranslator};
use j18n_validator::TranslationValidator;
use serde_json::{Map, Value};
use std::collections::BTreeSet;
use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::Arc;
use tracing::info;

pub fn target_identifier(definition: &I18nDefinition) -> String {
	format!("{}@{}", definition.id, definition.language)
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
		let mut hash_cache = I18nHashingCache::load_from(&options.hash_cache_path).await?;

		info!(
			"Scanned {} total entries from {} dict",
			reference_data.walked_tree_map.len(),
			reference_i18n.language
		);

		for target in generate_i18n_for {
			let target_id = target_identifier(target);
			let cached_hashing = hash_cache
				.get(&target_id)
				.cloned()
				.unwrap_or_else(I18nHashing::empty);
			let changed_keys = cached_hashing.compute_changed_keys(&reference_hashing);

			info!(
				"{} keys changed since {} was last synced",
				changed_keys.len(),
				target.language
			);

			translate_into_target(
				translator,
				reference_i18n,
				&reference_data,
				&changed_keys,
				target,
				mode,
				options,
			)
			.await?;

			hash_cache.set(target_id, reference_hashing.clone());
			hash_cache.save_to(&options.hash_cache_path).await?;
		}

		Ok(())
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
) -> J18nResult<()>
where
	T: I18nTranslator + ?Sized,
{
	let target_data = read_i18n_data(target, &options.exclude_patterns).await?;
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
	let total_characters: usize = entries_to_translate.iter().map(|(_, value)| value.len()).sum();
	let windowed_entries: Vec<Vec<(String, String)>> = entries_to_translate
		.chunks(options.batch_size)
		.map(|chunk| chunk.to_vec())
		.collect();

	info!(
		"Translating {} entries ({} characters) to {} in a total of {} batches...",
		entries_to_translate.len(),
		total_characters,
		target.language,
		windowed_entries.len()
	);

	let total_batches = windowed_entries.len();
	let translated_count = Arc::new(AtomicUsize::new(0));
	let translated_batches: Vec<J18nResult<Vec<(String, String)>>> =
		stream::iter(windowed_entries.into_iter().map(|window| {
			let translated_count = Arc::clone(&translated_count);

			async move {
				let result = translate_batch(translator, reference_i18n, target, window, options).await;
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

	info!("Writing ({mode}) JSON to \"{}\"...", target.file.display());

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

	Ok(())
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
	use j18n_io::{java_string_hashcode_hex, I18nHashing, I18nHashingCache};
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
			hash_cache_path: dir.path().join(".hash-cache.json"),
			interpolation_patterns: vec![Regex::new(r"\{\{(.+?)\}\}").unwrap()],
			parallel_batches: 3,
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
			&[target.clone()],
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
	async fn sync_only_translates_missing_keys_when_hash_cache_matches() {
		let dir = TempDir::new().unwrap();
		let reference = definition_in(&dir, "en");
		let target = definition_in(&dir, "pt");

		fs::write(&reference.file, "{\n\t\"a\": \"A\",\n\t\"b\": \"B\"\n}\n")
			.await
			.unwrap();
		fs::write(&target.file, r#"{"a": "AA"}"#).await.unwrap();

		let mut cache = I18nHashingCache::empty();
		let mut hashes = HashMap::new();

		hashes.insert("a".to_string(), java_string_hashcode_hex("A"));
		hashes.insert("b".to_string(), java_string_hashcode_hex("B"));
		cache.set(
			target_identifier(&target),
			I18nHashing { json_key_to_hash_map: hashes },
		);
		cache.save_to(&dir.path().join(".hash-cache.json")).await.unwrap();

		let translator = MockTranslator::default();

		I18nGenerator::execute(
			&translator,
			&reference,
			&[target.clone()],
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
			&[target.clone()],
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
		fs::write(&target.file, r#"{"keep": "KK", "stale": "S"}"#).await.unwrap();

		let translator = MockTranslator::default();

		I18nGenerator::execute(
			&translator,
			&reference,
			&[target.clone()],
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
			&[target.clone()],
			GenerationMode::Sync,
			&default_options(&dir),
		)
		.await
		.unwrap();

		let cache_path = dir.path().join(".hash-cache.json");

		assert!(cache_path.exists());

		let cache = I18nHashingCache::load_from(&cache_path).await.unwrap();
		let hashing = cache.get(&target_identifier(&target)).unwrap();

		assert_eq!(
			hashing.json_key_to_hash_map.get("a").unwrap(),
			&java_string_hashcode_hex("A")
		);
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
			&[target.clone()],
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
			&[target.clone()],
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
			&[target.clone()],
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
			&[target.clone()],
			GenerationMode::Regenerate,
			&default_options(&dir),
		)
		.await
		.unwrap();

		let written = fs::read_to_string(&target.file).await.unwrap();

		assert!(written.contains("\n  \"a\""), "expected 2-space indent, got:\n{written}");
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
		let mut cache = I18nHashingCache::empty();
		let mut hashes = HashMap::new();

		hashes.insert("a".to_string(), java_string_hashcode_hex("A"));
		hashes.insert("b".to_string(), java_string_hashcode_hex("B"));
		cache.set(
			target_identifier(&target),
			I18nHashing { json_key_to_hash_map: hashes },
		);
		cache.save_to(&dir.path().join(".hash-cache.json")).await.unwrap();

		I18nGenerator::execute(
			&translator,
			&reference,
			&[target.clone()],
			GenerationMode::Sync,
			&default_options(&dir),
		)
		.await
		.unwrap();

		let written = fs::read_to_string(&target.file).await.unwrap();

		assert!(written.contains("\n    \"a\""), "expected 4-space indent, got:\n{written}");
	}

	#[tokio::test]
	async fn batch_size_controls_number_of_calls_to_translator() {
		let dir = TempDir::new().unwrap();
		let reference = definition_in(&dir, "en");
		let target = definition_in(&dir, "pt");

		fs::write(
			&reference.file,
			r#"{"a":"A","b":"B","c":"C","d":"D","e":"E"}"#,
		)
		.await
		.unwrap();

		let translator = MockTranslator::default();
		let mut options = default_options(&dir);

		options.batch_size = 2;

		I18nGenerator::execute(
			&translator,
			&reference,
			&[target.clone()],
			GenerationMode::Regenerate,
			&options,
		)
		.await
		.unwrap();

		let captured = translator.captured.lock().unwrap();

		assert_eq!(captured.len(), 3, "5 entries with batch_size=2 should produce 3 batches");
	}
}
