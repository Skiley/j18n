use anyhow::{Context, Result};
use j18n_core::PathPattern;
use j18n_translator::compile_interpolation_patterns;
use serde::de::{self, Deserializer, Visitor};
use serde::Deserialize;
use std::fmt;
use std::path::{Path, PathBuf};

#[derive(Debug, Deserialize)]
pub struct DefinitionEntry {
	pub file: String,
	pub language: String,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub enum NamespacesConfig {
	List(Vec<String>),
	Wildcard,
}

impl<'de> Deserialize<'de> for NamespacesConfig {
	fn deserialize<D>(deserializer: D) -> Result<Self, D::Error>
	where
		D: Deserializer<'de>,
	{
		struct NamespacesVisitor;

		impl<'de> Visitor<'de> for NamespacesVisitor {
			type Value = NamespacesConfig;

			fn expecting(&self, formatter: &mut fmt::Formatter) -> fmt::Result {
				formatter.write_str("the string \"*\" or an array of namespace names")
			}

			fn visit_str<E>(self, value: &str) -> Result<Self::Value, E>
			where
				E: de::Error,
			{
				if value == "*" {
					Ok(NamespacesConfig::Wildcard)
				} else {
					Err(E::custom(format!(
						"expected \"*\" for wildcard namespace discovery, got \"{value}\""
					)))
				}
			}

			fn visit_string<E>(self, value: String) -> Result<Self::Value, E>
			where
				E: de::Error,
			{
				self.visit_str(&value)
			}

			fn visit_seq<A>(self, mut seq: A) -> Result<Self::Value, A::Error>
			where
				A: de::SeqAccess<'de>,
			{
				let mut names: Vec<String> = Vec::new();

				while let Some(name) = seq.next_element::<String>()? {
					names.push(name);
				}

				Ok(NamespacesConfig::List(names))
			}
		}

		deserializer.deserialize_any(NamespacesVisitor)
	}
}

#[derive(Debug, Deserialize)]
pub struct I18nToolConfig {
	#[serde(rename = "additionalPrompts")]
	pub additional_prompts: Vec<String>,

	#[serde(rename = "batchSize")]
	pub batch_size: usize,

	#[serde(rename = "excludePatterns")]
	pub exclude_patterns: Vec<String>,

	#[serde(rename = "generateI18nFor")]
	pub generate_i18n_for: Vec<DefinitionEntry>,

	#[serde(rename = "hashCacheLocation", default)]
	pub hash_cache_location: Option<PathBuf>,

	#[serde(rename = "interpolationPatterns")]
	pub interpolation_patterns: Vec<String>,

	#[serde(rename = "namespaces", default)]
	pub namespaces: Option<NamespacesConfig>,

	#[serde(rename = "parallelBatches")]
	pub parallel_batches: usize,

	#[serde(rename = "referenceI18n")]
	pub reference_i18n: DefinitionEntry,

	pub translator: TranslatorKind,
}

#[derive(Clone, Copy, Debug, Deserialize, Eq, Hash, PartialEq)]
#[serde(rename_all = "kebab-case")]
pub enum TranslatorKind {
	ClaudeCode,
	GeminiApi,
}

impl I18nToolConfig {
	pub fn compile_patterns(&self) -> Result<(Vec<PathPattern>, Vec<regex::Regex>)> {
		let exclude_patterns = self
			.exclude_patterns
			.iter()
			.map(|raw| PathPattern::parse(raw))
			.collect::<Result<Vec<_>, _>>()
			.context("invalid excludePatterns")?;
		let interpolation_patterns =
			compile_interpolation_patterns(&self.interpolation_patterns).context("invalid interpolationPatterns")?;

		Ok((exclude_patterns, interpolation_patterns))
	}

	pub fn validate_numbers(&self) -> Result<()> {
		if self.batch_size == 0 {
			anyhow::bail!("batchSize must be at least 1");
		}

		if self.parallel_batches == 0 {
			anyhow::bail!("parallelBatches must be at least 1");
		}

		Ok(())
	}
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

	fn full_config_json() -> &'static str {
		r#"{
			"additionalPrompts": ["context line"],
			"batchSize": 50,
			"excludePatterns": ["sample.**"],
			"generateI18nFor": [
				{ "file": "locales/pt.json", "language": "Brazilian Portuguese" },
				{ "file": "locales/es.json", "language": "Spanish" }
			],
			"interpolationPatterns": ["\\{\\{(.+?)\\}\\}"],
			"parallelBatches": 3,
			"referenceI18n": { "file": "locales/en.json", "language": "English" },
			"translator": "claude-code"
		}"#
	}

	#[test]
	fn parses_full_valid_config() {
		let dir = TempDir::new().unwrap();
		let path = write_config(&dir, "a.json", full_config_json());

		let config = load_config(&path).unwrap();

		assert_eq!(config.batch_size, 50);
		assert_eq!(config.parallel_batches, 3);
		assert_eq!(config.exclude_patterns, vec!["sample.**".to_string()]);
		assert_eq!(config.interpolation_patterns, vec!["\\{\\{(.+?)\\}\\}".to_string()]);
		assert_eq!(config.reference_i18n.file, "locales/en.json");
		assert_eq!(config.reference_i18n.language, "English");
		assert_eq!(config.generate_i18n_for.len(), 2);
		assert_eq!(config.generate_i18n_for[0].file, "locales/pt.json");
		assert_eq!(config.generate_i18n_for[0].language, "Brazilian Portuguese");
		assert!(matches!(config.translator, TranslatorKind::ClaudeCode));
		assert!(config.hash_cache_location.is_none());
	}

	#[test]
	fn parses_optional_hash_cache_location_when_present() {
		let dir = TempDir::new().unwrap();
		let path = write_config(
			&dir,
			"a.json",
			r#"{
				"additionalPrompts": [],
				"batchSize": 50,
				"excludePatterns": [],
				"generateI18nFor": [{ "file": "pt.json", "language": "Portuguese" }],
				"hashCacheLocation": "custom/.cache.json",
				"interpolationPatterns": [],
				"parallelBatches": 3,
				"referenceI18n": { "file": "en.json", "language": "English" },
				"translator": "claude-code"
			}"#,
		);

		let config = load_config(&path).unwrap();

		assert_eq!(config.hash_cache_location, Some(PathBuf::from("custom/.cache.json")));
	}

	#[test]
	fn compile_patterns_returns_compiled_lists() {
		let dir = TempDir::new().unwrap();
		let path = write_config(&dir, "a.json", full_config_json());

		let (exclude, interpolation) = load_config(&path).unwrap().compile_patterns().unwrap();

		assert_eq!(exclude.len(), 1);
		assert_eq!(interpolation.len(), 1);
	}

	#[test]
	fn compile_patterns_errors_on_invalid_exclude_pattern() {
		let dir = TempDir::new().unwrap();
		let path = write_config(
			&dir,
			"a.json",
			r#"{
				"additionalPrompts": [],
				"batchSize": 50,
				"excludePatterns": ["a..b"],
				"generateI18nFor": [],
				"interpolationPatterns": [],
				"parallelBatches": 3,
				"referenceI18n": { "file": "en.json", "language": "English" },
				"translator": "claude-code"
			}"#,
		);

		let err = load_config(&path).unwrap().compile_patterns().unwrap_err();

		assert!(format!("{err:#}").contains("excludePatterns"));
	}

	#[test]
	fn compile_patterns_errors_on_invalid_interpolation_regex() {
		let dir = TempDir::new().unwrap();
		let path = write_config(
			&dir,
			"a.json",
			r#"{
				"additionalPrompts": [],
				"batchSize": 50,
				"excludePatterns": [],
				"generateI18nFor": [],
				"interpolationPatterns": ["["],
				"parallelBatches": 3,
				"referenceI18n": { "file": "en.json", "language": "English" },
				"translator": "claude-code"
			}"#,
		);

		let err = load_config(&path).unwrap().compile_patterns().unwrap_err();

		assert!(format!("{err:#}").contains("interpolationPatterns"));
	}

	#[test]
	fn validate_numbers_errors_when_batch_size_is_zero() {
		let dir = TempDir::new().unwrap();
		let path = write_config(
			&dir,
			"a.json",
			r#"{
				"additionalPrompts": [],
				"batchSize": 0,
				"excludePatterns": [],
				"generateI18nFor": [],
				"interpolationPatterns": [],
				"parallelBatches": 3,
				"referenceI18n": { "file": "en.json", "language": "English" },
				"translator": "claude-code"
			}"#,
		);

		let err = load_config(&path).unwrap().validate_numbers().unwrap_err();

		assert!(format!("{err:#}").contains("batchSize"));
	}

	#[test]
	fn validate_numbers_errors_when_parallel_batches_is_zero() {
		let dir = TempDir::new().unwrap();
		let path = write_config(
			&dir,
			"a.json",
			r#"{
				"additionalPrompts": [],
				"batchSize": 50,
				"excludePatterns": [],
				"generateI18nFor": [],
				"interpolationPatterns": [],
				"parallelBatches": 0,
				"referenceI18n": { "file": "en.json", "language": "English" },
				"translator": "claude-code"
			}"#,
		);

		let err = load_config(&path).unwrap().validate_numbers().unwrap_err();

		assert!(format!("{err:#}").contains("parallelBatches"));
	}

	#[test]
	fn parses_gemini_translator() {
		let dir = TempDir::new().unwrap();
		let path = write_config(
			&dir,
			"a.json",
			r#"{
				"additionalPrompts": [],
				"batchSize": 50,
				"excludePatterns": [],
				"generateI18nFor": [],
				"interpolationPatterns": [],
				"parallelBatches": 3,
				"referenceI18n": { "file": "en.json", "language": "English" },
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
				"additionalPrompts": [],
				"batchSize": 50,
				"excludePatterns": [],
				"generateI18nFor": [],
				"interpolationPatterns": [],
				"parallelBatches": 3,
				"referenceI18n": { "file": "en.json", "language": "English" },
				"translator": "openai"
			}"#,
		);

		let err = load_config(&path).unwrap_err();
		let text = format!("{err:#}");

		assert!(text.contains("claude-code") && text.contains("gemini-api"));
	}

	#[test]
	fn rejects_missing_required_fields() {
		let dir = TempDir::new().unwrap();
		let path = write_config(&dir, "a.json", r#"{"batchSize": 50}"#);

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
				"additionalPrompts": [],
				"batchSize": 50,
				"excludePatterns": [],
				"generateI18nFor": [],
				"interpolationPatterns": [],
				"parallelBatches": 3,
				"referenceI18n": { "file": "en.json", "language": "English" },
				"translator": "claude-code",
				"mode": "SYNC",
				"some_other_field": 42
			}"#,
		);

		let config = load_config(&path).unwrap();

		assert_eq!(config.batch_size, 50);
	}

	#[test]
	fn namespaces_field_defaults_to_none_when_omitted() {
		let dir = TempDir::new().unwrap();
		let path = write_config(&dir, "a.json", full_config_json());

		let config = load_config(&path).unwrap();

		assert!(config.namespaces.is_none());
	}

	#[test]
	fn namespaces_field_parses_explicit_list() {
		let dir = TempDir::new().unwrap();
		let path = write_config(
			&dir,
			"a.json",
			r#"{
				"additionalPrompts": [],
				"batchSize": 50,
				"excludePatterns": [],
				"generateI18nFor": [{ "file": "locales/pt/{namespace}.json", "language": "Portuguese" }],
				"interpolationPatterns": [],
				"namespaces": ["common", "auth", "checkout"],
				"parallelBatches": 3,
				"referenceI18n": { "file": "locales/en/{namespace}.json", "language": "English" },
				"translator": "claude-code"
			}"#,
		);

		let config = load_config(&path).unwrap();

		match config.namespaces {
			Some(NamespacesConfig::List(names)) => {
				assert_eq!(names, vec!["common".to_string(), "auth".to_string(), "checkout".to_string()]);
			}
			other => panic!("expected explicit list, got {other:?}"),
		}
	}

	#[test]
	fn namespaces_field_parses_wildcard_string() {
		let dir = TempDir::new().unwrap();
		let path = write_config(
			&dir,
			"a.json",
			r#"{
				"additionalPrompts": [],
				"batchSize": 50,
				"excludePatterns": [],
				"generateI18nFor": [{ "file": "locales/pt/{namespace}.json", "language": "Portuguese" }],
				"interpolationPatterns": [],
				"namespaces": "*",
				"parallelBatches": 3,
				"referenceI18n": { "file": "locales/en/{namespace}.json", "language": "English" },
				"translator": "claude-code"
			}"#,
		);

		let config = load_config(&path).unwrap();

		assert!(matches!(config.namespaces, Some(NamespacesConfig::Wildcard)));
	}

	#[test]
	fn namespaces_field_rejects_unknown_string_value() {
		let dir = TempDir::new().unwrap();
		let path = write_config(
			&dir,
			"a.json",
			r#"{
				"additionalPrompts": [],
				"batchSize": 50,
				"excludePatterns": [],
				"generateI18nFor": [],
				"interpolationPatterns": [],
				"namespaces": "auto",
				"parallelBatches": 3,
				"referenceI18n": { "file": "locales/en.json", "language": "English" },
				"translator": "claude-code"
			}"#,
		);

		let err = load_config(&path).unwrap_err();

		assert!(format!("{err:#}").contains("\"*\""));
	}
}
