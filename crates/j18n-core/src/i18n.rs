use serde_json::{Map, Value};
use std::path::PathBuf;

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct I18nDefinition {
	pub file: PathBuf,
	pub id: String,
	pub language: String,
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

	#[test]
	fn i18n_definition_keeps_id_separate_from_resolved_path() {
		let definition = I18nDefinition {
			file: PathBuf::from("/abs/locales/pt.json"),
			id: "locales/pt.json".to_string(),
			language: "Brazilian Portuguese".to_string(),
		};

		assert_eq!(definition.file, PathBuf::from("/abs/locales/pt.json"));
		assert_eq!(definition.id, "locales/pt.json");
		assert_eq!(definition.language, "Brazilian Portuguese");
	}
}
