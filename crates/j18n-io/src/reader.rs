use crate::json_walker::walk_json_tree_to_map;
use j18n_core::{key_matches_any, I18nData, I18nDefinition, J18nError, J18nResult, PathPattern};
use serde_json::{Map, Value};
use tokio::fs;

pub async fn read_i18n_data(definition: &I18nDefinition, exclude_patterns: &[PathPattern]) -> J18nResult<I18nData> {
	let path = &definition.file;

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
	let parsed_dict: Map<String, Value> =
		serde_json::from_slice(&raw_bytes).map_err(|source| J18nError::Json {
			path: path.clone(),
			source,
		})?;
	let json_dict = filter_excluded(&parsed_dict, "", exclude_patterns);
	let walked_tree_map = walk_json_tree_to_map(&json_dict);

	Ok(I18nData {
		json_dict,
		walked_tree_map,
	})
}

fn filter_excluded(json: &Map<String, Value>, prefix: &str, patterns: &[PathPattern]) -> Map<String, Value> {
	let mut result = Map::new();

	for (key, value) in json {
		let path = if prefix.is_empty() {
			key.clone()
		} else {
			format!("{prefix}.{key}")
		};

		if key_matches_any(&path, patterns) {
			continue;
		}

		match value {
			Value::Object(nested) => {
				let filtered = filter_excluded(nested, &path, patterns);

				result.insert(key.clone(), Value::Object(filtered));
			}
			other => {
				result.insert(key.clone(), other.clone());
			}
		}
	}

	result
}

#[cfg(test)]
mod tests {
	use super::*;
	use tempfile::TempDir;
	use tokio::fs;

	fn definition_in(dir: &TempDir, code: &str) -> I18nDefinition {
		let file = dir.path().join(format!("{code}.json"));
		let id = format!("{code}.json");

		I18nDefinition {
			file,
			id,
			language: code.to_string(),
		}
	}

	#[tokio::test]
	async fn returns_empty_data_when_file_is_missing() {
		let dir = TempDir::new().unwrap();
		let definition = definition_in(&dir, "en");

		let data = read_i18n_data(&definition, &[]).await.unwrap();

		assert!(data.json_dict.is_empty());
		assert!(data.walked_tree_map.is_empty());
	}

	#[tokio::test]
	async fn parses_flat_object_without_excludes() {
		let dir = TempDir::new().unwrap();
		let definition = definition_in(&dir, "en");

		fs::write(&definition.file, r#"{"a": "1", "b": "2"}"#)
			.await
			.unwrap();

		let data = read_i18n_data(&definition, &[]).await.unwrap();

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
			&definition.file,
			r#"{"section": {"key": "value"}, "other": "x"}"#,
		)
		.await
		.unwrap();

		let data = read_i18n_data(&definition, &[]).await.unwrap();

		assert_eq!(
			data.walked_tree_map,
			vec![("section.key".into(), "value".into()), ("other".into(), "x".into())]
		);
	}

	#[tokio::test]
	async fn excludes_keys_matching_top_level_pattern() {
		let dir = TempDir::new().unwrap();
		let definition = definition_in(&dir, "en");

		fs::write(&definition.file, r#"{"sample": "X", "real": "Y"}"#)
			.await
			.unwrap();

		let patterns = vec![PathPattern::parse("sample").unwrap()];
		let data = read_i18n_data(&definition, &patterns).await.unwrap();

		assert!(!data.json_dict.contains_key("sample"));
		assert_eq!(data.walked_tree_map, vec![("real".into(), "Y".into())]);
	}

	#[tokio::test]
	async fn excludes_keys_matching_double_star_pattern() {
		let dir = TempDir::new().unwrap();
		let definition = definition_in(&dir, "en");

		fs::write(
			&definition.file,
			r#"{"sample": {"a": "X", "b": "Y"}, "real": "Z"}"#,
		)
		.await
		.unwrap();

		let patterns = vec![PathPattern::parse("sample.**").unwrap()];
		let data = read_i18n_data(&definition, &patterns).await.unwrap();

		assert!(!data.json_dict.contains_key("sample"));
		assert_eq!(data.walked_tree_map, vec![("real".into(), "Z".into())]);
	}

	#[tokio::test]
	async fn returns_error_for_invalid_json() {
		let dir = TempDir::new().unwrap();
		let definition = definition_in(&dir, "en");

		fs::write(&definition.file, "not json").await.unwrap();

		let err = read_i18n_data(&definition, &[]).await.unwrap_err();

		assert!(matches!(err, J18nError::Json { .. }));
	}
}
