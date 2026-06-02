mod args;
mod config;
mod expand;

use anyhow::{Context, Result};
use args::{Cli, Command, CommandArgs, InitArgs, InstallGitHookArgs};
use clap::Parser;
use config::{I18nToolConfig, NamespacesConfig, TranslatorSelection};
use j18n_anthropic_api::AnthropicApiI18nTranslator;
use j18n_claude_code::ClaudeCodeBasedI18nTranslator;
use j18n_codex::CodexCliBasedI18nTranslator;
use j18n_core::{GenerationMode, I18nDefinition};
use j18n_gemini_api::GeminiApiI18nTranslator;
use j18n_generator::{I18nGenerator, J18nOptions};
use j18n_openai_api::OpenAiApiI18nTranslator;
use j18n_translator::I18nTranslator;
use j18n_validator::TranslationValidator;
use std::path::{Path, PathBuf};
use tracing::info;
use tracing_subscriber::EnvFilter;

const HASH_CACHE_FILE_NAME: &str = ".j18n-cache.ini";

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
	"\t\"retriesPerError\": 3,\n",
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
		Command::Check(args) => check(args).await,
		Command::Baseline(args) => baseline(args).await,
		Command::InstallGitHook(args) => install_git_hook(args).await,
	}
}

async fn init(args: InitArgs) -> Result<()> {
	let path = args.resolved_path();

	if tokio::fs::try_exists(&path)
		.await
		.with_context(|| format!("failed to stat \"{}\"", path.display()))?
	{
		anyhow::bail!("refusing to overwrite existing file at \"{}\"", path.display());
	}

	if let Some(parent) = path.parent() {
		if !parent.as_os_str().is_empty() {
			tokio::fs::create_dir_all(parent)
				.await
				.with_context(|| format!("failed to create directory \"{}\"", parent.display()))?;
		}
	}

	tokio::fs::write(&path, SKELETON_CONFIG)
		.await
		.with_context(|| format!("failed to write \"{}\"", path.display()))?;

	info!("Created skeleton config at \"{}\"", path.display());

	Ok(())
}

#[derive(Debug)]
struct ResolvedConfig {
	config: I18nToolConfig,
	runs: Vec<ResolvedRun>,
	options: J18nOptions,
}

#[derive(Debug)]
struct ResolvedRun {
	namespace: Option<String>,
	reference_i18n: I18nDefinition,
	generated_i18ns: Vec<I18nDefinition>,
}

async fn resolve_config(config_path: &Path) -> Result<ResolvedConfig> {
	let config = config::load_config(config_path)?;

	config
		.validate_numbers()
		.with_context(|| format!("invalid config \"{}\"", config_path.display()))?;

	let (exclude_patterns, interpolation_patterns) = config
		.compile_patterns()
		.with_context(|| format!("invalid config \"{}\"", config_path.display()))?;
	let target_files: Vec<&str> = config
		.generate_i18n_for
		.iter()
		.map(|entry| entry.file.as_str())
		.collect();

	expand::validate_token_consistency(config.namespaces.is_some(), &config.reference_i18n.file, &target_files)
		.with_context(|| format!("invalid config \"{}\"", config_path.display()))?;

	let resolved_reference_template = resolve_relative(config_path, Path::new(&config.reference_i18n.file));
	let hash_cache_location =
		resolve_hash_cache_location(config_path, &config.hash_cache_location, &resolved_reference_template);
	let runs = build_runs(config_path, &config, &resolved_reference_template).await?;
	let options = J18nOptions {
		batch_size: config.batch_size,
		exclude_patterns,
		format: config.format,
		hash_cache_location,
		interpolation_patterns,
		parallel_batches: config.parallel_batches,
		retries_per_error: config.retries_per_error,
	};

	Ok(ResolvedConfig { config, runs, options })
}

async fn build_runs(
	config_path: &Path,
	config: &I18nToolConfig,
	resolved_reference_template: &Path,
) -> Result<Vec<ResolvedRun>> {
	match &config.namespaces {
		None => Ok(vec![ResolvedRun {
			namespace: None,
			reference_i18n: build_definition(
				config_path,
				&config.reference_i18n.file,
				&config.reference_i18n.language,
			),
			generated_i18ns: config
				.generate_i18n_for
				.iter()
				.map(|entry| build_definition(config_path, &entry.file, &entry.language))
				.collect(),
		}]),
		Some(NamespacesConfig::List(names)) => build_namespaced_runs(config_path, config, names, false),
		Some(NamespacesConfig::Wildcard) => {
			let names = expand::discover_namespaces_from_reference(resolved_reference_template)
				.await
				.with_context(|| format!("namespace discovery failed for \"{}\"", config_path.display()))?;

			info!(
				"Discovered {} namespace(s) for \"{}\": {}",
				names.len(),
				config_path.display(),
				names.join(", ")
			);
			build_namespaced_runs(config_path, config, &names, false)
		}
		Some(NamespacesConfig::RecursiveWildcard) => {
			let names = expand::discover_namespaces_recursive(resolved_reference_template)
				.await
				.with_context(|| format!("recursive namespace discovery failed for \"{}\"", config_path.display()))?;

			info!(
				"Discovered {} namespace(s) recursively for \"{}\": {}",
				names.len(),
				config_path.display(),
				names.join(", ")
			);
			build_namespaced_runs(config_path, config, &names, true)
		}
	}
}

fn build_namespaced_runs(
	config_path: &Path,
	config: &I18nToolConfig,
	namespace_names: &[String],
	allow_nested: bool,
) -> Result<Vec<ResolvedRun>> {
	let target_files: Vec<String> = config
		.generate_i18n_for
		.iter()
		.map(|entry| entry.file.clone())
		.collect();
	let expanded = expand::expand_with_list(
		&config.reference_i18n.file,
		&target_files,
		namespace_names,
		allow_nested,
	)
	.with_context(|| format!("invalid config \"{}\"", config_path.display()))?;
	let mut runs: Vec<ResolvedRun> = Vec::with_capacity(expanded.len());

	for run in expanded {
		let reference_i18n = build_definition(config_path, &run.reference_file, &config.reference_i18n.language);
		let mut generated_i18ns: Vec<I18nDefinition> = Vec::with_capacity(run.target_files.len());

		for (index, target_file) in run.target_files.iter().enumerate() {
			let language = &config.generate_i18n_for[index].language;

			generated_i18ns.push(build_definition(config_path, target_file, language));
		}

		runs.push(ResolvedRun {
			namespace: run.namespace,
			reference_i18n,
			generated_i18ns,
		});
	}

	Ok(runs)
}

async fn run(args: CommandArgs, mode: GenerationMode) -> Result<()> {
	for config_path in &args.resolved_configs() {
		let resolved = resolve_config(config_path).await?;
		let translator: Box<dyn I18nTranslator> = match &resolved.config.translator {
			TranslatorSelection::ClaudeCode { model, effort } => {
				Box::new(ClaudeCodeBasedI18nTranslator::with_settings(
					resolved.config.additional_prompts.clone(),
					model.clone(),
					effort.clone(),
				))
			}
			TranslatorSelection::GeminiApi { model } => Box::new(GeminiApiI18nTranslator::with_settings(
				resolved.config.additional_prompts.clone(),
				model.clone(),
			)?),
			TranslatorSelection::Codex { model, effort } => Box::new(CodexCliBasedI18nTranslator::with_settings(
				resolved.config.additional_prompts.clone(),
				model.clone(),
				effort.clone(),
			)),
			TranslatorSelection::AnthropicApi { model } => Box::new(AnthropicApiI18nTranslator::with_settings(
				resolved.config.additional_prompts.clone(),
				model.clone(),
			)?),
			TranslatorSelection::OpenAiApi { model } => Box::new(OpenAiApiI18nTranslator::openai(
				resolved.config.additional_prompts.clone(),
				model.clone(),
			)?),
			TranslatorSelection::OpenRouterApi { model } => Box::new(OpenAiApiI18nTranslator::openrouter(
				resolved.config.additional_prompts.clone(),
				model.clone(),
			)?),
		};

		for resolved_run in &resolved.runs {
			if let Some(namespace) = &resolved_run.namespace {
				info!("Processing namespace \"{namespace}\"");
			}

			I18nGenerator::execute(
				translator.as_ref(),
				&resolved_run.reference_i18n,
				&resolved_run.generated_i18ns,
				mode,
				&resolved.options,
			)
			.await?;
			TranslationValidator::validate_translations(
				&resolved_run.reference_i18n,
				&resolved_run.generated_i18ns,
				&resolved.options.exclude_patterns,
				&resolved.options.interpolation_patterns,
				resolved.options.format,
			)
			.await?;
		}
	}

	Ok(())
}

async fn check(args: CommandArgs) -> Result<()> {
	let mut out_of_sync = false;
	let mut total_files: usize = 0;
	let mut total_entries: usize = 0;

	for config_path in &args.resolved_configs() {
		let resolved = resolve_config(config_path).await?;

		for resolved_run in &resolved.runs {
			let report = I18nGenerator::check(
				&resolved_run.reference_i18n,
				&resolved_run.generated_i18ns,
				&resolved.options,
			)
			.await?;

			total_files += report.targets.len();
			total_entries += report.reference_entries * report.targets.len();

			for result in &report.targets {
				if result.needs_sync() {
					out_of_sync = true;

					let namespace_label = resolved_run
						.namespace
						.as_deref()
						.map(|namespace| format!(" [namespace: {namespace}]"))
						.unwrap_or_default();

					info!(
						"{} ({}){} is out of sync: {} key(s) need translation, {} stale key(s)",
						result.target.language,
						result.target.file.display(),
						namespace_label,
						result.missing_or_changed_keys.len(),
						result.stale_keys.len(),
					);
				}
			}
		}
	}

	if out_of_sync {
		anyhow::bail!("translations are out of sync; run `j18n sync` to update");
	}

	info!("All {total_files} file(s) in sync ({total_entries} entries checked)");

	Ok(())
}

async fn baseline(args: CommandArgs) -> Result<()> {
	let mut total_targets: usize = 0;

	for config_path in &args.resolved_configs() {
		let resolved = resolve_config(config_path).await?;

		for resolved_run in &resolved.runs {
			if let Some(namespace) = &resolved_run.namespace {
				info!("Baselining namespace \"{namespace}\"");
			}

			let count = I18nGenerator::baseline(
				&resolved_run.reference_i18n,
				&resolved_run.generated_i18ns,
				&resolved.options,
			)
			.await?;

			total_targets += count;
		}

		info!(
			"Wrote hash cache to \"{}\"",
			resolved.options.hash_cache_location.display()
		);
	}

	info!("Baseline complete: {total_targets} target(s) recorded");

	Ok(())
}

async fn install_git_hook(args: InstallGitHookArgs) -> Result<()> {
	let cwd = std::env::current_dir().context("failed to read current directory")?;

	install_git_hook_at(&cwd, &args.hook, &args.resolved_configs()).await
}

async fn install_git_hook_at(repo_root: &Path, hook: &str, configs: &[PathBuf]) -> Result<()> {
	let hooks_dir = resolve_git_hooks_dir(repo_root)?;

	tokio::fs::create_dir_all(&hooks_dir)
		.await
		.with_context(|| format!("failed to create hooks dir \"{}\"", hooks_dir.display()))?;

	let hook_path = hooks_dir.join(hook);
	let check_line = build_check_line(configs);
	let already_exists = tokio::fs::try_exists(&hook_path)
		.await
		.with_context(|| format!("failed to stat \"{}\"", hook_path.display()))?;

	if already_exists {
		let existing = tokio::fs::read_to_string(&hook_path)
			.await
			.with_context(|| format!("failed to read \"{}\"", hook_path.display()))?;

		if existing.lines().any(|line| line.trim() == check_line) {
			info!(
				"{hook} hook at \"{}\" already runs `{}`; nothing to do",
				hook_path.display(),
				check_line
			);

			return Ok(());
		}

		let mut updated = existing;

		if !updated.is_empty() && !updated.ends_with('\n') {
			updated.push('\n');
		}

		updated.push_str(&check_line);
		updated.push('\n');

		tokio::fs::write(&hook_path, updated.as_bytes())
			.await
			.with_context(|| format!("failed to write \"{}\"", hook_path.display()))?;

		info!(
			"Appended `{}` to existing {hook} hook at \"{}\"",
			check_line,
			hook_path.display()
		);
	} else {
		let script = build_initial_script(&check_line);

		tokio::fs::write(&hook_path, script.as_bytes())
			.await
			.with_context(|| format!("failed to write \"{}\"", hook_path.display()))?;

		info!("Installed {hook} hook at \"{}\"", hook_path.display());
	}

	#[cfg(unix)]
	{
		use std::os::unix::fs::PermissionsExt;

		let mut permissions = tokio::fs::metadata(&hook_path)
			.await
			.with_context(|| format!("failed to stat \"{}\"", hook_path.display()))?
			.permissions();

		permissions.set_mode(0o755);
		tokio::fs::set_permissions(&hook_path, permissions)
			.await
			.with_context(|| format!("failed to chmod \"{}\"", hook_path.display()))?;
	}

	Ok(())
}

fn resolve_git_hooks_dir(repo_root: &Path) -> Result<PathBuf> {
	let dot_git = repo_root.join(".git");
	let metadata = std::fs::metadata(&dot_git).with_context(|| {
		format!(
			"no .git found at \"{}\"; run from the git repo root",
			repo_root.display()
		)
	})?;

	let git_dir = if metadata.is_dir() {
		dot_git
	} else {
		let content =
			std::fs::read_to_string(&dot_git).with_context(|| format!("failed to read \"{}\"", dot_git.display()))?;
		let raw = content
			.lines()
			.find_map(|line| line.strip_prefix("gitdir:"))
			.context("malformed .git file: missing 'gitdir:' line")?
			.trim();
		let path = Path::new(raw);

		if path.is_absolute() {
			path.to_path_buf()
		} else {
			repo_root.join(path)
		}
	};

	Ok(git_dir.join("hooks"))
}

fn build_check_line(configs: &[PathBuf]) -> String {
	let mut line = String::from("j18n check");

	for config in configs {
		let normalized = config.to_string_lossy().replace('\\', "/");

		line.push_str(" -f ");
		line.push_str(&shell_single_quote(&normalized));
	}

	line
}

fn build_initial_script(check_line: &str) -> String {
	format!("#!/bin/sh\nset -e\n{check_line}\n")
}

fn shell_single_quote(value: &str) -> String {
	let escaped = value.replace('\'', "'\\''");

	format!("'{escaped}'")
}

fn build_definition(config_path: &Path, file_string: &str, language: &str) -> I18nDefinition {
	I18nDefinition {
		file: resolve_relative(config_path, Path::new(file_string)),
		id: file_string.to_string(),
		language: language.to_string(),
	}
}

fn resolve_hash_cache_location(
	config_path: &Path,
	hash_cache_location: &Option<PathBuf>,
	reference_file: &Path,
) -> PathBuf {
	if let Some(custom) = hash_cache_location {
		return resolve_relative(config_path, custom);
	}

	expand::fixed_prefix_directory(reference_file).join(HASH_CACHE_FILE_NAME)
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
		assert!(matches!(
			parsed.translator,
			TranslatorSelection::ClaudeCode { ref model, ref effort }
				if model == "opus" && effort == "high"
		));
		assert_eq!(parsed.batch_size, 50);
		assert_eq!(parsed.parallel_batches, 3);
		assert_eq!(parsed.retries_per_error, 3);
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
	fn build_definition_resolves_file_and_keeps_id_and_language() {
		let resolved = build_definition(
			Path::new("/configs/api.json"),
			"locales/pt.json",
			"Brazilian Portuguese",
		);

		assert_eq!(resolved.file, PathBuf::from("/configs").join("locales/pt.json"));
		assert_eq!(resolved.id, "locales/pt.json");
		assert_eq!(resolved.language, "Brazilian Portuguese");
	}

	#[test]
	fn resolve_hash_cache_location_uses_explicit_value_when_present() {
		let resolved = resolve_hash_cache_location(
			Path::new("/configs/api.json"),
			&Some(PathBuf::from(".my-cache.ini")),
			Path::new("/configs/locales/en.json"),
		);

		assert_eq!(resolved, PathBuf::from("/configs").join(".my-cache.ini"));
	}

	#[test]
	fn resolve_hash_cache_location_uses_explicit_absolute_value_unchanged() {
		let absolute = absolute_path(&["caches", "j18n.ini"]);
		let resolved = resolve_hash_cache_location(
			Path::new("/configs/api.json"),
			&Some(absolute.clone()),
			Path::new("/configs/locales/en.json"),
		);

		assert_eq!(resolved, absolute);
	}

	#[test]
	fn resolve_hash_cache_location_defaults_to_fixed_prefix_directory() {
		let resolved = resolve_hash_cache_location(
			Path::new("/configs/api.json"),
			&None,
			Path::new("/anywhere/locales/en.json"),
		);

		assert_eq!(resolved, PathBuf::from("/anywhere/locales/.j18n-cache.ini"));
	}

	#[tokio::test]
	async fn init_writes_skeleton_to_given_path() {
		let dir = TempDir::new().unwrap();
		let path = dir.path().join("config.json");

		init(InitArgs {
			path: Some(path.clone()),
		})
		.await
		.unwrap();

		assert!(path.exists());
		let written = tokio::fs::read_to_string(&path).await.unwrap();
		assert_eq!(written, SKELETON_CONFIG);
	}

	#[tokio::test]
	async fn init_creates_missing_parent_directories() {
		let dir = TempDir::new().unwrap();
		let path = dir.path().join("nested").join("dirs").join("config.json");

		init(InitArgs {
			path: Some(path.clone()),
		})
		.await
		.unwrap();

		assert!(path.exists());
	}

	#[tokio::test]
	async fn init_refuses_to_overwrite_existing_file() {
		let dir = TempDir::new().unwrap();
		let path = dir.path().join("config.json");

		tokio::fs::write(&path, "preexisting").await.unwrap();

		let err = init(InitArgs {
			path: Some(path.clone()),
		})
		.await
		.unwrap_err();
		let text = format!("{err:#}");

		assert!(text.contains("refusing to overwrite"));
		assert_eq!(tokio::fs::read_to_string(&path).await.unwrap(), "preexisting");
	}

	fn write_minimal_config(dir: &TempDir, target_languages: &[&str]) -> PathBuf {
		let config_path = dir.path().join("config.json");
		let targets: Vec<String> = target_languages
			.iter()
			.map(|language| format!(r#"{{ "file": "{language}.json", "language": "{language}" }}"#))
			.collect();
		let body = format!(
			r#"{{
				"additionalPrompts": [],
				"batchSize": 50,
				"excludePatterns": [],
				"generateI18nFor": [{targets}],
				"interpolationPatterns": [],
				"parallelBatches": 3,
				"referenceI18n": {{ "file": "en.json", "language": "English" }},
				"retriesPerError": 3,
				"translator": "claude-code"
			}}"#,
			targets = targets.join(", "),
		);

		std::fs::write(&config_path, body).unwrap();
		config_path
	}

	async fn write_matching_hash_cache(dir: &TempDir, target: &str, entries: &[(&str, &str)]) {
		use j18n_io::{content_hash_hex, I18nHashing, I18nHashingStore};
		use std::collections::HashMap;

		let mut hashes = HashMap::new();

		for (key, value) in entries {
			hashes.insert((*key).to_string(), content_hash_hex(value));
		}

		let store = I18nHashingStore::at(dir.path().join(".j18n-cache.ini"));

		store
			.save(
				&format!("{target}.json@{target}"),
				&I18nHashing {
					json_key_to_hash_map: hashes,
				},
			)
			.await
			.unwrap();
	}

	#[tokio::test]
	async fn check_succeeds_when_targets_are_in_sync() {
		let dir = TempDir::new().unwrap();
		let config = write_minimal_config(&dir, &["pt"]);

		std::fs::write(dir.path().join("en.json"), r#"{"a": "A", "b": "B"}"#).unwrap();
		std::fs::write(dir.path().join("pt.json"), r#"{"a": "AA", "b": "BB"}"#).unwrap();
		write_matching_hash_cache(&dir, "pt", &[("a", "A"), ("b", "B")]).await;

		check(CommandArgs { configs: vec![config] }).await.unwrap();
	}

	#[tokio::test]
	async fn check_fails_when_target_is_missing_keys() {
		let dir = TempDir::new().unwrap();
		let config = write_minimal_config(&dir, &["pt"]);

		std::fs::write(dir.path().join("en.json"), r#"{"a": "A", "b": "B"}"#).unwrap();
		std::fs::write(dir.path().join("pt.json"), r#"{"a": "AA"}"#).unwrap();
		write_matching_hash_cache(&dir, "pt", &[("a", "A"), ("b", "B")]).await;

		let err = check(CommandArgs { configs: vec![config] }).await.unwrap_err();

		assert!(format!("{err:#}").contains("out of sync"));
	}

	#[tokio::test]
	async fn check_fails_when_reference_value_changed_since_last_sync() {
		let dir = TempDir::new().unwrap();
		let config = write_minimal_config(&dir, &["pt"]);

		std::fs::write(dir.path().join("en.json"), r#"{"a": "A-NEW"}"#).unwrap();
		std::fs::write(dir.path().join("pt.json"), r#"{"a": "AA"}"#).unwrap();
		write_matching_hash_cache(&dir, "pt", &[("a", "A-OLD")]).await;

		let err = check(CommandArgs { configs: vec![config] }).await.unwrap_err();

		assert!(format!("{err:#}").contains("out of sync"));
	}

	#[tokio::test]
	async fn check_fails_when_target_has_keys_absent_from_reference() {
		let dir = TempDir::new().unwrap();
		let config = write_minimal_config(&dir, &["pt"]);

		std::fs::write(dir.path().join("en.json"), r#"{"a": "A"}"#).unwrap();
		std::fs::write(dir.path().join("pt.json"), r#"{"a": "AA", "stale": "S"}"#).unwrap();
		write_matching_hash_cache(&dir, "pt", &[("a", "A")]).await;

		let err = check(CommandArgs { configs: vec![config] }).await.unwrap_err();

		assert!(format!("{err:#}").contains("out of sync"));
	}

	#[test]
	fn init_args_resolved_path_returns_default_when_path_omitted() {
		let args = InitArgs::default();

		assert_eq!(args.resolved_path(), PathBuf::from("j18n.json"));
	}

	#[test]
	fn init_args_resolved_path_returns_provided_path_when_present() {
		let args = InitArgs {
			path: Some(PathBuf::from("custom/config.json")),
		};

		assert_eq!(args.resolved_path(), PathBuf::from("custom/config.json"));
	}

	#[test]
	fn init_command_parses_with_no_arguments() {
		let cli = Cli::try_parse_from(["j18n", "init"]).unwrap();

		match cli.command {
			Command::Init(args) => {
				assert!(args.path.is_none());
				assert_eq!(args.resolved_path(), PathBuf::from("j18n.json"));
			}
			other => panic!("expected Init, got {other:?}"),
		}
	}

	#[test]
	fn init_command_parses_with_short_file_flag() {
		let cli = Cli::try_parse_from(["j18n", "init", "-f", "custom.json"]).unwrap();

		match cli.command {
			Command::Init(args) => {
				assert_eq!(args.path, Some(PathBuf::from("custom.json")));
			}
			other => panic!("expected Init, got {other:?}"),
		}
	}

	#[test]
	fn init_command_rejects_positional_path_argument() {
		// Bare positional path is no longer accepted; the user must use -f / --file.
		let result = Cli::try_parse_from(["j18n", "init", "j18n.json"]);

		assert!(result.is_err());
	}

	#[tokio::test]
	async fn init_writes_skeleton_to_default_path_when_omitted() {
		let dir = TempDir::new().unwrap();
		let original_cwd = env::current_dir().unwrap();

		env::set_current_dir(dir.path()).unwrap();

		init(InitArgs::default()).await.unwrap();

		let default_path = dir.path().join("j18n.json");
		assert!(default_path.exists());
		let written = tokio::fs::read_to_string(&default_path).await.unwrap();
		assert_eq!(written, SKELETON_CONFIG);

		env::set_current_dir(original_cwd).unwrap();
	}

	#[test]
	fn resolved_configs_returns_default_when_no_files_provided() {
		let args = CommandArgs::default();

		assert_eq!(args.resolved_configs(), vec![PathBuf::from("j18n.json")]);
	}

	#[test]
	fn resolved_configs_returns_provided_files_when_present() {
		let args = CommandArgs {
			configs: vec![PathBuf::from("a.json"), PathBuf::from("b.json")],
		};

		assert_eq!(
			args.resolved_configs(),
			vec![PathBuf::from("a.json"), PathBuf::from("b.json")]
		);
	}

	#[test]
	fn check_command_parses_with_no_arguments() {
		let cli = Cli::try_parse_from(["j18n", "check"]).unwrap();

		match cli.command {
			Command::Check(args) => {
				assert!(args.configs.is_empty());
				assert_eq!(args.resolved_configs(), vec![PathBuf::from("j18n.json")]);
			}
			other => panic!("expected Check, got {other:?}"),
		}
	}

	#[test]
	fn check_command_parses_with_single_short_file_flag() {
		let cli = Cli::try_parse_from(["j18n", "check", "-f", "custom.json"]).unwrap();

		match cli.command {
			Command::Check(args) => {
				assert_eq!(args.configs, vec![PathBuf::from("custom.json")]);
				assert_eq!(args.resolved_configs(), vec![PathBuf::from("custom.json")]);
			}
			other => panic!("expected Check, got {other:?}"),
		}
	}

	#[test]
	fn check_command_parses_with_repeated_long_file_flag() {
		let cli = Cli::try_parse_from(["j18n", "check", "--file", "a.json", "--file", "b.json"]).unwrap();

		match cli.command {
			Command::Check(args) => {
				assert_eq!(args.configs, vec![PathBuf::from("a.json"), PathBuf::from("b.json")]);
			}
			other => panic!("expected Check, got {other:?}"),
		}
	}

	#[test]
	fn check_command_rejects_positional_config_argument() {
		// Bare positional configs are no longer accepted; the user must use -f / --file.
		let result = Cli::try_parse_from(["j18n", "check", "j18n.json"]);

		assert!(result.is_err());
	}

	#[test]
	fn build_check_line_quotes_each_config_path() {
		let line = build_check_line(&[PathBuf::from("locales/app.json"), PathBuf::from("locales/web.json")]);

		assert_eq!(line, "j18n check -f 'locales/app.json' -f 'locales/web.json'");
	}

	#[test]
	fn build_check_line_normalizes_windows_separators() {
		let line = build_check_line(&[PathBuf::from(r"locales\nested\app.json")]);

		assert_eq!(line, "j18n check -f 'locales/nested/app.json'");
	}

	#[test]
	fn build_initial_script_includes_shebang_and_set_e() {
		let script = build_initial_script("j18n check -f 'a.json'");

		assert_eq!(script, "#!/bin/sh\nset -e\nj18n check -f 'a.json'\n");
	}

	#[test]
	fn shell_single_quote_escapes_embedded_single_quotes() {
		assert_eq!(shell_single_quote("a'b"), "'a'\\''b'");
		assert_eq!(shell_single_quote("plain"), "'plain'");
	}

	#[test]
	fn resolve_git_hooks_dir_uses_dot_git_directory_when_present() {
		let dir = TempDir::new().unwrap();

		std::fs::create_dir(dir.path().join(".git")).unwrap();

		let hooks = resolve_git_hooks_dir(dir.path()).unwrap();

		assert_eq!(hooks, dir.path().join(".git").join("hooks"));
	}

	#[test]
	fn resolve_git_hooks_dir_follows_dot_git_file_pointing_to_actual_git_dir() {
		let dir = TempDir::new().unwrap();
		let actual_git_dir = dir.path().join(".real-git");

		std::fs::create_dir(&actual_git_dir).unwrap();
		std::fs::write(
			dir.path().join(".git"),
			format!("gitdir: {}\n", actual_git_dir.display()),
		)
		.unwrap();

		let hooks = resolve_git_hooks_dir(dir.path()).unwrap();

		assert_eq!(hooks, actual_git_dir.join("hooks"));
	}

	#[test]
	fn resolve_git_hooks_dir_resolves_relative_gitdir_against_repo_root() {
		let dir = TempDir::new().unwrap();

		std::fs::create_dir(dir.path().join(".real-git")).unwrap();
		std::fs::write(dir.path().join(".git"), "gitdir: .real-git\n").unwrap();

		let hooks = resolve_git_hooks_dir(dir.path()).unwrap();

		assert_eq!(hooks, dir.path().join(".real-git").join("hooks"));
	}

	#[test]
	fn resolve_git_hooks_dir_errors_when_dot_git_is_missing() {
		let dir = TempDir::new().unwrap();
		let err = resolve_git_hooks_dir(dir.path()).unwrap_err();

		assert!(format!("{err:#}").contains(".git"));
	}

	#[tokio::test]
	async fn install_git_hook_at_writes_executable_pre_commit_script() {
		let dir = TempDir::new().unwrap();

		std::fs::create_dir_all(dir.path().join(".git").join("hooks")).unwrap();

		install_git_hook_at(dir.path(), "pre-commit", &[PathBuf::from("locales/app.json")])
			.await
			.unwrap();

		let hook_path = dir.path().join(".git").join("hooks").join("pre-commit");
		let content = tokio::fs::read_to_string(&hook_path).await.unwrap();

		assert_eq!(content, "#!/bin/sh\nset -e\nj18n check -f 'locales/app.json'\n");

		#[cfg(unix)]
		{
			use std::os::unix::fs::PermissionsExt;

			let mode = tokio::fs::metadata(&hook_path).await.unwrap().permissions().mode();

			assert_eq!(mode & 0o111, 0o111, "hook should be executable, mode={mode:o}");
		}
	}

	#[tokio::test]
	async fn install_git_hook_at_creates_hooks_dir_when_missing() {
		let dir = TempDir::new().unwrap();

		std::fs::create_dir(dir.path().join(".git")).unwrap();

		install_git_hook_at(dir.path(), "pre-commit", &[PathBuf::from("a.json")])
			.await
			.unwrap();

		assert!(dir.path().join(".git").join("hooks").join("pre-commit").exists());
	}

	#[tokio::test]
	async fn install_git_hook_at_appends_to_existing_pre_commit_hook() {
		let dir = TempDir::new().unwrap();
		let hooks_dir = dir.path().join(".git").join("hooks");

		std::fs::create_dir_all(&hooks_dir).unwrap();
		std::fs::write(hooks_dir.join("pre-commit"), "#!/bin/sh\nnpm test\n").unwrap();

		install_git_hook_at(dir.path(), "pre-commit", &[PathBuf::from("a.json")])
			.await
			.unwrap();

		let content = tokio::fs::read_to_string(hooks_dir.join("pre-commit")).await.unwrap();

		assert_eq!(content, "#!/bin/sh\nnpm test\nj18n check -f 'a.json'\n");
	}

	#[tokio::test]
	async fn install_git_hook_at_appends_when_run_twice_with_different_configs() {
		let dir = TempDir::new().unwrap();

		std::fs::create_dir_all(dir.path().join(".git").join("hooks")).unwrap();

		install_git_hook_at(dir.path(), "pre-commit", &[PathBuf::from("a.json")])
			.await
			.unwrap();
		install_git_hook_at(dir.path(), "pre-commit", &[PathBuf::from("b.json")])
			.await
			.unwrap();

		let content = tokio::fs::read_to_string(dir.path().join(".git").join("hooks").join("pre-commit"))
			.await
			.unwrap();

		assert_eq!(
			content,
			"#!/bin/sh\nset -e\nj18n check -f 'a.json'\nj18n check -f 'b.json'\n"
		);
	}

	#[tokio::test]
	async fn install_git_hook_at_is_idempotent_for_identical_configs() {
		let dir = TempDir::new().unwrap();

		std::fs::create_dir_all(dir.path().join(".git").join("hooks")).unwrap();

		install_git_hook_at(dir.path(), "pre-commit", &[PathBuf::from("a.json")])
			.await
			.unwrap();
		install_git_hook_at(dir.path(), "pre-commit", &[PathBuf::from("a.json")])
			.await
			.unwrap();

		let content = tokio::fs::read_to_string(dir.path().join(".git").join("hooks").join("pre-commit"))
			.await
			.unwrap();

		assert_eq!(content, "#!/bin/sh\nset -e\nj18n check -f 'a.json'\n");
	}

	#[tokio::test]
	async fn install_git_hook_at_appends_newline_when_existing_hook_lacks_trailing_newline() {
		let dir = TempDir::new().unwrap();
		let hooks_dir = dir.path().join(".git").join("hooks");

		std::fs::create_dir_all(&hooks_dir).unwrap();
		std::fs::write(hooks_dir.join("pre-commit"), "#!/bin/sh\nnpm test").unwrap();

		install_git_hook_at(dir.path(), "pre-commit", &[PathBuf::from("a.json")])
			.await
			.unwrap();

		let content = tokio::fs::read_to_string(hooks_dir.join("pre-commit")).await.unwrap();

		assert_eq!(content, "#!/bin/sh\nnpm test\nj18n check -f 'a.json'\n");
	}

	#[tokio::test]
	async fn install_git_hook_at_errors_when_not_in_git_repo() {
		let dir = TempDir::new().unwrap();
		let err = install_git_hook_at(dir.path(), "pre-commit", &[PathBuf::from("a.json")])
			.await
			.unwrap_err();

		assert!(format!("{err:#}").contains(".git"));
	}

	#[tokio::test]
	async fn install_git_hook_at_writes_named_hook() {
		let dir = TempDir::new().unwrap();

		std::fs::create_dir_all(dir.path().join(".git").join("hooks")).unwrap();

		install_git_hook_at(dir.path(), "pre-push", &[PathBuf::from("a.json")])
			.await
			.unwrap();

		let hook_path = dir.path().join(".git").join("hooks").join("pre-push");

		assert!(hook_path.exists());

		let content = tokio::fs::read_to_string(&hook_path).await.unwrap();

		assert_eq!(content, "#!/bin/sh\nset -e\nj18n check -f 'a.json'\n");
	}

	fn write_namespaced_config(
		dir: &TempDir,
		reference_template: &str,
		target_languages: &[(&str, &str)],
		namespaces_field_json: &str,
	) -> PathBuf {
		let config_path = dir.path().join("config.json");
		let targets: Vec<String> = target_languages
			.iter()
			.map(|(template, language)| format!(r#"{{ "file": "{template}", "language": "{language}" }}"#))
			.collect();
		let body = format!(
			r#"{{
				"additionalPrompts": [],
				"batchSize": 50,
				"excludePatterns": [],
				"generateI18nFor": [{targets}],
				"interpolationPatterns": [],
				"namespaces": {namespaces_field_json},
				"parallelBatches": 3,
				"referenceI18n": {{ "file": "{reference_template}", "language": "English" }},
				"retriesPerError": 3,
				"translator": "claude-code"
			}}"#,
			targets = targets.join(", "),
		);

		std::fs::write(&config_path, body).unwrap();
		config_path
	}

	#[tokio::test]
	async fn resolve_config_without_namespaces_produces_single_run() {
		let dir = TempDir::new().unwrap();
		let config = write_minimal_config(&dir, &["pt", "es"]);

		let resolved = resolve_config(&config).await.unwrap();

		assert_eq!(resolved.runs.len(), 1);
		assert!(resolved.runs[0].namespace.is_none());
		assert_eq!(resolved.runs[0].generated_i18ns.len(), 2);
	}

	#[tokio::test]
	async fn resolve_config_with_explicit_namespace_list_produces_one_run_per_namespace() {
		let dir = TempDir::new().unwrap();
		let config = write_namespaced_config(
			&dir,
			"locales/en/{namespace}.json",
			&[("locales/pt/{namespace}.json", "Portuguese")],
			r#"["common", "auth", "checkout"]"#,
		);

		let resolved = resolve_config(&config).await.unwrap();

		assert_eq!(resolved.runs.len(), 3);
		assert_eq!(resolved.runs[0].namespace.as_deref(), Some("common"));
		assert_eq!(resolved.runs[0].reference_i18n.id, "locales/en/common.json");
		assert_eq!(resolved.runs[0].generated_i18ns[0].id, "locales/pt/common.json");
		assert_eq!(resolved.runs[1].namespace.as_deref(), Some("auth"));
		assert_eq!(resolved.runs[2].namespace.as_deref(), Some("checkout"));
	}

	#[tokio::test]
	async fn resolve_config_with_wildcard_discovers_namespaces_from_reference_parent() {
		let dir = TempDir::new().unwrap();
		let locales_en = dir.path().join("locales").join("en");

		std::fs::create_dir_all(&locales_en).unwrap();
		std::fs::write(locales_en.join("common.json"), "{}").unwrap();
		std::fs::write(locales_en.join("auth.json"), "{}").unwrap();

		let config = write_namespaced_config(
			&dir,
			"locales/en/{namespace}.json",
			&[("locales/pt/{namespace}.json", "Portuguese")],
			r#""*""#,
		);

		let resolved = resolve_config(&config).await.unwrap();

		let mut namespaces: Vec<&str> = resolved
			.runs
			.iter()
			.filter_map(|run| run.namespace.as_deref())
			.collect();

		namespaces.sort();
		assert_eq!(namespaces, vec!["auth", "common"]);
	}

	#[tokio::test]
	async fn resolve_config_with_recursive_wildcard_discovers_nested_namespaces() {
		let dir = TempDir::new().unwrap();
		let docs = dir.path().join("docs");

		std::fs::create_dir_all(docs.join("getting-started")).unwrap();
		std::fs::write(docs.join("welcome.mdx"), "# Welcome").unwrap();
		std::fs::write(docs.join("getting-started").join("faq.mdx"), "# FAQ").unwrap();

		let config = write_namespaced_config(
			&dir,
			"docs/{namespace}.mdx",
			&[("i18n/pt/current/{namespace}.mdx", "Portuguese")],
			r#""**""#,
		);

		let resolved = resolve_config(&config).await.unwrap();

		let mut namespaces: Vec<&str> = resolved
			.runs
			.iter()
			.filter_map(|run| run.namespace.as_deref())
			.collect();

		namespaces.sort();
		assert_eq!(namespaces, vec!["getting-started/faq", "welcome"]);

		let faq_run = resolved
			.runs
			.iter()
			.find(|run| run.namespace.as_deref() == Some("getting-started/faq"))
			.unwrap();

		assert_eq!(faq_run.reference_i18n.id, "docs/getting-started/faq.mdx");
		assert_eq!(faq_run.generated_i18ns[0].id, "i18n/pt/current/getting-started/faq.mdx");
	}

	#[tokio::test]
	async fn resolve_config_errors_when_token_present_without_namespaces_field() {
		let dir = TempDir::new().unwrap();
		let config = write_namespaced_config(
			&dir,
			"locales/en/{namespace}.json",
			&[("locales/pt/{namespace}.json", "Portuguese")],
			"null",
		);

		let err = resolve_config(&config).await.unwrap_err();

		assert!(format!("{err:#}").contains("`namespaces` is not set"));
	}

	#[tokio::test]
	async fn resolve_config_errors_when_namespaces_set_but_target_lacks_token() {
		let dir = TempDir::new().unwrap();
		let config = write_namespaced_config(
			&dir,
			"locales/en/{namespace}.json",
			&[("locales/pt.json", "Portuguese")],
			r#"["common"]"#,
		);

		let err = resolve_config(&config).await.unwrap_err();

		assert!(format!("{err:#}").contains("every `file`"));
	}

	#[tokio::test]
	async fn resolve_config_with_namespaces_puts_hash_cache_in_fixed_prefix_directory() {
		let dir = TempDir::new().unwrap();
		let config = write_namespaced_config(
			&dir,
			"locales/en/{namespace}.json",
			&[("locales/pt/{namespace}.json", "Portuguese")],
			r#"["common"]"#,
		);

		let resolved = resolve_config(&config).await.unwrap();

		assert_eq!(
			resolved.options.hash_cache_location,
			dir.path().join("locales").join("en").join(".j18n-cache.ini"),
		);
	}

	#[tokio::test]
	async fn check_succeeds_for_namespaced_config_when_all_namespaces_in_sync() {
		let dir = TempDir::new().unwrap();
		let locales_en = dir.path().join("locales").join("en");
		let locales_pt = dir.path().join("locales").join("pt");

		std::fs::create_dir_all(&locales_en).unwrap();
		std::fs::create_dir_all(&locales_pt).unwrap();
		std::fs::write(locales_en.join("common.json"), r#"{"a": "A"}"#).unwrap();
		std::fs::write(locales_en.join("auth.json"), r#"{"x": "X"}"#).unwrap();
		std::fs::write(locales_pt.join("common.json"), r#"{"a": "AA"}"#).unwrap();
		std::fs::write(locales_pt.join("auth.json"), r#"{"x": "XX"}"#).unwrap();

		let config = write_namespaced_config(
			&dir,
			"locales/en/{namespace}.json",
			&[("locales/pt/{namespace}.json", "Portuguese")],
			r#"["common", "auth"]"#,
		);

		use j18n_io::{content_hash_hex, I18nHashing, I18nHashingStore};
		use std::collections::HashMap;
		let store = I18nHashingStore::at(locales_en.join(".j18n-cache.ini"));
		let mut common_hashes = HashMap::new();
		common_hashes.insert("a".to_string(), content_hash_hex("A"));
		store
			.save(
				"locales/pt/common.json@Portuguese",
				&I18nHashing {
					json_key_to_hash_map: common_hashes,
				},
			)
			.await
			.unwrap();
		let mut auth_hashes = HashMap::new();
		auth_hashes.insert("x".to_string(), content_hash_hex("X"));
		store
			.save(
				"locales/pt/auth.json@Portuguese",
				&I18nHashing {
					json_key_to_hash_map: auth_hashes,
				},
			)
			.await
			.unwrap();

		check(CommandArgs { configs: vec![config] }).await.unwrap();
	}

	#[tokio::test]
	async fn baseline_then_check_passes_for_existing_translations() {
		let dir = TempDir::new().unwrap();
		let config = write_minimal_config(&dir, &["pt"]);

		std::fs::write(dir.path().join("en.json"), r#"{"a": "A", "b": "B"}"#).unwrap();
		std::fs::write(dir.path().join("pt.json"), r#"{"a": "AA", "b": "BB"}"#).unwrap();

		baseline(CommandArgs {
			configs: vec![config.clone()],
		})
		.await
		.unwrap();
		check(CommandArgs { configs: vec![config] }).await.unwrap();
	}

	#[tokio::test]
	async fn baseline_overrides_stale_cache_so_check_passes() {
		let dir = TempDir::new().unwrap();
		let config = write_minimal_config(&dir, &["pt"]);

		std::fs::write(dir.path().join("en.json"), r#"{"a": "A-NEW"}"#).unwrap();
		std::fs::write(dir.path().join("pt.json"), r#"{"a": "AA"}"#).unwrap();
		write_matching_hash_cache(&dir, "pt", &[("a", "A-OLD")]).await;

		baseline(CommandArgs {
			configs: vec![config.clone()],
		})
		.await
		.unwrap();
		check(CommandArgs { configs: vec![config] }).await.unwrap();
	}

	#[tokio::test]
	async fn baseline_then_check_passes_for_namespaced_config() {
		let dir = TempDir::new().unwrap();
		let locales_en = dir.path().join("locales").join("en");
		let locales_pt = dir.path().join("locales").join("pt");

		std::fs::create_dir_all(&locales_en).unwrap();
		std::fs::create_dir_all(&locales_pt).unwrap();
		std::fs::write(locales_en.join("common.json"), r#"{"a": "A"}"#).unwrap();
		std::fs::write(locales_en.join("auth.json"), r#"{"x": "X"}"#).unwrap();
		std::fs::write(locales_pt.join("common.json"), r#"{"a": "AA"}"#).unwrap();
		std::fs::write(locales_pt.join("auth.json"), r#"{"x": "XX"}"#).unwrap();

		let config = write_namespaced_config(
			&dir,
			"locales/en/{namespace}.json",
			&[("locales/pt/{namespace}.json", "Portuguese")],
			r#"["common", "auth"]"#,
		);

		baseline(CommandArgs {
			configs: vec![config.clone()],
		})
		.await
		.unwrap();
		check(CommandArgs { configs: vec![config] }).await.unwrap();
	}

	#[tokio::test]
	async fn check_fails_for_namespaced_config_when_one_namespace_is_out_of_sync() {
		let dir = TempDir::new().unwrap();
		let locales_en = dir.path().join("locales").join("en");
		let locales_pt = dir.path().join("locales").join("pt");

		std::fs::create_dir_all(&locales_en).unwrap();
		std::fs::create_dir_all(&locales_pt).unwrap();
		std::fs::write(locales_en.join("common.json"), r#"{"a": "A"}"#).unwrap();
		std::fs::write(locales_en.join("auth.json"), r#"{"x": "X", "y": "Y"}"#).unwrap();
		std::fs::write(locales_pt.join("common.json"), r#"{"a": "AA"}"#).unwrap();
		std::fs::write(locales_pt.join("auth.json"), r#"{"x": "XX"}"#).unwrap();

		let config = write_namespaced_config(
			&dir,
			"locales/en/{namespace}.json",
			&[("locales/pt/{namespace}.json", "Portuguese")],
			r#"["common", "auth"]"#,
		);

		use j18n_io::{content_hash_hex, I18nHashing, I18nHashingStore};
		use std::collections::HashMap;
		let store = I18nHashingStore::at(locales_en.join(".j18n-cache.ini"));
		let mut common_hashes = HashMap::new();
		common_hashes.insert("a".to_string(), content_hash_hex("A"));
		store
			.save(
				"locales/pt/common.json@Portuguese",
				&I18nHashing {
					json_key_to_hash_map: common_hashes,
				},
			)
			.await
			.unwrap();
		let mut auth_hashes = HashMap::new();
		auth_hashes.insert("x".to_string(), content_hash_hex("X"));
		auth_hashes.insert("y".to_string(), content_hash_hex("Y"));
		store
			.save(
				"locales/pt/auth.json@Portuguese",
				&I18nHashing {
					json_key_to_hash_map: auth_hashes,
				},
			)
			.await
			.unwrap();

		let err = check(CommandArgs { configs: vec![config] }).await.unwrap_err();

		assert!(format!("{err:#}").contains("out of sync"));
	}
}
