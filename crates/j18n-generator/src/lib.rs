use futures::stream::{self, StreamExt};
use j18n_core::{GenerationMode, I18nDefinition, J18nResult};
use j18n_io::{read_i18n_data, write_i18n_tree_map, I18nHashingCache};
use j18n_translator::I18nTranslator;
use j18n_validator::TranslationValidator;
use serde_json::{Map, Value};
use std::collections::BTreeSet;
use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::Arc;
use tracing::info;

const BATCH_SIZE: usize = 50;
const PARALLEL_LIMIT: usize = 3;
const HASH_CACHE_FILE_NAME: &str = ".hash-cache.json";

pub struct I18nGenerator;

impl I18nGenerator {
	pub async fn execute<T>(
		translator: &T,
		reference_i18n: &I18nDefinition,
		generate_i18n_for: &[I18nDefinition],
		mode: GenerationMode,
	) -> J18nResult<()>
	where
		T: I18nTranslator + ?Sized,
	{
		let reference_data = read_i18n_data(reference_i18n).await?;
		let reference_entries = reference_data.walked_tree_map.clone();
		let hash_cache_path = reference_i18n
			.json_file_path
			.parent()
			.map(|parent| parent.join(HASH_CACHE_FILE_NAME))
			.expect("reference json must have a parent directory");
		let reference_cached_hashing = I18nHashingCache::load_hash_cache_from(&hash_cache_path).await?;
		let reference_current_hashing = I18nHashingCache::compute_hash_cache_from(&reference_data);
		let changed_keys_since_last_hashing = reference_cached_hashing.compute_changed_keys(&reference_current_hashing);

		info!(
			"Scanned {} total entries from {} dict",
			reference_entries.len(),
			reference_i18n.language.language_name()
		);
		info!(
			"{} keys changed since last translation",
			changed_keys_since_last_hashing.len()
		);

		for target in generate_i18n_for {
			translate_into_target(
				translator,
				reference_i18n,
				&reference_data,
				&reference_entries,
				&changed_keys_since_last_hashing,
				target,
				mode,
			)
			.await?;
		}

		I18nHashingCache::save_hash_cache_to(&reference_current_hashing, &hash_cache_path).await?;

		Ok(())
	}
}

#[allow(clippy::too_many_arguments)]
async fn translate_into_target<T>(
	translator: &T,
	reference_i18n: &I18nDefinition,
	reference_data: &j18n_core::I18nData,
	reference_entries: &[(String, String)],
	changed_keys_since_last_hashing: &BTreeSet<String>,
	target: &I18nDefinition,
	mode: GenerationMode,
) -> J18nResult<()>
where
	T: I18nTranslator + ?Sized,
{
	let target_data = read_i18n_data(target).await?;
	let entries_to_translate: Vec<(String, String)> = match mode {
		GenerationMode::Regenerate => reference_entries.to_vec(),
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
		.chunks(BATCH_SIZE)
		.map(|chunk| chunk.to_vec())
		.collect();

	info!(
		"Translating {} entries ({} characters) to {} in a total of {} batches...",
		entries_to_translate.len(),
		total_characters,
		target.language.language_name(),
		windowed_entries.len()
	);

	let total_batches = windowed_entries.len();
	let translated_count = Arc::new(AtomicUsize::new(0));
	let translated_batches: Vec<J18nResult<Vec<(String, String)>>> =
		stream::iter(windowed_entries.into_iter().map(|window| {
			let translated_count = Arc::clone(&translated_count);

			async move {
				let result = translate_batch(translator, reference_i18n, target, window).await;
				let current = translated_count.fetch_add(1, Ordering::SeqCst) + 1;

				info!("Batch {current}/{total_batches} translated");

				result
			}
		}))
		.buffer_unordered(PARALLEL_LIMIT)
		.collect()
		.await;

	let translated_batches: Vec<Vec<(String, String)>> =
		translated_batches.into_iter().collect::<J18nResult<Vec<_>>>()?;

	info!("Writing ({mode}) JSON to \"{}\"...", target.json_file_path.display());

	let initial_json_dict: Map<String, Value> = match mode {
		GenerationMode::Regenerate => reference_data.json_dict.clone(),
		GenerationMode::Sync => merge_json_objects(&reference_data.json_dict, &target_data.json_dict),
	};

	write_i18n_tree_map(
		target,
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

	let translated_values = translator
		.translate_i18n_values(from.language, to.language, batch_values.clone())
		.await?;

	TranslationValidator::validate_translation(&batch_values, &translated_values)?;

	let mut translations: Vec<(String, String)> = Vec::with_capacity(batch_keys.len());

	for (index, key) in batch_keys.into_iter().enumerate() {
		let translated_value = translated_values[index].clone();

		translations.push((key, translated_value));
	}

	Ok(translations)
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
	use j18n_core::Language;
	use j18n_io::{java_string_hashcode_hex, I18nHashing, I18nHashingCache};
	use std::collections::HashMap;
	use std::sync::Mutex;
	use tempfile::TempDir;
	use tokio::fs;

	#[derive(Default)]
	struct MockTranslator {
		captured: Mutex<Vec<(Language, Language, Vec<String>)>>,
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

		async fn translate_i18n_values(
			&self,
			from: Language,
			to: Language,
			values: Vec<String>,
		) -> j18n_core::J18nResult<Vec<String>> {
			self.captured.lock().unwrap().push((from, to, values.clone()));

			let responses = self.responses.lock().unwrap();
			let translated = values
				.iter()
				.map(|value| {
					responses
						.get(value)
						.cloned()
						.unwrap_or_else(|| format!("[{}]{value}", to.iso_639_code()))
				})
				.collect();

			Ok(translated)
		}
	}

	fn definition_in(dir: &TempDir, code: &str) -> I18nDefinition {
		I18nDefinition::from_base_dir(dir.path(), Language::from_iso_639_code(code).unwrap())
	}

	async fn read_json(path: &std::path::Path) -> Map<String, Value> {
		let raw = fs::read_to_string(path).await.unwrap();

		serde_json::from_str(&raw).unwrap()
	}

	#[tokio::test]
	async fn sync_without_hash_cache_treats_every_key_as_changed() {
		let dir = TempDir::new().unwrap();
		let reference = definition_in(&dir, "en");
		let target = definition_in(&dir, "pt");

		fs::write(&reference.json_file_path, r#"{"a": "A", "b": "B"}"#).await.unwrap();
		fs::write(&target.json_file_path, r#"{"a": "AA"}"#).await.unwrap();

		let translator = MockTranslator::default();

		I18nGenerator::execute(&translator, &reference, &[target.clone()], GenerationMode::Sync)
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

		fs::write(&reference.json_file_path, r#"{"a": "A", "b": "B"}"#).await.unwrap();
		fs::write(&target.json_file_path, r#"{"a": "AA"}"#).await.unwrap();

		let mut cache = HashMap::new();

		cache.insert("a".to_string(), java_string_hashcode_hex("A"));
		cache.insert("b".to_string(), java_string_hashcode_hex("B"));

		I18nHashingCache::save_hash_cache_to(
			&I18nHashing {
				json_key_to_hash_map: cache,
			},
			&dir.path().join(".hash-cache.json"),
		)
		.await
		.unwrap();

		let translator = MockTranslator::default();

		I18nGenerator::execute(&translator, &reference, &[target.clone()], GenerationMode::Sync)
			.await
			.unwrap();

		assert_eq!(translator.captured_inputs(), vec!["B".to_string()]);

		let written = read_json(&target.json_file_path).await;

		assert_eq!(written["a"], "AA");
		assert_eq!(written["b"], "[pt]B");
	}

	#[tokio::test]
	async fn sync_translates_keys_changed_per_hash_cache() {
		let dir = TempDir::new().unwrap();
		let reference = definition_in(&dir, "en");
		let target = definition_in(&dir, "pt");

		fs::write(&reference.json_file_path, r#"{"a": "A", "b": "B"}"#).await.unwrap();
		fs::write(&target.json_file_path, r#"{"a": "AA", "b": "BB"}"#).await.unwrap();

		let mut cache = HashMap::new();

		cache.insert("a".to_string(), java_string_hashcode_hex("A"));
		cache.insert("b".to_string(), java_string_hashcode_hex("B-old"));

		I18nHashingCache::save_hash_cache_to(
			&I18nHashing {
				json_key_to_hash_map: cache,
			},
			&dir.path().join(".hash-cache.json"),
		)
		.await
		.unwrap();

		let translator = MockTranslator::default();

		I18nGenerator::execute(&translator, &reference, &[target.clone()], GenerationMode::Sync)
			.await
			.unwrap();

		assert_eq!(translator.captured_inputs(), vec!["B".to_string()]);

		let written = read_json(&target.json_file_path).await;

		assert_eq!(written["a"], "AA");
		assert_eq!(written["b"], "[pt]B");
	}

	#[tokio::test]
	async fn regenerate_translates_every_key_overwriting_existing_translations() {
		let dir = TempDir::new().unwrap();
		let reference = definition_in(&dir, "en");
		let target = definition_in(&dir, "pt");

		fs::write(&reference.json_file_path, r#"{"a": "A", "b": "B"}"#).await.unwrap();
		fs::write(&target.json_file_path, r#"{"a": "AA", "b": "BB"}"#).await.unwrap();

		let translator = MockTranslator::default();

		I18nGenerator::execute(&translator, &reference, &[target.clone()], GenerationMode::Regenerate)
			.await
			.unwrap();

		let mut inputs = translator.captured_inputs();

		inputs.sort();
		assert_eq!(inputs, vec!["A".to_string(), "B".to_string()]);

		let written = read_json(&target.json_file_path).await;

		assert_eq!(written["a"], "[pt]A");
		assert_eq!(written["b"], "[pt]B");
	}

	#[tokio::test]
	async fn target_keys_absent_from_reference_are_pruned() {
		let dir = TempDir::new().unwrap();
		let reference = definition_in(&dir, "en");
		let target = definition_in(&dir, "pt");

		fs::write(&reference.json_file_path, r#"{"keep": "K"}"#).await.unwrap();
		fs::write(&target.json_file_path, r#"{"keep": "KK", "stale": "S"}"#).await.unwrap();

		let translator = MockTranslator::default();

		I18nGenerator::execute(&translator, &reference, &[target.clone()], GenerationMode::Sync)
			.await
			.unwrap();

		let written = read_json(&target.json_file_path).await;

		assert!(written.contains_key("keep"));
		assert!(!written.contains_key("stale"));
	}

	#[tokio::test]
	async fn hash_cache_is_written_after_run() {
		let dir = TempDir::new().unwrap();
		let reference = definition_in(&dir, "en");
		let target = definition_in(&dir, "pt");

		fs::write(&reference.json_file_path, r#"{"a": "A"}"#).await.unwrap();

		let translator = MockTranslator::default();

		I18nGenerator::execute(&translator, &reference, &[target], GenerationMode::Sync)
			.await
			.unwrap();

		let cache_path = dir.path().join(".hash-cache.json");

		assert!(cache_path.exists());

		let cache = I18nHashingCache::load_hash_cache_from(&cache_path).await.unwrap();

		assert_eq!(
			cache.json_key_to_hash_map.get("a").unwrap(),
			&java_string_hashcode_hex("A")
		);
	}

	#[tokio::test]
	async fn nested_keys_round_trip_through_dot_separated_paths() {
		let dir = TempDir::new().unwrap();
		let reference = definition_in(&dir, "en");
		let target = definition_in(&dir, "pt");

		fs::write(
			&reference.json_file_path,
			r#"{"section": {"a": "A", "b": "B"}}"#,
		)
		.await
		.unwrap();

		let translator = MockTranslator::default()
			.with_response("A", "AA-pt")
			.with_response("B", "BB-pt");

		I18nGenerator::execute(&translator, &reference, &[target.clone()], GenerationMode::Regenerate)
			.await
			.unwrap();

		let written = read_json(&target.json_file_path).await;

		assert_eq!(written["section"]["a"], "AA-pt");
		assert_eq!(written["section"]["b"], "BB-pt");
	}

	#[tokio::test]
	async fn regenerate_runs_against_multiple_target_languages() {
		let dir = TempDir::new().unwrap();
		let reference = definition_in(&dir, "en");
		let pt = definition_in(&dir, "pt");
		let es = definition_in(&dir, "es");

		fs::write(&reference.json_file_path, r#"{"a": "A"}"#).await.unwrap();

		let translator = MockTranslator::default();

		I18nGenerator::execute(&translator, &reference, &[pt.clone(), es.clone()], GenerationMode::Regenerate)
			.await
			.unwrap();

		assert_eq!(read_json(&pt.json_file_path).await["a"], "[pt]A");
		assert_eq!(read_json(&es.json_file_path).await["a"], "[es]A");
	}
}
