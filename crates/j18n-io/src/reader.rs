use crate::json_walker::walk_json_tree_to_map;
use j18n_core::{I18nData, I18nDefinition, J18nError, J18nResult};
use serde_json::{Map, Value};
use tokio::fs;

pub async fn read_i18n_data(definition: &I18nDefinition) -> J18nResult<I18nData> {
	let path = &definition.json_file_path;

	if !fs::try_exists(path).await.map_err(|source| J18nError::Io {
		path: path.clone(),
		source,
	})? {
		return Ok(I18nData::empty());
	}

	let raw_bytes = fs::read(path).await.map_err(|source| J18nError::Io {
		path: path.clone(),
		source,
	})?;
	let mut json_dict: Map<String, Value> = serde_json::from_slice(&raw_bytes).map_err(|source| J18nError::Json {
		path: path.clone(),
		source,
	})?;

	json_dict.remove("sample");

	let walked_tree_map = walk_json_tree_to_map(&json_dict);

	Ok(I18nData {
		json_dict,
		walked_tree_map,
	})
}

#[cfg(test)]
mod tests {
	use super::*;
	use j18n_core::Language;
	use tempfile::TempDir;
	use tokio::fs;

	fn definition_in(dir: &TempDir, code: &str) -> I18nDefinition {
		I18nDefinition::from_base_dir(dir.path(), Language::from_iso_639_code(code).unwrap())
	}

	#[tokio::test]
	async fn returns_empty_data_when_file_is_missing() {
		let dir = TempDir::new().unwrap();
		let definition = definition_in(&dir, "en");

		let data = read_i18n_data(&definition).await.unwrap();

		assert!(data.json_dict.is_empty());
		assert!(data.walked_tree_map.is_empty());
	}

	#[tokio::test]
	async fn parses_flat_object() {
		let dir = TempDir::new().unwrap();
		let definition = definition_in(&dir, "en");

		fs::write(&definition.json_file_path, r#"{"a": "1", "b": "2"}"#)
			.await
			.unwrap();

		let data = read_i18n_data(&definition).await.unwrap();

		assert_eq!(
			data.walked_tree_map,
			vec![("a".into(), "1".into()), ("b".into(), "2".into())]
		);
	}

	#[tokio::test]
	async fn flattens_nested_dictionaries() {
		let dir = TempDir::new().unwrap();
		let definition = definition_in(&dir, "en");

		fs::write(
			&definition.json_file_path,
			r#"{"section": {"key": "value"}, "other": "x"}"#,
		)
		.await
		.unwrap();

		let data = read_i18n_data(&definition).await.unwrap();

		assert_eq!(
			data.walked_tree_map,
			vec![("section.key".into(), "value".into()), ("other".into(), "x".into())]
		);
	}

	#[tokio::test]
	async fn strips_root_level_sample_key() {
		let dir = TempDir::new().unwrap();
		let definition = definition_in(&dir, "en");

		fs::write(
			&definition.json_file_path,
			r#"{"sample": "X", "real": {"a": "Y"}}"#,
		)
		.await
		.unwrap();

		let data = read_i18n_data(&definition).await.unwrap();

		assert!(!data.json_dict.contains_key("sample"));
		assert_eq!(data.walked_tree_map, vec![("real.a".into(), "Y".into())]);
	}

	#[tokio::test]
	async fn returns_error_for_invalid_json() {
		let dir = TempDir::new().unwrap();
		let definition = definition_in(&dir, "en");

		fs::write(&definition.json_file_path, "not json").await.unwrap();

		let err = read_i18n_data(&definition).await.unwrap_err();

		assert!(matches!(err, j18n_core::J18nError::Json { .. }));
	}
}
