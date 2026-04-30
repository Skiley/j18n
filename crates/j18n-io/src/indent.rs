use j18n_core::{J18nError, J18nResult};
use std::path::Path;
use tokio::fs;

pub const DEFAULT_INDENT: &str = "\t";

pub async fn detect_indentation(path: &Path) -> J18nResult<Option<String>> {
	if !fs::try_exists(path).await.map_err(|source| J18nError::Io {
		path: path.to_path_buf(),
		source,
	})? {
		return Ok(None);
	}

	let content = fs::read_to_string(path).await.map_err(|source| J18nError::Io {
		path: path.to_path_buf(),
		source,
	})?;

	Ok(detect_indentation_unit(&content))
}

pub fn detect_indentation_unit(content: &str) -> Option<String> {
	for line in content.lines() {
		let mut chars = line.chars();
		let first = chars.next()?;

		if first != ' ' && first != '\t' {
			continue;
		}

		let mut indent = String::new();

		indent.push(first);
		indent.extend(chars.take_while(|c| *c == ' ' || *c == '\t'));

		return Some(indent);
	}

	None
}

#[cfg(test)]
mod tests {
	use super::*;
	use tempfile::TempDir;

	#[test]
	fn detects_tab_indentation() {
		let content = "{\n\t\"a\": \"x\"\n}\n";

		assert_eq!(detect_indentation_unit(content), Some("\t".to_string()));
	}

	#[test]
	fn detects_two_space_indentation() {
		let content = "{\n  \"a\": \"x\"\n}\n";

		assert_eq!(detect_indentation_unit(content), Some("  ".to_string()));
	}

	#[test]
	fn detects_four_space_indentation() {
		let content = "{\n    \"a\": \"x\"\n}\n";

		assert_eq!(detect_indentation_unit(content), Some("    ".to_string()));
	}

	#[test]
	fn returns_none_when_no_indentation_is_present() {
		let content = "{\"a\":\"x\"}";

		assert_eq!(detect_indentation_unit(content), None);
	}

	#[test]
	fn returns_none_for_empty_content() {
		assert_eq!(detect_indentation_unit(""), None);
	}

	#[tokio::test]
	async fn detect_indentation_returns_none_for_missing_file() {
		let dir = TempDir::new().unwrap();
		let path = dir.path().join("missing.json");

		assert!(detect_indentation(&path).await.unwrap().is_none());
	}

	#[tokio::test]
	async fn detect_indentation_reads_file_indent() {
		let dir = TempDir::new().unwrap();
		let path = dir.path().join("present.json");

		tokio::fs::write(&path, "{\n  \"a\": \"x\"\n}\n").await.unwrap();

		assert_eq!(detect_indentation(&path).await.unwrap(), Some("  ".to_string()));
	}
}
