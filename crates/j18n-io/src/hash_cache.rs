use crate::compare::natural_key_cmp;
use crate::hashing::I18nHashing;
use j18n_core::{J18nError, J18nResult};
use serde::Serialize;
use serde_json::{Map, Value};
use std::collections::HashMap;
use std::path::Path;
use tokio::fs;
use tokio::io::AsyncWriteExt;

#[derive(Clone, Debug, Default)]
pub struct I18nHashingCache {
	by_target: HashMap<String, I18nHashing>,
}

impl I18nHashingCache {
	pub fn empty() -> Self {
		Self::default()
	}

	pub fn from_flat(by_target: HashMap<String, I18nHashing>) -> Self {
		Self { by_target }
	}

	pub fn get(&self, target_id: &str) -> Option<&I18nHashing> {
		self.by_target.get(target_id)
	}

	pub fn set(&mut self, target_id: impl Into<String>, hashing: I18nHashing) {
		self.by_target.insert(target_id.into(), hashing);
	}

	pub fn target_ids(&self) -> impl Iterator<Item = &str> {
		self.by_target.keys().map(|s| s.as_str())
	}

	pub fn len(&self) -> usize {
		self.by_target.len()
	}

	pub fn is_empty(&self) -> bool {
		self.by_target.is_empty()
	}

	pub async fn load_from(path: &Path) -> J18nResult<Self> {
		if !fs::try_exists(path).await.map_err(|source| J18nError::Io {
			path: path.to_path_buf(),
			source,
		})? {
			return Ok(Self::empty());
		}

		let raw_bytes = fs::read(path).await.map_err(|source| J18nError::Io {
			path: path.to_path_buf(),
			source,
		})?;
		let parsed: HashMap<String, HashMap<String, String>> =
			serde_json::from_slice(&raw_bytes).map_err(|source| J18nError::Json {
				path: path.to_path_buf(),
				source,
			})?;
		let by_target = parsed
			.into_iter()
			.map(|(target_id, json_key_to_hash_map)| (target_id, I18nHashing { json_key_to_hash_map }))
			.collect();

		Ok(Self { by_target })
	}

	pub async fn save_to(&self, path: &Path) -> J18nResult<()> {
		let mut sorted_target_ids: Vec<&String> = self.by_target.keys().collect();

		sorted_target_ids.sort_by(|a, b| natural_key_cmp(a, b));

		let mut top = Map::new();

		for target_id in sorted_target_ids {
			let hashing = &self.by_target[target_id];
			let mut sorted_keys: Vec<&String> = hashing.json_key_to_hash_map.keys().collect();

			sorted_keys.sort_by(|a, b| natural_key_cmp(a, b));

			let mut inner = Map::new();

			for key in sorted_keys {
				inner.insert(key.clone(), Value::String(hashing.json_key_to_hash_map[key].clone()));
			}

			top.insert(target_id.clone(), Value::Object(inner));
		}

		let serialized = serialize_pretty(&top).map_err(|source| J18nError::Json {
			path: path.to_path_buf(),
			source,
		})?;

		if let Some(parent) = path.parent() {
			fs::create_dir_all(parent).await.map_err(|source| J18nError::Io {
				path: parent.to_path_buf(),
				source,
			})?;
		}

		let mut file = fs::File::create(path).await.map_err(|source| J18nError::Io {
			path: path.to_path_buf(),
			source,
		})?;

		file.write_all(serialized.as_bytes())
			.await
			.map_err(|source| J18nError::Io {
				path: path.to_path_buf(),
				source,
			})?;
		file.write_all(b"\n").await.map_err(|source| J18nError::Io {
			path: path.to_path_buf(),
			source,
		})?;

		Ok(())
	}
}

fn serialize_pretty(value: &Map<String, Value>) -> Result<String, serde_json::Error> {
	let formatter = serde_json::ser::PrettyFormatter::with_indent(b"\t");
	let mut buffer = Vec::new();
	let mut serializer = serde_json::Serializer::with_formatter(&mut buffer, formatter);

	value.serialize(&mut serializer)?;

	Ok(String::from_utf8(buffer).expect("serde_json always produces valid UTF-8"))
}

#[cfg(test)]
mod tests {
	use super::*;
	use crate::java_string_hashcode_hex;
	use tempfile::TempDir;
	use tokio::fs;

	fn hashing_with(entries: &[(&str, &str)]) -> I18nHashing {
		let mut map = HashMap::new();

		for (key, value) in entries {
			map.insert(key.to_string(), value.to_string());
		}

		I18nHashing {
			json_key_to_hash_map: map,
		}
	}

	#[tokio::test]
	async fn load_returns_empty_when_file_is_missing() {
		let dir = TempDir::new().unwrap();
		let path = dir.path().join(".hash-cache.json");

		let loaded = I18nHashingCache::load_from(&path).await.unwrap();

		assert!(loaded.is_empty());
	}

	#[tokio::test]
	async fn save_then_load_round_trips_multiple_targets() {
		let dir = TempDir::new().unwrap();
		let path = dir.path().join(".hash-cache.json");

		let mut cache = I18nHashingCache::empty();

		cache.set("locales/pt.json@Portuguese", hashing_with(&[("a", "1"), ("b", "2")]));
		cache.set("locales/es.json@Spanish", hashing_with(&[("a", "1")]));

		cache.save_to(&path).await.unwrap();

		let loaded = I18nHashingCache::load_from(&path).await.unwrap();

		assert_eq!(loaded.len(), 2);
		let pt = loaded.get("locales/pt.json@Portuguese").unwrap();
		assert_eq!(pt.json_key_to_hash_map.get("a"), Some(&"1".to_string()));
		assert_eq!(pt.json_key_to_hash_map.get("b"), Some(&"2".to_string()));
		let es = loaded.get("locales/es.json@Spanish").unwrap();
		assert_eq!(es.json_key_to_hash_map.get("a"), Some(&"1".to_string()));
	}

	#[tokio::test]
	async fn save_uses_tab_indentation_and_trailing_newline() {
		let dir = TempDir::new().unwrap();
		let path = dir.path().join(".hash-cache.json");

		let mut cache = I18nHashingCache::empty();

		cache.set("pt", hashing_with(&[("a", "1")]));

		cache.save_to(&path).await.unwrap();

		let written = fs::read_to_string(&path).await.unwrap();

		assert_eq!(written, "{\n\t\"pt\": {\n\t\t\"a\": \"1\"\n\t}\n}\n");
	}

	#[tokio::test]
	async fn save_writes_target_ids_and_inner_keys_in_sorted_order() {
		let dir = TempDir::new().unwrap();
		let path = dir.path().join(".hash-cache.json");

		let mut cache = I18nHashingCache::empty();

		cache.set("z@Zulu", hashing_with(&[("c", "3"), ("a", "1"), ("b", "2")]));
		cache.set("a@Aymara", hashing_with(&[("y", "y"), ("x", "x")]));

		cache.save_to(&path).await.unwrap();

		let written = fs::read_to_string(&path).await.unwrap();
		let a_position = written.find("a@Aymara").unwrap();
		let z_position = written.find("z@Zulu").unwrap();

		assert!(a_position < z_position);

		let inner_x = written.find("\"x\"").unwrap();
		let inner_y = written.find("\"y\"").unwrap();
		let inner_a = written.find("\"a\"").unwrap();
		let inner_b = written.find("\"b\"").unwrap();
		let inner_c = written.find("\"c\"").unwrap();

		assert!(inner_x < inner_y);
		assert!(inner_a < inner_b);
		assert!(inner_b < inner_c);
	}

	#[tokio::test]
	async fn load_errors_on_old_flat_format() {
		let dir = TempDir::new().unwrap();
		let path = dir.path().join(".hash-cache.json");

		fs::write(&path, r#"{"key": "hash"}"#).await.unwrap();

		assert!(I18nHashingCache::load_from(&path).await.is_err());
	}

	#[test]
	fn from_i18n_data_uses_java_string_hashcode() {
		let data = j18n_core::I18nData {
			json_dict: Default::default(),
			walked_tree_map: vec![("greeting".into(), "abc".into())],
		};
		let hashing = I18nHashing::from_i18n_data(&data);

		assert_eq!(
			hashing.json_key_to_hash_map.get("greeting").unwrap(),
			&java_string_hashcode_hex("abc")
		);
	}

	#[test]
	fn get_returns_none_for_unknown_target_id() {
		let cache = I18nHashingCache::empty();

		assert!(cache.get("unknown").is_none());
	}

	#[test]
	fn set_replaces_existing_entry() {
		let mut cache = I18nHashingCache::empty();

		cache.set("pt", hashing_with(&[("a", "1")]));
		cache.set("pt", hashing_with(&[("a", "2"), ("b", "3")]));

		let pt = cache.get("pt").unwrap();

		assert_eq!(pt.json_key_to_hash_map.len(), 2);
		assert_eq!(pt.json_key_to_hash_map.get("a"), Some(&"2".to_string()));
	}

	#[test]
	fn target_ids_returns_all_ids() {
		let mut cache = I18nHashingCache::empty();

		cache.set("pt", I18nHashing::empty());
		cache.set("es", I18nHashing::empty());

		let mut ids: Vec<String> = cache.target_ids().map(String::from).collect();

		ids.sort();
		assert_eq!(ids, vec!["es".to_string(), "pt".to_string()]);
	}
}
