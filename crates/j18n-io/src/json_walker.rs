use serde_json::{Map, Value};

pub fn walk_json_tree_to_map(json: &Map<String, Value>) -> Vec<(String, String)> {
	let mut output = Vec::new();

	walk(json, "", &mut output);

	output
}

fn walk(json: &Map<String, Value>, key_prefix: &str, output: &mut Vec<(String, String)>) {
	for (key, value) in json {
		match value {
			Value::String(string_value) => {
				output.push((format!("{key_prefix}{key}"), string_value.clone()));
			}
			Value::Object(object_value) => {
				let new_prefix = format!("{key_prefix}{key}.");

				walk(object_value, &new_prefix, output);
			}
			_ => {}
		}
	}
}

#[cfg(test)]
mod tests {
	use super::*;

	fn parse(json: &str) -> Map<String, Value> {
		serde_json::from_str(json).unwrap()
	}

	#[test]
	fn walks_flat_string_object() {
		let json = parse(r#"{"a": "1", "b": "2"}"#);

		assert_eq!(
			walk_json_tree_to_map(&json),
			vec![("a".into(), "1".into()), ("b".into(), "2".into())]
		);
	}

	#[test]
	fn walks_nested_objects_with_dot_separated_keys() {
		let json = parse(r#"{"a": {"b": "1", "c": {"d": "2"}}}"#);

		assert_eq!(
			walk_json_tree_to_map(&json),
			vec![("a.b".into(), "1".into()), ("a.c.d".into(), "2".into())]
		);
	}

	#[test]
	fn skips_non_string_primitives() {
		let json = parse(r#"{"n": 1, "b": true, "x": null, "s": "ok"}"#);

		assert_eq!(walk_json_tree_to_map(&json), vec![("s".into(), "ok".into())]);
	}

	#[test]
	fn preserves_insertion_order() {
		let json = parse(r#"{"z": "1", "m": "2", "a": "3"}"#);

		assert_eq!(
			walk_json_tree_to_map(&json),
			vec![
				("z".into(), "1".into()),
				("m".into(), "2".into()),
				("a".into(), "3".into()),
			]
		);
	}

	#[test]
	fn walks_empty_object() {
		let json = parse("{}");

		assert!(walk_json_tree_to_map(&json).is_empty());
	}
}
