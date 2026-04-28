use crate::language::Language;
use serde_json::{Map, Value};
use std::path::{Path, PathBuf};

#[derive(Clone, Debug)]
pub struct I18nDefinition {
	pub json_file_path: PathBuf,
	pub language: Language,
}

impl I18nDefinition {
	pub fn from_base_dir(base_dir: impl AsRef<Path>, language: Language) -> Self {
		let json_file_path = base_dir.as_ref().join(format!("{}.json", language.iso_639_code()));

		Self {
			json_file_path,
			language,
		}
	}
}

#[derive(Clone, Debug, Default)]
pub struct I18nData {
	pub json_dict: Map<String, Value>,
	pub walked_tree_map: Vec<(String, String)>,
}

impl I18nData {
	pub fn empty() -> Self {
		Self::default()
	}

	pub fn walked_tree_keys(&self) -> impl Iterator<Item = &str> {
		self.walked_tree_map.iter().map(|(key, _)| key.as_str())
	}
}

#[cfg(test)]
mod tests {
	use super::*;

	#[test]
	fn from_base_dir_joins_iso_code_with_json_extension() {
		let definition = I18nDefinition::from_base_dir("locales", Language::ENGLISH);

		assert_eq!(definition.json_file_path, PathBuf::from("locales").join("en.json"));
		assert_eq!(definition.language, Language::ENGLISH);
	}

	#[test]
	fn from_base_dir_handles_iso_codes_with_dash() {
		let zh_cn = Language::from_iso_639_code("zh-CN").unwrap();
		let definition = I18nDefinition::from_base_dir("locales", zh_cn);

		assert_eq!(definition.json_file_path, PathBuf::from("locales").join("zh-CN.json"));
	}

	#[test]
	fn empty_i18n_data_has_no_entries() {
		let data = I18nData::empty();

		assert!(data.json_dict.is_empty());
		assert!(data.walked_tree_map.is_empty());
		assert_eq!(data.walked_tree_keys().count(), 0);
	}

	#[test]
	fn walked_tree_keys_iterates_keys_in_order() {
		let data = I18nData {
			json_dict: Default::default(),
			walked_tree_map: vec![("a".into(), "1".into()), ("b.c".into(), "2".into())],
		};
		let keys: Vec<&str> = data.walked_tree_keys().collect();

		assert_eq!(keys, vec!["a", "b.c"]);
	}
}
