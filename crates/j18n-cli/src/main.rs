mod args;
mod config;

use anyhow::{Context, Result};
use args::{Cli, Command, CommandArgs, InitArgs};
use clap::Parser;
use config::{I18nToolConfig, TranslatorKind};
use j18n_claude_code::ClaudeCodeBasedI18nTranslator;
use j18n_core::{GenerationMode, I18nDefinition, Language};
use j18n_gemini_api::GeminiApiI18nTranslator;
use j18n_generator::I18nGenerator;
use j18n_translator::I18nTranslator;
use j18n_validator::TranslationValidator;
use std::path::{Path, PathBuf};
use tracing::info;
use tracing_subscriber::EnvFilter;

const SKELETON_CONFIG: &str = "{\n\t\"baseDirectory\": \"\",\n\t\"referenceI18n\": \"en\",\n\t\"generateI18nFor\": [],\n\t\"translator\": \"claude-code\"\n}\n";

#[tokio::main]
async fn main() -> Result<()> {
	init_logging();

	let cli = Cli::parse();

	match cli.command {
		Command::Init(args) => init(args).await,
		Command::Sync(args) => run(args, GenerationMode::Sync).await,
		Command::Regenerate(args) => run(args, GenerationMode::Regenerate).await,
	}
}

async fn init(args: InitArgs) -> Result<()> {
	if tokio::fs::try_exists(&args.path)
		.await
		.with_context(|| format!("failed to stat \"{}\"", args.path.display()))?
	{
		anyhow::bail!("refusing to overwrite existing file at \"{}\"", args.path.display());
	}

	if let Some(parent) = args.path.parent() {
		if !parent.as_os_str().is_empty() {
			tokio::fs::create_dir_all(parent)
				.await
				.with_context(|| format!("failed to create directory \"{}\"", parent.display()))?;
		}
	}

	tokio::fs::write(&args.path, SKELETON_CONFIG)
		.await
		.with_context(|| format!("failed to write \"{}\"", args.path.display()))?;

	info!("Created skeleton config at \"{}\"", args.path.display());

	Ok(())
}

async fn run(args: CommandArgs, mode: GenerationMode) -> Result<()> {
	for config_path in &args.configs {
		let config = config::load_config(config_path)?;
		let (reference_i18n, generated_i18ns) = build_definitions(config_path, &config)
			.with_context(|| format!("invalid config \"{}\"", config_path.display()))?;
		let translator: Box<dyn I18nTranslator> = match config.translator {
			TranslatorKind::ClaudeCode => Box::new(ClaudeCodeBasedI18nTranslator::new()),
			TranslatorKind::GeminiApi => Box::new(GeminiApiI18nTranslator::new()?),
		};

		I18nGenerator::execute(translator.as_ref(), &reference_i18n, &generated_i18ns, mode).await?;
		TranslationValidator::validate_translations(&reference_i18n, &generated_i18ns).await?;
	}

	Ok(())
}

fn build_definitions(config_path: &Path, config: &I18nToolConfig) -> Result<(I18nDefinition, Vec<I18nDefinition>)> {
	let reference_language = Language::from_iso_639_code(&config.reference_i18n)
		.with_context(|| format!("invalid referenceI18n \"{}\"", config.reference_i18n))?;
	let base_dir = resolve_base_dir(config_path, &config.base_directory);
	let reference_i18n = I18nDefinition::from_base_dir(&base_dir, reference_language);
	let generated_i18ns = build_target_definitions(&base_dir, &config.generate_i18n_for)?;

	Ok((reference_i18n, generated_i18ns))
}

fn resolve_base_dir(config_path: &Path, base_directory: &Path) -> PathBuf {
	if base_directory.is_absolute() {
		return base_directory.to_path_buf();
	}

	config_path
		.parent()
		.filter(|parent| !parent.as_os_str().is_empty())
		.map(|parent| parent.join(base_directory))
		.unwrap_or_else(|| base_directory.to_path_buf())
}

fn build_target_definitions(base_dir: &Path, codes: &[String]) -> Result<Vec<I18nDefinition>> {
	let mut definitions = Vec::with_capacity(codes.len());

	for code in codes {
		let language =
			Language::from_iso_639_code(code).with_context(|| format!("invalid generateI18nFor entry \"{code}\""))?;

		definitions.push(I18nDefinition::from_base_dir(base_dir, language));
	}

	Ok(definitions)
}

fn init_logging() {
	let env_filter = EnvFilter::try_from_default_env().unwrap_or_else(|_| EnvFilter::new("info"));

	tracing_subscriber::fmt()
		.with_env_filter(env_filter)
		.with_target(false)
		.with_writer(std::io::stderr)
		.init();
}

#[cfg(test)]
mod tests {
	use super::*;
	use std::env;
	use tempfile::TempDir;

	fn absolute_path(parts: &[&str]) -> PathBuf {
		let mut path = if cfg!(windows) {
			PathBuf::from(r"C:\")
		} else {
			PathBuf::from("/")
		};

		for part in parts {
			path.push(part);
		}

		path
	}

	#[test]
	fn skeleton_config_parses_back_into_defaults() {
		let parsed: I18nToolConfig = serde_json::from_str(SKELETON_CONFIG).unwrap();

		assert_eq!(parsed.reference_i18n, "en");
		assert!(parsed.generate_i18n_for.is_empty());
		assert!(matches!(parsed.translator, TranslatorKind::ClaudeCode));
		assert_eq!(parsed.base_directory, PathBuf::from(""));
	}

	#[test]
	fn resolve_base_dir_returns_absolute_path_unchanged() {
		let absolute = absolute_path(&["abs", "locales"]);

		assert_eq!(
			resolve_base_dir(Path::new("/some/config.json"), &absolute),
			absolute
		);
	}

	#[test]
	fn resolve_base_dir_joins_relative_path_to_config_parent() {
		let resolved = resolve_base_dir(Path::new("/configs/api.json"), Path::new("../locales"));

		assert_eq!(resolved, PathBuf::from("/configs").join("../locales"));
	}

	#[test]
	fn resolve_base_dir_falls_back_to_relative_when_config_has_no_parent() {
		let resolved = resolve_base_dir(Path::new("api.json"), Path::new("locales"));

		assert_eq!(resolved, PathBuf::from("locales"));
	}

	#[test]
	fn build_target_definitions_returns_one_definition_per_code() {
		let base = PathBuf::from("/locales");
		let codes = vec!["pt".to_string(), "es".to_string()];
		let definitions = build_target_definitions(&base, &codes).unwrap();

		assert_eq!(definitions.len(), 2);
		assert_eq!(definitions[0].language.iso_639_code(), "pt");
		assert_eq!(definitions[0].json_file_path, PathBuf::from("/locales/pt.json"));
		assert_eq!(definitions[1].language.iso_639_code(), "es");
	}

	#[test]
	fn build_target_definitions_errors_on_unknown_code() {
		let base = PathBuf::from("/locales");
		let codes = vec!["pt".to_string(), "xx".to_string()];

		let err = build_target_definitions(&base, &codes).unwrap_err();
		let text = format!("{err:#}");

		assert!(text.contains("xx"));
	}

	#[tokio::test]
	async fn init_writes_skeleton_to_given_path() {
		let dir = TempDir::new().unwrap();
		let path = dir.path().join("config.json");
		let original_cwd = env::current_dir().unwrap();

		init(InitArgs { path: path.clone() }).await.unwrap();

		assert!(path.exists());
		let written = tokio::fs::read_to_string(&path).await.unwrap();
		assert_eq!(written, SKELETON_CONFIG);

		env::set_current_dir(original_cwd).unwrap();
	}

	#[tokio::test]
	async fn init_creates_missing_parent_directories() {
		let dir = TempDir::new().unwrap();
		let path = dir.path().join("nested").join("dirs").join("config.json");

		init(InitArgs { path: path.clone() }).await.unwrap();

		assert!(path.exists());
	}

	#[tokio::test]
	async fn init_refuses_to_overwrite_existing_file() {
		let dir = TempDir::new().unwrap();
		let path = dir.path().join("config.json");

		tokio::fs::write(&path, "preexisting").await.unwrap();

		let err = init(InitArgs { path: path.clone() }).await.unwrap_err();
		let text = format!("{err:#}");

		assert!(text.contains("refusing to overwrite"));
		assert_eq!(tokio::fs::read_to_string(&path).await.unwrap(), "preexisting");
	}

	#[test]
	fn build_definitions_uses_resolved_base_dir() {
		let dir = TempDir::new().unwrap();
		let config_path = dir.path().join("nested").join("config.json");
		let config = I18nToolConfig {
			base_directory: PathBuf::from("../locales"),
			generate_i18n_for: vec!["pt".to_string()],
			reference_i18n: "en".to_string(),
			translator: TranslatorKind::ClaudeCode,
		};

		let (reference, generated) = build_definitions(&config_path, &config).unwrap();

		assert!(reference.json_file_path.ends_with("locales/en.json") || reference.json_file_path.ends_with("locales\\en.json"));
		assert_eq!(generated.len(), 1);
		assert_eq!(generated[0].language.iso_639_code(), "pt");
	}
}
