use crate::compare::natural_key_cmp;
use j18n_core::{I18nDefinition, J18nError, J18nResult};
use serde::Serialize;
use serde_json::{Map, Value};
use tokio::fs;
use tokio::io::AsyncWriteExt;

pub async fn write_i18n_tree_map(
	definition: &I18nDefinition,
	indent: &[u8],
	reference_json_dict: &Map<String, Value>,
	initial_json_dict: Map<String, Value>,
	json_tree_map_list: &[Vec<(String, String)>],
) -> J18nResult<()> {
	let mut translated_json_dict = initial_json_dict;

	for batch in json_tree_map_list {
		for (key, value) in batch {
			translated_json_dict = change_i18n_property(translated_json_dict, key, value);
		}
	}

	let cleaned_json_dict = remove_keys_absent_from_reference_dict(reference_json_dict, &translated_json_dict);
	let sorted_json_dict = sort_object_recursively(cleaned_json_dict);
	let serialized = serialize_pretty(&sorted_json_dict, indent).map_err(|source| J18nError::Json {
		path: definition.file.clone(),
		source,
	})?;

	if let Some(parent) = definition.file.parent() {
		fs::create_dir_all(parent).await.map_err(|source| J18nError::Io {
			path: parent.to_path_buf(),
			source,
		})?;
	}

	let mut file = fs::File::create(&definition.file)
		.await
		.map_err(|source| J18nError::Io {
			path: definition.file.clone(),
			source,
		})?;

	file.write_all(serialized.as_bytes())
		.await
		.map_err(|source| J18nError::Io {
			path: definition.file.clone(),
			source,
		})?;
	file.write_all(b"\n").await.map_err(|source| J18nError::Io {
		path: definition.file.clone(),
		source,
	})?;

	Ok(())
}

fn change_i18n_property(mut json: Map<String, Value>, key_dot_separated: &str, value: &str) -> Map<String, Value> {
	if let Some((this_part, rest_parts)) = key_dot_separated.split_once('.') {
		let sub_json = match json.remove(this_part) {
			Some(Value::Object(existing)) => existing,
			_ => Map::new(),
		};
		let changed_sub_json = change_i18n_property(sub_json, rest_parts, value);

		json.insert(this_part.to_string(), Value::Object(changed_sub_json));

		return json;
	}

	json.insert(key_dot_separated.to_string(), Value::String(value.to_string()));

	json
}

fn remove_keys_absent_from_reference_dict(
	reference_dict: &Map<String, Value>,
	target_dict: &Map<String, Value>,
) -> Map<String, Value> {
	let mut result = Map::new();

	for (key, target_value) in target_dict {
		let Some(reference_value) = reference_dict.get(key) else {
			continue;
		};

		match (reference_value, target_value) {
			(Value::Object(reference_sub), Value::Object(target_sub)) => {
				result.insert(
					key.clone(),
					Value::Object(remove_keys_absent_from_reference_dict(reference_sub, target_sub)),
				);
			}
			_ => {
				result.insert(key.clone(), target_value.clone());
			}
		}
	}

	result
}

fn sort_object_recursively(map: Map<String, Value>) -> Map<String, Value> {
	let mut entries: Vec<(String, Value)> = map.into_iter().collect();

	entries.sort_by(|(a, _), (b, _)| natural_key_cmp(a, b));

	let mut result = Map::new();

	for (key, value) in entries {
		result.insert(key, sort_value_recursively(value));
	}

	result
}

fn sort_value_recursively(value: Value) -> Value {
	match value {
		Value::Object(map) => Value::Object(sort_object_recursively(map)),
		Value::Array(items) => Value::Array(items.into_iter().map(sort_value_recursively).collect()),
		other => other,
	}
}

fn serialize_pretty(value: &Map<String, Value>, indent: &[u8]) -> Result<String, serde_json::Error> {
	let formatter = serde_json::ser::PrettyFormatter::with_indent(indent);
	let mut buffer = Vec::new();
	let mut serializer = serde_json::Serializer::with_formatter(&mut buffer, formatter);

	value.serialize(&mut serializer)?;

	Ok(String::from_utf8(buffer).expect("serde_json always produces valid UTF-8"))
}

#[cfg(test)]
mod tests {
	use super::*;
	use tempfile::TempDir;
	use tokio::fs;

	fn parse(json: &str) -> Map<String, Value> {
		serde_json::from_str(json).unwrap()
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

	#[tokio::test]
	async fn writes_with_supplied_indentation_and_trailing_newline() {
		let dir = TempDir::new().unwrap();
		let definition = definition_in(&dir, "pt");
		let reference = parse(r#"{"a": "x"}"#);
		let initial = reference.clone();
		let translations = vec![vec![("a".to_string(), "y".to_string())]];

		write_i18n_tree_map(&definition, b"\t", &reference, initial, &translations)
			.await
			.unwrap();

		let written = fs::read_to_string(&definition.file).await.unwrap();

		assert_eq!(written, "{\n\t\"a\": \"y\"\n}\n");
	}

	#[tokio::test]
	async fn writes_with_two_space_indent_when_requested() {
		let dir = TempDir::new().unwrap();
		let definition = definition_in(&dir, "pt");
		let reference = parse(r#"{"a": "x"}"#);
		let initial = reference.clone();

		write_i18n_tree_map(&definition, b"  ", &reference, initial, &[]).await.unwrap();

		let written = fs::read_to_string(&definition.file).await.unwrap();

		assert_eq!(written, "{\n  \"a\": \"x\"\n}\n");
	}

	#[tokio::test]
	async fn applies_dot_separated_keys_into_nested_objects() {
		let dir = TempDir::new().unwrap();
		let definition = definition_in(&dir, "pt");
		let reference = parse(r#"{"section": {"a": "X", "b": "Y"}}"#);
		let initial = reference.clone();
		let translations = vec![vec![
			("section.a".to_string(), "AA".to_string()),
			("section.b".to_string(), "BB".to_string()),
		]];

		write_i18n_tree_map(&definition, b"\t", &reference, initial, &translations)
			.await
			.unwrap();

		let written = fs::read_to_string(&definition.file).await.unwrap();
		let parsed: Map<String, Value> = serde_json::from_str(&written).unwrap();

		assert_eq!(parsed["section"]["a"], "AA");
		assert_eq!(parsed["section"]["b"], "BB");
	}

	#[tokio::test]
	async fn prunes_keys_absent_from_reference_dict() {
		let dir = TempDir::new().unwrap();
		let definition = definition_in(&dir, "pt");
		let reference = parse(r#"{"keep": "K"}"#);
		let initial = parse(r#"{"keep": "K", "stale": "S"}"#);

		write_i18n_tree_map(&definition, b"\t", &reference, initial, &[]).await.unwrap();

		let written = fs::read_to_string(&definition.file).await.unwrap();
		let parsed: Map<String, Value> = serde_json::from_str(&written).unwrap();

		assert!(parsed.contains_key("keep"));
		assert!(!parsed.contains_key("stale"));
	}

	#[tokio::test]
	async fn prunes_nested_keys_absent_from_reference_dict() {
		let dir = TempDir::new().unwrap();
		let definition = definition_in(&dir, "pt");
		let reference = parse(r#"{"section": {"keep": "K"}}"#);
		let initial = parse(r#"{"section": {"keep": "K", "stale": "S"}}"#);

		write_i18n_tree_map(&definition, b"\t", &reference, initial, &[]).await.unwrap();

		let written = fs::read_to_string(&definition.file).await.unwrap();
		let parsed: Map<String, Value> = serde_json::from_str(&written).unwrap();

		assert!(parsed["section"].as_object().unwrap().contains_key("keep"));
		assert!(!parsed["section"].as_object().unwrap().contains_key("stale"));
	}

	#[tokio::test]
	async fn sorts_top_level_keys_alphabetically() {
		let dir = TempDir::new().unwrap();
		let definition = definition_in(&dir, "pt");
		let reference = parse(r#"{"zebra": "Z", "apple": "A", "mango": "M"}"#);
		let initial = reference.clone();

		write_i18n_tree_map(&definition, b"\t", &reference, initial, &[]).await.unwrap();

		let written = fs::read_to_string(&definition.file).await.unwrap();
		let apple_pos = written.find("\"apple\"").unwrap();
		let mango_pos = written.find("\"mango\"").unwrap();
		let zebra_pos = written.find("\"zebra\"").unwrap();

		assert!(apple_pos < mango_pos);
		assert!(mango_pos < zebra_pos);
	}

	#[tokio::test]
	async fn sorts_numeric_keys_in_natural_order() {
		let dir = TempDir::new().unwrap();
		let definition = definition_in(&dir, "pt");
		let reference = parse(r#"{"0":"a","1":"b","10":"c","11":"d","2":"e"}"#);
		let initial = reference.clone();

		write_i18n_tree_map(&definition, b"\t", &reference, initial, &[]).await.unwrap();

		let written = fs::read_to_string(&definition.file).await.unwrap();
		let positions: Vec<usize> = ["\"0\"", "\"1\"", "\"2\"", "\"10\"", "\"11\""]
			.iter()
			.map(|key| written.find(key).unwrap())
			.collect();

		for window in positions.windows(2) {
			assert!(window[0] < window[1], "natural order violated: {:?}", positions);
		}
	}

	#[tokio::test]
	async fn sorts_camel_case_keys_with_uppercase_before_lowercase() {
		let dir = TempDir::new().unwrap();
		let definition = definition_in(&dir, "pt");
		let reference = parse(r#"{"types": "T", "typeSelection": "S"}"#);
		let initial = reference.clone();

		write_i18n_tree_map(&definition, b"\t", &reference, initial, &[]).await.unwrap();

		let written = fs::read_to_string(&definition.file).await.unwrap();
		let type_selection_pos = written.find("\"typeSelection\"").unwrap();
		let types_pos = written.find("\"types\"").unwrap();

		assert!(type_selection_pos < types_pos, "expected typeSelection before types");
	}

	#[tokio::test]
	async fn sorts_nested_keys_recursively() {
		let dir = TempDir::new().unwrap();
		let definition = definition_in(&dir, "pt");
		let reference = parse(r#"{"section": {"zebra": "Z", "apple": "A"}}"#);
		let initial = reference.clone();

		write_i18n_tree_map(&definition, b"\t", &reference, initial, &[]).await.unwrap();

		let written = fs::read_to_string(&definition.file).await.unwrap();
		let apple_pos = written.find("\"apple\"").unwrap();
		let zebra_pos = written.find("\"zebra\"").unwrap();

		assert!(apple_pos < zebra_pos);
	}

	#[tokio::test]
	async fn creates_parent_directories_when_missing() {
		let dir = TempDir::new().unwrap();
		let nested_dir = dir.path().join("does/not/exist");
		let definition = I18nDefinition {
			file: nested_dir.join("pt.json"),
			id: "pt.json".to_string(),
			language: "pt".to_string(),
		};
		let reference = parse(r#"{"a": "x"}"#);
		let initial = reference.clone();

		write_i18n_tree_map(&definition, b"\t", &reference, initial, &[]).await.unwrap();

		assert!(definition.file.exists());
	}
}
