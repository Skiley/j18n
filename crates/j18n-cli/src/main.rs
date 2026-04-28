mod args;
mod config;

use anyhow::{Context, Result};
use args::{Cli, Command, CommandArgs, InitArgs};
use clap::Parser;
use config::{DefinitionEntry, TranslatorKind};
#[cfg(test)]
use config::I18nToolConfig;
use j18n_claude_code::ClaudeCodeBasedI18nTranslator;
use j18n_core::{GenerationMode, I18nDefinition};
use j18n_gemini_api::GeminiApiI18nTranslator;
use j18n_generator::{I18nGenerator, J18nOptions};
use j18n_translator::I18nTranslator;
use j18n_validator::TranslationValidator;
use std::path::{Path, PathBuf};
use tracing::info;
use tracing_subscriber::EnvFilter;

const HASH_CACHE_FILE_NAME: &str = ".hash-cache.json";

const SKELETON_CONFIG: &str = concat!(
	"{\n",
	"\t\"additionalPrompts\": [],\n",
	"\t\"batchSize\": 50,\n",
	"\t\"excludePatterns\": [],\n",
	"\t\"generateI18nFor\": [\n",
	"\t\t{ \"file\": \"locales/pt.json\", \"language\": \"Portuguese\" }\n",
	"\t],\n",
	"\t\"interpolationPatterns\": [],\n",
	"\t\"parallelBatches\": 3,\n",
	"\t\"referenceI18n\": { \"file\": \"locales/en.json\", \"language\": \"English\" },\n",
	"\t\"translator\": \"claude-code\"\n",
	"}\n",
);

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

		config
			.validate_numbers()
			.with_context(|| format!("invalid config \"{}\"", config_path.display()))?;

		let (exclude_patterns, interpolation_patterns) = config
			.compile_patterns()
			.with_context(|| format!("invalid config \"{}\"", config_path.display()))?;
		let reference_i18n = resolve_definition(config_path, &config.reference_i18n);
		let generated_i18ns: Vec<I18nDefinition> = config
			.generate_i18n_for
			.iter()
			.map(|definition| resolve_definition(config_path, definition))
			.collect();
		let hash_cache_path = resolve_hash_cache_path(config_path, &config.hash_cache_location, &reference_i18n.file);
		let options = J18nOptions {
			batch_size: config.batch_size,
			exclude_patterns,
			hash_cache_path,
			interpolation_patterns,
			parallel_batches: config.parallel_batches,
		};
		let translator: Box<dyn I18nTranslator> = match config.translator {
			TranslatorKind::ClaudeCode => Box::new(ClaudeCodeBasedI18nTranslator::new(config.additional_prompts.clone())),
			TranslatorKind::GeminiApi => Box::new(GeminiApiI18nTranslator::new(config.additional_prompts.clone())?),
		};

		I18nGenerator::execute(translator.as_ref(), &reference_i18n, &generated_i18ns, mode, &options).await?;
		TranslationValidator::validate_translations(
			&reference_i18n,
			&generated_i18ns,
			&options.exclude_patterns,
			&options.interpolation_patterns,
		)
		.await?;
	}

	Ok(())
}

fn resolve_definition(config_path: &Path, entry: &DefinitionEntry) -> I18nDefinition {
	I18nDefinition {
		file: resolve_relative(config_path, Path::new(&entry.file)),
		id: entry.file.clone(),
		language: entry.language.clone(),
	}
}

fn resolve_hash_cache_path(
	config_path: &Path,
	hash_cache_location: &Option<PathBuf>,
	reference_file: &Path,
) -> PathBuf {
	if let Some(custom) = hash_cache_location {
		return resolve_relative(config_path, custom);
	}

	reference_file
		.parent()
		.map(|parent| parent.join(HASH_CACHE_FILE_NAME))
		.unwrap_or_else(|| PathBuf::from(HASH_CACHE_FILE_NAME))
}

fn resolve_relative(config_path: &Path, file: &Path) -> PathBuf {
	if file.is_absolute() {
		return file.to_path_buf();
	}

	config_path
		.parent()
		.filter(|parent| !parent.as_os_str().is_empty())
		.map(|parent| parent.join(file))
		.unwrap_or_else(|| file.to_path_buf())
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

		assert_eq!(parsed.reference_i18n.language, "English");
		assert_eq!(parsed.reference_i18n.file, "locales/en.json");
		assert_eq!(parsed.generate_i18n_for.len(), 1);
		assert_eq!(parsed.generate_i18n_for[0].language, "Portuguese");
		assert_eq!(parsed.generate_i18n_for[0].file, "locales/pt.json");
		assert!(matches!(parsed.translator, TranslatorKind::ClaudeCode));
		assert_eq!(parsed.batch_size, 50);
		assert_eq!(parsed.parallel_batches, 3);
		assert!(parsed.exclude_patterns.is_empty());
		assert!(parsed.interpolation_patterns.is_empty());
		assert!(parsed.additional_prompts.is_empty());
		assert!(parsed.hash_cache_location.is_none());
	}

	#[test]
	fn skeleton_config_compiles_and_validates() {
		let parsed: I18nToolConfig = serde_json::from_str(SKELETON_CONFIG).unwrap();

		assert!(parsed.validate_numbers().is_ok());
		assert!(parsed.compile_patterns().is_ok());
	}

	#[test]
	fn resolve_relative_returns_absolute_path_unchanged() {
		let absolute = absolute_path(&["abs", "locales", "en.json"]);

		assert_eq!(resolve_relative(Path::new("/some/config.json"), &absolute), absolute);
	}

	#[test]
	fn resolve_relative_joins_relative_path_to_config_parent() {
		let resolved = resolve_relative(Path::new("/configs/api.json"), Path::new("../locales/en.json"));

		assert_eq!(resolved, PathBuf::from("/configs").join("../locales/en.json"));
	}

	#[test]
	fn resolve_relative_falls_back_to_relative_when_config_has_no_parent() {
		let resolved = resolve_relative(Path::new("api.json"), Path::new("locales/en.json"));

		assert_eq!(resolved, PathBuf::from("locales/en.json"));
	}

	#[test]
	fn resolve_definition_resolves_file_and_keeps_id_and_language() {
		let resolved = resolve_definition(
			Path::new("/configs/api.json"),
			&DefinitionEntry {
				file: "locales/pt.json".to_string(),
				language: "Brazilian Portuguese".to_string(),
			},
		);

		assert_eq!(resolved.file, PathBuf::from("/configs").join("locales/pt.json"));
		assert_eq!(resolved.id, "locales/pt.json");
		assert_eq!(resolved.language, "Brazilian Portuguese");
	}

	#[test]
	fn resolve_hash_cache_path_uses_explicit_value_when_present() {
		let resolved = resolve_hash_cache_path(
			Path::new("/configs/api.json"),
			&Some(PathBuf::from(".cache.json")),
			Path::new("/configs/locales/en.json"),
		);

		assert_eq!(resolved, PathBuf::from("/configs").join(".cache.json"));
	}

	#[test]
	fn resolve_hash_cache_path_uses_explicit_absolute_value_unchanged() {
		let absolute = absolute_path(&["caches", "j18n.json"]);
		let resolved = resolve_hash_cache_path(
			Path::new("/configs/api.json"),
			&Some(absolute.clone()),
			Path::new("/configs/locales/en.json"),
		);

		assert_eq!(resolved, absolute);
	}

	#[test]
	fn resolve_hash_cache_path_defaults_to_reference_directory() {
		let resolved = resolve_hash_cache_path(
			Path::new("/configs/api.json"),
			&None,
			Path::new("/anywhere/locales/en.json"),
		);

		assert_eq!(resolved, PathBuf::from("/anywhere/locales/.hash-cache.json"));
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
}
