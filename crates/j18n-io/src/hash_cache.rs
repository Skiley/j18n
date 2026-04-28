use crate::hashing::{java_string_hashcode_hex, I18nHashing};
use crate::json_walker::walk_json_tree_to_map;
use j18n_core::{I18nData, J18nError, J18nResult};
use serde::Serialize;
use serde_json::{Map, Value};
use std::collections::HashMap;
use std::path::Path;
use tokio::fs;
use tokio::io::AsyncWriteExt;

pub struct I18nHashingCache;

impl I18nHashingCache {
	pub fn compute_hash_cache_from(i18n_data: &I18nData) -> I18nHashing {
		let json_key_to_hash_map = i18n_data
			.walked_tree_map
			.iter()
			.map(|(key, value)| (key.clone(), java_string_hashcode_hex(value)))
			.collect();

		I18nHashing { json_key_to_hash_map }
	}

	pub async fn load_hash_cache_from(path: &Path) -> J18nResult<I18nHashing> {
		if !fs::try_exists(path).await.map_err(|source| J18nError::Io {
			path: path.to_path_buf(),
			source,
		})? {
			return Ok(I18nHashing::default());
		}

		let raw_bytes = fs::read(path).await.map_err(|source| J18nError::Io {
			path: path.to_path_buf(),
			source,
		})?;
		let json_dict: Map<String, Value> = serde_json::from_slice(&raw_bytes).map_err(|source| J18nError::Json {
			path: path.to_path_buf(),
			source,
		})?;
		let walked = walk_json_tree_to_map(&json_dict);
		let json_key_to_hash_map: HashMap<String, String> = walked.into_iter().collect();

		Ok(I18nHashing { json_key_to_hash_map })
	}

	pub async fn save_hash_cache_to(hashing: &I18nHashing, path: &Path) -> J18nResult<()> {
		let mut sorted: Vec<(&String, &String)> = hashing.json_key_to_hash_map.iter().collect();

		sorted.sort_by(|a, b| a.0.cmp(b.0));

		let mut ordered = Map::new();

		for (key, value) in sorted {
			ordered.insert(key.clone(), Value::String(value.clone()));
		}

		let serialized = serialize_pretty(&ordered).map_err(|source| J18nError::Json {
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
	use tempfile::TempDir;
	use tokio::fs;

	#[tokio::test]
	async fn load_returns_empty_when_file_is_missing() {
		let dir = TempDir::new().unwrap();
		let path = dir.path().join(".hash-cache.json");

		let loaded = I18nHashingCache::load_hash_cache_from(&path).await.unwrap();

		assert!(loaded.json_key_to_hash_map.is_empty());
	}

	#[tokio::test]
	async fn save_then_load_round_trip() {
		let dir = TempDir::new().unwrap();
		let path = dir.path().join(".hash-cache.json");
		let mut original = HashMap::new();

		original.insert("foo".to_string(), "abc".to_string());
		original.insert("nested.key".to_string(), "def".to_string());

		let hashing = I18nHashing {
			json_key_to_hash_map: original.clone(),
		};

		I18nHashingCache::save_hash_cache_to(&hashing, &path).await.unwrap();

		let loaded = I18nHashingCache::load_hash_cache_from(&path).await.unwrap();

		assert_eq!(loaded.json_key_to_hash_map, original);
	}

	#[tokio::test]
	async fn save_uses_tab_indentation_and_trailing_newline() {
		let dir = TempDir::new().unwrap();
		let path = dir.path().join(".hash-cache.json");
		let mut map = HashMap::new();

		map.insert("a".to_string(), "1".to_string());

		let hashing = I18nHashing { json_key_to_hash_map: map };

		I18nHashingCache::save_hash_cache_to(&hashing, &path).await.unwrap();

		let written = fs::read_to_string(&path).await.unwrap();

		assert_eq!(written, "{\n\t\"a\": \"1\"\n}\n");
	}

	#[tokio::test]
	async fn save_writes_keys_in_sorted_order() {
		let dir = TempDir::new().unwrap();
		let path = dir.path().join(".hash-cache.json");
		let mut map = HashMap::new();

		map.insert("b".to_string(), "2".to_string());
		map.insert("a".to_string(), "1".to_string());
		map.insert("c".to_string(), "3".to_string());

		let hashing = I18nHashing { json_key_to_hash_map: map };

		I18nHashingCache::save_hash_cache_to(&hashing, &path).await.unwrap();

		let written = fs::read_to_string(&path).await.unwrap();
		let a_pos = written.find("\"a\"").unwrap();
		let b_pos = written.find("\"b\"").unwrap();
		let c_pos = written.find("\"c\"").unwrap();

		assert!(a_pos < b_pos);
		assert!(b_pos < c_pos);
	}

	#[test]
	fn compute_hash_cache_uses_java_string_hashcode() {
		let data = j18n_core::I18nData {
			json_dict: Default::default(),
			walked_tree_map: vec![("greeting".into(), "abc".into())],
		};
		let hashing = I18nHashingCache::compute_hash_cache_from(&data);

		assert_eq!(hashing.json_key_to_hash_map.get("greeting").unwrap(), "17862");
	}
}
