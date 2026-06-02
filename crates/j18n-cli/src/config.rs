use anyhow::{Context, Result};
use j18n_core::{ContentFormat, PathPattern};
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
	/// `"*"` — discover namespaces in the single directory that contains the
	/// `{namespace}` token component (non-recursive).
	Wildcard,
	/// `"**"` — discover namespaces recursively under that directory; the
	/// `{namespace}` token then represents a nested relative path (e.g.
	/// `getting-started/faq`). Requires the token to sit in the file name.
	RecursiveWildcard,
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
				formatter.write_str("the string \"*\" or \"**\", or an array of namespace names")
			}

			fn visit_str<E>(self, value: &str) -> Result<Self::Value, E>
			where
				E: de::Error,
			{
				match value {
					"*" => Ok(NamespacesConfig::Wildcard),
					"**" => Ok(NamespacesConfig::RecursiveWildcard),
					other => Err(E::custom(format!(
						"expected \"*\" (single-directory) or \"**\" (recursive) for wildcard namespace discovery, got \"{other}\""
					))),
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

	/// What kind of files the reference/targets are. `"json"` (default) flattens
	/// i18n JSON objects into keyed entries; `"markdown"` translates whole
	/// Markdown/MDX documents while preserving their syntax.
	#[serde(rename = "format", default)]
	pub format: ContentFormat,

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

	#[serde(rename = "retriesPerError")]
	pub retries_per_error: usize,

	pub translator: TranslatorSelection,
}

pub const CLAUDE_CODE_DEFAULT_MODEL: &str = "opus";
pub const CLAUDE_CODE_DEFAULT_EFFORT: &str = "high";
pub const GEMINI_DEFAULT_MODEL: &str = "gemini-3.1-pro-preview";
pub const CODEX_DEFAULT_MODEL: &str = "gpt-5.1";
pub const CODEX_DEFAULT_EFFORT: &str = "high";
pub const ANTHROPIC_API_DEFAULT_MODEL: &str = "claude-sonnet-4-5";
pub const OPENAI_API_DEFAULT_MODEL: &str = "gpt-5.1";
pub const OPENROUTER_DEFAULT_MODEL: &str = "openai/gpt-5.1";

#[derive(Clone, Debug, Eq, PartialEq)]
pub enum TranslatorSelection {
	ClaudeCode { model: String, effort: String },
	GeminiApi { model: String },
	Codex { model: String, effort: String },
	AnthropicApi { model: String },
	OpenAiApi { model: String },
	OpenRouterApi { model: String },
}

impl TranslatorSelection {
	pub fn parse(value: &str) -> Result<Self, String> {
		let parts: Vec<&str> = value.split('/').collect();
		let kind = parts[0];
		let extra = &parts[1..];

		match kind {
			"claude-code" => match extra.len() {
				0 => Ok(TranslatorSelection::ClaudeCode {
					model: CLAUDE_CODE_DEFAULT_MODEL.into(),
					effort: CLAUDE_CODE_DEFAULT_EFFORT.into(),
				}),
				1 => Ok(TranslatorSelection::ClaudeCode {
					model: extra[0].into(),
					effort: CLAUDE_CODE_DEFAULT_EFFORT.into(),
				}),
				2 => Ok(TranslatorSelection::ClaudeCode {
					model: extra[0].into(),
					effort: extra[1].into(),
				}),
				_ => Err(format!(
					"too many segments in translator \"{value}\"; expected at most \"claude-code/<model>/<effort>\""
				)),
			},
			"gemini-api" => match extra.len() {
				0 => Ok(TranslatorSelection::GeminiApi {
					model: GEMINI_DEFAULT_MODEL.into(),
				}),
				1 => Ok(TranslatorSelection::GeminiApi {
					model: normalize_gemini_model(extra[0]),
				}),
				_ => Err(format!(
					"too many segments in translator \"{value}\"; expected at most \"gemini-api/<model>\""
				)),
			},
			"codex" => match extra.len() {
				0 => Ok(TranslatorSelection::Codex {
					model: CODEX_DEFAULT_MODEL.into(),
					effort: CODEX_DEFAULT_EFFORT.into(),
				}),
				1 => Ok(TranslatorSelection::Codex {
					model: extra[0].into(),
					effort: CODEX_DEFAULT_EFFORT.into(),
				}),
				2 => Ok(TranslatorSelection::Codex {
					model: extra[0].into(),
					effort: extra[1].into(),
				}),
				_ => Err(format!(
					"too many segments in translator \"{value}\"; expected at most \"codex/<model>/<effort>\""
				)),
			},
			"anthropic-api" => match extra.len() {
				0 => Ok(TranslatorSelection::AnthropicApi {
					model: ANTHROPIC_API_DEFAULT_MODEL.into(),
				}),
				1 => Ok(TranslatorSelection::AnthropicApi { model: extra[0].into() }),
				_ => Err(format!(
					"too many segments in translator \"{value}\"; expected at most \"anthropic-api/<model>\""
				)),
			},
			"openai-api" => match extra.len() {
				0 => Ok(TranslatorSelection::OpenAiApi {
					model: OPENAI_API_DEFAULT_MODEL.into(),
				}),
				1 => Ok(TranslatorSelection::OpenAiApi { model: extra[0].into() }),
				_ => Err(format!(
					"too many segments in translator \"{value}\"; expected at most \"openai-api/<model>\""
				)),
			},
			// OpenRouter model slugs themselves contain '/' (e.g. "openai/gpt-5.1"),
			// so everything after the kind is the model — no segment-count limit.
			"openrouter-api" => {
				if extra.is_empty() {
					Ok(TranslatorSelection::OpenRouterApi {
						model: OPENROUTER_DEFAULT_MODEL.into(),
					})
				} else {
					Ok(TranslatorSelection::OpenRouterApi { model: extra.join("/") })
				}
			}
			other => Err(format!(
				"unknown translator kind \"{other}\"; expected one of \"claude-code\", \"gemini-api\", \"codex\", \
				\"anthropic-api\", \"openai-api\", or \"openrouter-api\""
			)),
		}
	}
}

fn normalize_gemini_model(value: &str) -> String {
	if value.starts_with("gemini-") {
		value.to_string()
	} else {
		format!("gemini-{value}")
	}
}

impl<'de> Deserialize<'de> for TranslatorSelection {
	fn deserialize<D>(deserializer: D) -> Result<Self, D::Error>
	where
		D: Deserializer<'de>,
	{
		let raw = String::deserialize(deserializer)?;

		TranslatorSelection::parse(&raw).map_err(de::Error::custom)
	}
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
			"retriesPerError": 3,
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
		assert_eq!(config.retries_per_error, 3);
		assert_eq!(config.exclude_patterns, vec!["sample.**".to_string()]);
		assert_eq!(config.interpolation_patterns, vec!["\\{\\{(.+?)\\}\\}".to_string()]);
		assert_eq!(config.reference_i18n.file, "locales/en.json");
		assert_eq!(config.reference_i18n.language, "English");
		assert_eq!(config.generate_i18n_for.len(), 2);
		assert_eq!(config.generate_i18n_for[0].file, "locales/pt.json");
		assert_eq!(config.generate_i18n_for[0].language, "Brazilian Portuguese");
		assert!(matches!(
			config.translator,
			TranslatorSelection::ClaudeCode { ref model, ref effort }
				if model == CLAUDE_CODE_DEFAULT_MODEL && effort == CLAUDE_CODE_DEFAULT_EFFORT
		));
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
				"hashCacheLocation": "custom/.j18n-cache.ini",
				"interpolationPatterns": [],
				"parallelBatches": 3,
				"retriesPerError": 3,
				"referenceI18n": { "file": "en.json", "language": "English" },
				"translator": "claude-code"
			}"#,
		);

		let config = load_config(&path).unwrap();

		assert_eq!(
			config.hash_cache_location,
			Some(PathBuf::from("custom/.j18n-cache.ini"))
		);
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
				"retriesPerError": 3,
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
				"retriesPerError": 3,
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
				"retriesPerError": 3,
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
				"retriesPerError": 3,
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
				"retriesPerError": 3,
				"referenceI18n": { "file": "en.json", "language": "English" },
				"translator": "gemini-api"
			}"#,
		);

		let config = load_config(&path).unwrap();

		assert!(matches!(
			config.translator,
			TranslatorSelection::GeminiApi { ref model } if model == GEMINI_DEFAULT_MODEL
		));
	}

	#[test]
	fn parses_gemini_translator_with_explicit_model() {
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
				"retriesPerError": 3,
				"referenceI18n": { "file": "en.json", "language": "English" },
				"translator": "gemini-api/3.1-pro"
			}"#,
		);

		let config = load_config(&path).unwrap();

		assert!(matches!(
			config.translator,
			TranslatorSelection::GeminiApi { ref model } if model == "gemini-3.1-pro"
		));
	}

	#[test]
	fn parses_gemini_translator_with_full_model_name_unchanged() {
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
				"retriesPerError": 3,
				"referenceI18n": { "file": "en.json", "language": "English" },
				"translator": "gemini-api/gemini-3.1-pro-preview"
			}"#,
		);

		let config = load_config(&path).unwrap();

		assert!(matches!(
			config.translator,
			TranslatorSelection::GeminiApi { ref model } if model == "gemini-3.1-pro-preview"
		));
	}

	#[test]
	fn parses_claude_code_with_model_and_effort() {
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
				"retriesPerError": 3,
				"referenceI18n": { "file": "en.json", "language": "English" },
				"translator": "claude-code/sonnet/medium"
			}"#,
		);

		let config = load_config(&path).unwrap();

		assert!(matches!(
			config.translator,
			TranslatorSelection::ClaudeCode { ref model, ref effort }
				if model == "sonnet" && effort == "medium"
		));
	}

	#[test]
	fn parses_claude_code_with_model_only_defaults_effort_to_high() {
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
				"retriesPerError": 3,
				"referenceI18n": { "file": "en.json", "language": "English" },
				"translator": "claude-code/opus"
			}"#,
		);

		let config = load_config(&path).unwrap();

		assert!(matches!(
			config.translator,
			TranslatorSelection::ClaudeCode { ref model, ref effort }
				if model == "opus" && effort == CLAUDE_CODE_DEFAULT_EFFORT
		));
	}

	#[test]
	fn parses_codex_translator_with_model_and_effort() {
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
				"retriesPerError": 3,
				"referenceI18n": { "file": "en.json", "language": "English" },
				"translator": "codex/gpt-5.1/low"
			}"#,
		);

		let config = load_config(&path).unwrap();

		assert!(matches!(
			config.translator,
			TranslatorSelection::Codex { ref model, ref effort }
				if model == "gpt-5.1" && effort == "low"
		));
	}

	#[test]
	fn parses_codex_translator_with_only_kind_uses_defaults() {
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
				"retriesPerError": 3,
				"referenceI18n": { "file": "en.json", "language": "English" },
				"translator": "codex"
			}"#,
		);

		let config = load_config(&path).unwrap();

		assert!(matches!(
			config.translator,
			TranslatorSelection::Codex { ref model, ref effort }
				if model == CODEX_DEFAULT_MODEL && effort == CODEX_DEFAULT_EFFORT
		));
	}

	#[test]
	fn parses_anthropic_api_translator_with_defaults_and_explicit_model() {
		assert!(matches!(
			TranslatorSelection::parse("anthropic-api"),
			Ok(TranslatorSelection::AnthropicApi { ref model }) if model == ANTHROPIC_API_DEFAULT_MODEL
		));
		assert!(matches!(
			TranslatorSelection::parse("anthropic-api/claude-opus-4-5"),
			Ok(TranslatorSelection::AnthropicApi { ref model }) if model == "claude-opus-4-5"
		));
		assert!(TranslatorSelection::parse("anthropic-api/claude/extra").is_err());
	}

	#[test]
	fn parses_openai_api_translator_with_defaults_and_explicit_model() {
		assert!(matches!(
			TranslatorSelection::parse("openai-api"),
			Ok(TranslatorSelection::OpenAiApi { ref model }) if model == OPENAI_API_DEFAULT_MODEL
		));
		assert!(matches!(
			TranslatorSelection::parse("openai-api/gpt-4.1-mini"),
			Ok(TranslatorSelection::OpenAiApi { ref model }) if model == "gpt-4.1-mini"
		));
		assert!(TranslatorSelection::parse("openai-api/gpt-4.1/extra").is_err());
	}

	#[test]
	fn parses_openrouter_translator_defaulting_and_preserving_slash_in_model_slug() {
		assert!(matches!(
			TranslatorSelection::parse("openrouter-api"),
			Ok(TranslatorSelection::OpenRouterApi { ref model }) if model == OPENROUTER_DEFAULT_MODEL
		));
		// OpenRouter model slugs contain '/', and even three-segment slugs round-trip.
		assert!(matches!(
			TranslatorSelection::parse("openrouter-api/anthropic/claude-sonnet-4.5"),
			Ok(TranslatorSelection::OpenRouterApi { ref model }) if model == "anthropic/claude-sonnet-4.5"
		));
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
				"retriesPerError": 3,
				"referenceI18n": { "file": "en.json", "language": "English" },
				"translator": "openai"
			}"#,
		);

		let err = load_config(&path).unwrap_err();
		let text = format!("{err:#}");

		assert!(text.contains("claude-code") && text.contains("gemini-api") && text.contains("codex"));
	}

	#[test]
	fn format_defaults_to_json_when_absent() {
		let dir = TempDir::new().unwrap();
		let path = write_config(&dir, "a.json", full_config_json());

		let config = load_config(&path).unwrap();

		assert_eq!(config.format, ContentFormat::Json);
	}

	#[test]
	fn parses_markdown_format() {
		let dir = TempDir::new().unwrap();
		let path = write_config(
			&dir,
			"a.json",
			r#"{
				"additionalPrompts": [],
				"batchSize": 50,
				"excludePatterns": [],
				"format": "markdown",
				"generateI18nFor": [{ "file": "i18n/pt/welcome.mdx", "language": "Brazilian Portuguese" }],
				"interpolationPatterns": [],
				"parallelBatches": 3,
				"retriesPerError": 3,
				"referenceI18n": { "file": "docs/welcome.mdx", "language": "English" },
				"translator": "claude-code"
			}"#,
		);

		let config = load_config(&path).unwrap();

		assert_eq!(config.format, ContentFormat::Markdown);
	}

	#[test]
	fn rejects_unknown_format_value() {
		let dir = TempDir::new().unwrap();
		let path = write_config(
			&dir,
			"a.json",
			r#"{
				"additionalPrompts": [],
				"batchSize": 50,
				"excludePatterns": [],
				"format": "yaml",
				"generateI18nFor": [],
				"interpolationPatterns": [],
				"parallelBatches": 3,
				"retriesPerError": 3,
				"referenceI18n": { "file": "en.json", "language": "English" },
				"translator": "claude-code"
			}"#,
		);

		assert!(load_config(&path).is_err());
	}

	#[test]
	fn rejects_too_many_segments_in_translator_value() {
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
				"retriesPerError": 3,
				"referenceI18n": { "file": "en.json", "language": "English" },
				"translator": "claude-code/opus/high/extra"
			}"#,
		);

		let err = load_config(&path).unwrap_err();
		let text = format!("{err:#}");

		assert!(text.contains("too many segments"));
	}

	#[test]
	fn rejects_too_many_segments_in_gemini_translator_value() {
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
				"retriesPerError": 3,
				"referenceI18n": { "file": "en.json", "language": "English" },
				"translator": "gemini-api/3.1-pro/high"
			}"#,
		);

		let err = load_config(&path).unwrap_err();

		assert!(format!("{err:#}").contains("too many segments"));
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
	fn rejects_missing_retries_per_error() {
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
				"translator": "claude-code"
			}"#,
		);

		let err = load_config(&path).unwrap_err();

		assert!(format!("{err:#}").contains("retriesPerError"));
	}

	#[test]
	fn parses_retries_per_error_zero_meaning_no_retries() {
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
				"retriesPerError": 0,
				"translator": "claude-code"
			}"#,
		);

		let config = load_config(&path).unwrap();

		assert_eq!(config.retries_per_error, 0);
	}

	#[test]
	fn parses_retries_per_error_with_custom_value() {
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
				"retriesPerError": 7,
				"translator": "claude-code"
			}"#,
		);

		let config = load_config(&path).unwrap();

		assert_eq!(config.retries_per_error, 7);
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
				"retriesPerError": 3,
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
				"retriesPerError": 3,
				"referenceI18n": { "file": "locales/en/{namespace}.json", "language": "English" },
				"translator": "claude-code"
			}"#,
		);

		let config = load_config(&path).unwrap();

		match config.namespaces {
			Some(NamespacesConfig::List(names)) => {
				assert_eq!(
					names,
					vec!["common".to_string(), "auth".to_string(), "checkout".to_string()]
				);
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
				"retriesPerError": 3,
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
				"retriesPerError": 3,
				"referenceI18n": { "file": "locales/en.json", "language": "English" },
				"translator": "claude-code"
			}"#,
		);

		let err = load_config(&path).unwrap_err();

		assert!(format!("{err:#}").contains("\"*\""));
	}
}
