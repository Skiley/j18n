use anyhow::{Context, Result};
use serde::Deserialize;
use std::path::{Path, PathBuf};

#[derive(Debug, Deserialize)]
pub struct I18nToolConfig {
	#[serde(rename = "baseDirectory")]
	pub base_directory: PathBuf,

	#[serde(rename = "generateI18nFor")]
	pub generate_i18n_for: Vec<String>,

	#[serde(rename = "referenceI18n")]
	pub reference_i18n: String,

	pub translator: TranslatorKind,
}

#[derive(Clone, Copy, Debug, Deserialize, Eq, Hash, PartialEq)]
#[serde(rename_all = "kebab-case")]
pub enum TranslatorKind {
	ClaudeCode,
	GeminiApi,
}

pub fn load_config(path: &Path) -> Result<I18nToolConfig> {
	let raw = std::fs::read(path).with_context(|| format!("failed to read config file \"{}\"", path.display()))?;
	let config: I18nToolConfig =
		serde_json::from_slice(&raw).with_context(|| format!("failed to parse config file \"{}\"", path.display()))?;

	Ok(config)
}

#[cfg(test)]
mod tests {
	use super::*;
	use tempfile::TempDir;

	fn write_config(dir: &TempDir, name: &str, contents: &str) -> std::path::PathBuf {
		let path = dir.path().join(name);

		std::fs::write(&path, contents).unwrap();
		path
	}

	#[test]
	fn parses_minimal_valid_config() {
		let dir = TempDir::new().unwrap();
		let path = write_config(
			&dir,
			"a.json",
			r#"{
				"baseDirectory": "./locales",
				"referenceI18n": "en",
				"generateI18nFor": ["pt", "es"],
				"translator": "claude-code"
			}"#,
		);

		let config = load_config(&path).unwrap();

		assert_eq!(config.base_directory, PathBuf::from("./locales"));
		assert_eq!(config.reference_i18n, "en");
		assert_eq!(config.generate_i18n_for, vec!["pt".to_string(), "es".to_string()]);
		assert!(matches!(config.translator, TranslatorKind::ClaudeCode));
	}

	#[test]
	fn parses_gemini_translator() {
		let dir = TempDir::new().unwrap();
		let path = write_config(
			&dir,
			"a.json",
			r#"{
				"baseDirectory": ".",
				"referenceI18n": "en",
				"generateI18nFor": [],
				"translator": "gemini-api"
			}"#,
		);

		let config = load_config(&path).unwrap();

		assert!(matches!(config.translator, TranslatorKind::GeminiApi));
	}

	#[test]
	fn rejects_unknown_translator_value() {
		let dir = TempDir::new().unwrap();
		let path = write_config(
			&dir,
			"a.json",
			r#"{
				"baseDirectory": ".",
				"referenceI18n": "en",
				"generateI18nFor": [],
				"translator": "openai"
			}"#,
		);

		let err = load_config(&path).unwrap_err();
		let err_text = format!("{err:#}");

		assert!(
			err_text.contains("claude-code") && err_text.contains("gemini-api"),
			"error should mention valid variants: {err_text}"
		);
	}

	#[test]
	fn rejects_missing_required_fields() {
		let dir = TempDir::new().unwrap();
		let path = write_config(&dir, "a.json", r#"{"baseDirectory": "."}"#);

		assert!(load_config(&path).is_err());
	}

	#[test]
	fn rejects_invalid_json() {
		let dir = TempDir::new().unwrap();
		let path = write_config(&dir, "a.json", "not json");

		assert!(load_config(&path).is_err());
	}

	#[test]
	fn rejects_missing_file() {
		let dir = TempDir::new().unwrap();
		let path = dir.path().join("does-not-exist.json");

		assert!(load_config(&path).is_err());
	}

	#[test]
	fn ignores_unknown_extra_fields() {
		let dir = TempDir::new().unwrap();
		let path = write_config(
			&dir,
			"a.json",
			r#"{
				"baseDirectory": "./locales",
				"referenceI18n": "en",
				"generateI18nFor": [],
				"translator": "claude-code",
				"mode": "SYNC",
				"some_other_field": 42
			}"#,
		);

		let config = load_config(&path).unwrap();

		assert_eq!(config.base_directory, PathBuf::from("./locales"));
	}
}
