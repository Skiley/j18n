use j18n_core::I18nData;
use std::collections::{BTreeSet, HashMap};

#[derive(Clone, Debug, Default)]
pub struct I18nHashing {
	pub json_key_to_hash_map: HashMap<String, String>,
}

impl I18nHashing {
	pub fn empty() -> Self {
		Self::default()
	}

	pub fn from_i18n_data(data: &I18nData) -> Self {
		let json_key_to_hash_map = data
			.walked_tree_map
			.iter()
			.map(|(key, value)| (key.clone(), java_string_hashcode_hex(value)))
			.collect();

		Self { json_key_to_hash_map }
	}

	pub fn compute_changed_keys(&self, compare_with: &I18nHashing) -> BTreeSet<String> {
		let mut changed_keys = BTreeSet::new();
		let mut all_keys = BTreeSet::new();

		all_keys.extend(self.json_key_to_hash_map.keys().cloned());
		all_keys.extend(compare_with.json_key_to_hash_map.keys().cloned());

		for key in all_keys {
			let reference_value = self.json_key_to_hash_map.get(&key);
			let target_value = compare_with.json_key_to_hash_map.get(&key);

			if reference_value != target_value {
				changed_keys.insert(key);
			}
		}

		changed_keys
	}
}

pub fn java_string_hashcode_hex(value: &str) -> String {
	let mut hash: i32 = 0;
	let mut buffer = [0u16; 2];

	for character in value.chars() {
		let units = character.encode_utf16(&mut buffer);

		for unit in units.iter() {
			hash = hash.wrapping_mul(31).wrapping_add(*unit as i32);
		}
	}

	format_signed_hex(hash)
}

fn format_signed_hex(value: i32) -> String {
	let widened = value as i64;

	if widened < 0 {
		format!("-{:x}", -widened)
	} else {
		format!("{:x}", widened)
	}
}

#[cfg(test)]
mod tests {
	use super::*;

	#[test]
	fn empty_string_hashes_to_zero() {
		assert_eq!(java_string_hashcode_hex(""), "0");
	}

	#[test]
	fn matches_known_java_hashcodes() {
		assert_eq!(java_string_hashcode_hex("a"), "61");
		assert_eq!(java_string_hashcode_hex("ab"), "c21");
		assert_eq!(java_string_hashcode_hex("abc"), "17862");
	}

	#[test]
	fn produces_negative_hex_for_strings_that_overflow_i32() {
		let hash = java_string_hashcode_hex("Delete my account");

		assert!(hash.starts_with('-'), "expected negative hex, got {hash}");
	}

	#[test]
	fn matches_kotlin_hash_cache_values_for_real_strings() {
		assert_eq!(java_string_hashcode_hex("This account no longer exists."), "10f79085");
		assert_eq!(java_string_hashcode_hex("Delete my account"), "-8a0ced2");
	}

	#[test]
	fn handles_unicode_via_utf16_units() {
		let hash = java_string_hashcode_hex("héllo");

		assert!(!hash.is_empty());
		assert_eq!(hash, java_string_hashcode_hex("héllo"));
	}

	#[test]
	fn compute_changed_keys_finds_added_removed_and_modified() {
		let mut a = HashMap::new();

		a.insert("kept_same".to_string(), "abc".to_string());
		a.insert("modified".to_string(), "old".to_string());
		a.insert("only_in_a".to_string(), "x".to_string());

		let mut b = HashMap::new();

		b.insert("kept_same".to_string(), "abc".to_string());
		b.insert("modified".to_string(), "new".to_string());
		b.insert("only_in_b".to_string(), "y".to_string());

		let hashing_a = I18nHashing {
			json_key_to_hash_map: a,
		};
		let hashing_b = I18nHashing {
			json_key_to_hash_map: b,
		};
		let changed = hashing_a.compute_changed_keys(&hashing_b);

		assert!(changed.contains("modified"));
		assert!(changed.contains("only_in_a"));
		assert!(changed.contains("only_in_b"));
		assert!(!changed.contains("kept_same"));
	}

	#[test]
	fn compute_changed_keys_returns_empty_when_identical() {
		let mut map = HashMap::new();

		map.insert("a".to_string(), "1".to_string());

		let hashing_a = I18nHashing {
			json_key_to_hash_map: map.clone(),
		};
		let hashing_b = I18nHashing {
			json_key_to_hash_map: map,
		};

		assert!(hashing_a.compute_changed_keys(&hashing_b).is_empty());
	}
}
