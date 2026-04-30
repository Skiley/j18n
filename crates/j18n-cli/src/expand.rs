use anyhow::{Context, Result};
use std::path::{Component, Path, PathBuf};

pub const NAMESPACE_TOKEN: &str = "{namespace}";

/// One translation unit produced by expanding a config: a single reference
/// definition paired with its target definitions, optionally tagged with the
/// namespace name it represents.
#[derive(Clone, Debug)]
pub struct ExpandedRun {
	pub namespace: Option<String>,
	pub reference_file: String,
	pub target_files: Vec<String>,
}

/// Validates that `{namespace}` is used consistently across the reference and
/// target file templates given the namespaces configuration. Returns an error
/// describing the inconsistency, if any.
pub fn validate_token_consistency(
	has_namespaces_field: bool,
	reference_file: &str,
	target_files: &[&str],
) -> Result<()> {
	let reference_has_token = reference_file.contains(NAMESPACE_TOKEN);
	let any_target_has_token = target_files.iter().any(|file| file.contains(NAMESPACE_TOKEN));
	let all_targets_have_token = target_files.iter().all(|file| file.contains(NAMESPACE_TOKEN));
	let any_token = reference_has_token || any_target_has_token;
	let all_tokens = reference_has_token && all_targets_have_token;

	if has_namespaces_field {
		if !all_tokens {
			anyhow::bail!(
				"`namespaces` is set, but every `file` (reference and targets) must contain \"{NAMESPACE_TOKEN}\""
			);
		}
	} else if any_token {
		anyhow::bail!("`file` contains \"{NAMESPACE_TOKEN}\" but `namespaces` is not set");
	}

	for path in std::iter::once(reference_file).chain(target_files.iter().copied()) {
		if path.matches(NAMESPACE_TOKEN).count() > 1 {
			anyhow::bail!("\"{path}\" contains multiple \"{NAMESPACE_TOKEN}\" tokens; only one is allowed per path");
		}
	}

	Ok(())
}

pub fn substitute_namespace(template: &str, namespace: &str) -> String {
	template.replace(NAMESPACE_TOKEN, namespace)
}

/// Validates an explicit namespace name. Forbids empty names, names containing
/// path separators, or names containing the namespace token.
pub fn validate_namespace_name(name: &str) -> Result<()> {
	if name.is_empty() {
		anyhow::bail!("namespace name must not be empty");
	}

	if name.contains('/') || name.contains('\\') {
		anyhow::bail!("namespace name \"{name}\" must not contain path separators");
	}

	if name.contains(NAMESPACE_TOKEN) {
		anyhow::bail!("namespace name \"{name}\" must not contain \"{NAMESPACE_TOKEN}\"");
	}

	Ok(())
}

/// Expands a (reference, targets) template pair against an explicit list of
/// namespace names, returning one [`ExpandedRun`] per namespace.
pub fn expand_with_list(
	reference_file: &str,
	target_files: &[String],
	namespaces: &[String],
) -> Result<Vec<ExpandedRun>> {
	if namespaces.is_empty() {
		anyhow::bail!("`namespaces` list is empty; provide at least one name or use \"*\" for wildcard discovery");
	}

	for name in namespaces {
		validate_namespace_name(name).with_context(|| format!("invalid namespace name \"{name}\""))?;
	}

	let mut runs: Vec<ExpandedRun> = Vec::with_capacity(namespaces.len());

	for namespace in namespaces {
		runs.push(ExpandedRun {
			namespace: Some(namespace.clone()),
			reference_file: substitute_namespace(reference_file, namespace),
			target_files: target_files
				.iter()
				.map(|file| substitute_namespace(file, namespace))
				.collect(),
		});
	}

	Ok(runs)
}

/// Locates the `{namespace}` token within a resolved reference template,
/// supporting tokens in the basename (`locales/en/{namespace}.json`) and in
/// directory components (`features/{namespace}/i18n/en.json`).
pub struct NamespaceTokenPosition {
	pub discovery_directory: PathBuf,
	pub component_prefix: String,
	pub component_suffix: String,
	pub token_in_basename: bool,
}

pub fn parse_namespace_token_position(resolved_reference_template: &Path) -> Result<NamespaceTokenPosition> {
	let display = resolved_reference_template.to_string_lossy().to_string();
	let token_position = display.find(NAMESPACE_TOKEN).with_context(|| {
		format!(
			"reference path \"{}\" must contain \"{NAMESPACE_TOKEN}\" for wildcard discovery",
			resolved_reference_template.display()
		)
	})?;
	let before_token = &display[..token_position];
	let after_token = &display[token_position + NAMESPACE_TOKEN.len()..];
	let component_start_in_before = before_token
		.rfind(['/', '\\'])
		.map(|index| index + 1)
		.unwrap_or(0);
	let component_prefix = before_token[component_start_in_before..].to_string();
	let component_end_in_after = after_token.find(['/', '\\']).unwrap_or(after_token.len());
	let component_suffix = after_token[..component_end_in_after].to_string();
	let token_in_basename = component_end_in_after == after_token.len();
	let discovery_directory_string = if component_start_in_before == 0 {
		"."
	} else {
		&before_token[..component_start_in_before - 1]
	};
	let discovery_directory = if discovery_directory_string.is_empty() {
		PathBuf::from(".")
	} else {
		PathBuf::from(discovery_directory_string)
	};

	Ok(NamespaceTokenPosition {
		discovery_directory,
		component_prefix,
		component_suffix,
		token_in_basename,
	})
}

/// Lists entries in the directory containing the `{namespace}` component of
/// the reference template, extracts candidate namespace names, and confirms
/// the full substituted reference path exists for each candidate.
///
/// Works for tokens in the basename (entries must be files matching the
/// pattern) and tokens in directory components (entries must be directories
/// whose substitution produces a real file at the full reference path). Hidden
/// entries (name starting with `.`) are skipped so the cache file and other
/// dotfiles are never mistaken for a namespace.
pub async fn discover_namespaces_from_reference(resolved_reference_template: &Path) -> Result<Vec<String>> {
	let position = parse_namespace_token_position(resolved_reference_template)?;

	if !tokio::fs::try_exists(&position.discovery_directory)
		.await
		.with_context(|| format!("failed to stat \"{}\"", position.discovery_directory.display()))?
	{
		anyhow::bail!(
			"cannot discover namespaces: directory \"{}\" does not exist",
			position.discovery_directory.display()
		);
	}

	let mut entries = tokio::fs::read_dir(&position.discovery_directory)
		.await
		.with_context(|| {
			format!(
				"failed to list \"{}\" for namespace discovery",
				position.discovery_directory.display()
			)
		})?;
	let template_string = resolved_reference_template.to_string_lossy().to_string();
	let mut namespaces: Vec<String> = Vec::new();

	while let Some(entry) = entries.next_entry().await.with_context(|| {
		format!(
			"failed to read entry in \"{}\"",
			position.discovery_directory.display()
		)
	})? {
		let file_type = entry
			.file_type()
			.await
			.with_context(|| format!("failed to stat \"{}\"", entry.path().display()))?;
		let expected_match = if position.token_in_basename {
			file_type.is_file()
		} else {
			file_type.is_dir()
		};

		if !expected_match {
			continue;
		}

		let file_name = entry.file_name();
		let Some(file_name) = file_name.to_str() else {
			continue;
		};

		if file_name.starts_with('.') {
			continue;
		}

		let Some(stripped) = file_name.strip_prefix(&position.component_prefix) else {
			continue;
		};
		let Some(namespace) = stripped.strip_suffix(&position.component_suffix) else {
			continue;
		};

		if namespace.is_empty() {
			continue;
		}

		let candidate_path = PathBuf::from(substitute_namespace(&template_string, namespace));

		if !tokio::fs::try_exists(&candidate_path)
			.await
			.unwrap_or(false)
		{
			continue;
		}

		namespaces.push(namespace.to_string());
	}

	if namespaces.is_empty() {
		anyhow::bail!(
			"no namespaces discovered in \"{}\" matching pattern \"{}{NAMESPACE_TOKEN}{}\"",
			position.discovery_directory.display(),
			position.component_prefix,
			position.component_suffix
		);
	}

	namespaces.sort();
	namespaces.dedup();

	Ok(namespaces)
}

/// Resolves the default hash-cache directory for a namespaced reference
/// template: the deepest path prefix that does not depend on `{namespace}`.
///
/// For `locales/en/{namespace}.json` → `locales/en/`. For
/// `features/{namespace}/i18n.en.json` → `features/`. For `{namespace}.json`
/// (no directory) → `.`.
pub fn fixed_prefix_directory(resolved_template: &Path) -> PathBuf {
	let display = resolved_template.to_string_lossy();
	let token_position = match display.find(NAMESPACE_TOKEN) {
		Some(position) => position,
		None => {
			return resolved_template
				.parent()
				.map(Path::to_path_buf)
				.unwrap_or_else(|| PathBuf::from("."));
		}
	};
	let before_token = &display[..token_position];
	let last_separator = before_token.rfind(['/', '\\']);
	let fixed_prefix = match last_separator {
		Some(index) => &before_token[..index],
		None => "",
	};
	let path = Path::new(fixed_prefix);

	if path.as_os_str().is_empty() {
		PathBuf::from(".")
	} else {
		// Normalize trailing separators by walking components.
		let mut normalized = PathBuf::new();

		for component in path.components() {
			match component {
				Component::CurDir => {}
				other => normalized.push(other.as_os_str()),
			}
		}

		if normalized.as_os_str().is_empty() {
			PathBuf::from(".")
		} else {
			normalized
		}
	}
}

#[cfg(test)]
mod tests {
	use super::*;
	use tempfile::TempDir;

	#[test]
	fn validate_token_consistency_passes_when_all_paths_use_token_and_namespaces_set() {
		let result = validate_token_consistency(
			true,
			"locales/en/{namespace}.json",
			&["locales/pt/{namespace}.json", "locales/es/{namespace}.json"],
		);

		assert!(result.is_ok());
	}

	#[test]
	fn validate_token_consistency_passes_when_no_paths_use_token_and_namespaces_unset() {
		let result = validate_token_consistency(false, "locales/en.json", &["locales/pt.json", "locales/es.json"]);

		assert!(result.is_ok());
	}

	#[test]
	fn validate_token_consistency_errors_when_namespaces_set_but_reference_lacks_token() {
		let result = validate_token_consistency(true, "locales/en.json", &["locales/pt/{namespace}.json"]);
		let err = result.unwrap_err();

		assert!(
			format!("{err:#}").contains("every `file`"),
			"unexpected error message: {err:#}"
		);
	}

	#[test]
	fn validate_token_consistency_errors_when_namespaces_set_but_one_target_lacks_token() {
		let result = validate_token_consistency(
			true,
			"locales/en/{namespace}.json",
			&["locales/pt/{namespace}.json", "locales/es.json"],
		);

		assert!(result.is_err());
	}

	#[test]
	fn validate_token_consistency_errors_when_token_used_without_namespaces_field() {
		let result = validate_token_consistency(false, "locales/en/{namespace}.json", &["locales/pt/{namespace}.json"]);
		let err = result.unwrap_err();

		assert!(format!("{err:#}").contains("`namespaces` is not set"));
	}

	#[test]
	fn validate_token_consistency_errors_when_token_appears_more_than_once() {
		let result = validate_token_consistency(true, "locales/{namespace}/{namespace}.json", &[]);
		let err = result.unwrap_err();

		assert!(format!("{err:#}").contains("multiple"));
	}

	#[test]
	fn substitute_namespace_replaces_token() {
		assert_eq!(
			substitute_namespace("locales/{namespace}.json", "common"),
			"locales/common.json"
		);
	}

	#[test]
	fn substitute_namespace_replaces_token_in_directory_components_too() {
		assert_eq!(
			substitute_namespace("features/{namespace}/i18n.en.json", "checkout"),
			"features/checkout/i18n.en.json"
		);
	}

	#[test]
	fn validate_namespace_name_accepts_simple_names() {
		assert!(validate_namespace_name("common").is_ok());
		assert!(validate_namespace_name("auth-flow").is_ok());
		assert!(validate_namespace_name("a.b").is_ok());
	}

	#[test]
	fn validate_namespace_name_rejects_empty_or_separator_or_token() {
		assert!(validate_namespace_name("").is_err());
		assert!(validate_namespace_name("a/b").is_err());
		assert!(validate_namespace_name("a\\b").is_err());
		assert!(validate_namespace_name("{namespace}").is_err());
	}

	#[test]
	fn expand_with_list_substitutes_each_namespace_into_each_path() {
		let runs = expand_with_list(
			"locales/en/{namespace}.json",
			&[
				"locales/pt/{namespace}.json".to_string(),
				"locales/es/{namespace}.json".to_string(),
			],
			&["common".to_string(), "auth".to_string()],
		)
		.unwrap();

		assert_eq!(runs.len(), 2);
		assert_eq!(runs[0].namespace.as_deref(), Some("common"));
		assert_eq!(runs[0].reference_file, "locales/en/common.json");
		assert_eq!(runs[0].target_files, vec!["locales/pt/common.json", "locales/es/common.json"]);
		assert_eq!(runs[1].namespace.as_deref(), Some("auth"));
		assert_eq!(runs[1].reference_file, "locales/en/auth.json");
	}

	#[test]
	fn expand_with_list_errors_on_empty_namespaces_list() {
		let result = expand_with_list(
			"locales/en/{namespace}.json",
			&["locales/pt/{namespace}.json".to_string()],
			&[],
		);
		let err = result.unwrap_err();

		assert!(format!("{err:#}").contains("empty"));
	}

	#[test]
	fn expand_with_list_errors_on_invalid_namespace_name() {
		let result = expand_with_list(
			"locales/en/{namespace}.json",
			&[],
			&["bad/name".to_string()],
		);

		assert!(result.is_err());
	}

	#[test]
	fn parse_namespace_token_position_handles_token_in_basename() {
		let position = parse_namespace_token_position(Path::new("/abs/locales/en/{namespace}.json")).unwrap();

		assert_eq!(position.discovery_directory, PathBuf::from("/abs/locales/en"));
		assert_eq!(position.component_prefix, "");
		assert_eq!(position.component_suffix, ".json");
		assert!(position.token_in_basename);
	}

	#[test]
	fn parse_namespace_token_position_handles_prefix_and_suffix_around_token_in_basename() {
		let position = parse_namespace_token_position(Path::new("locales/en/i18n-{namespace}-bundle.json")).unwrap();

		assert_eq!(position.component_prefix, "i18n-");
		assert_eq!(position.component_suffix, "-bundle.json");
		assert!(position.token_in_basename);
	}

	#[test]
	fn parse_namespace_token_position_handles_token_in_directory_component() {
		let position = parse_namespace_token_position(Path::new("features/{namespace}/i18n/en.json")).unwrap();

		assert_eq!(position.discovery_directory, PathBuf::from("features"));
		assert_eq!(position.component_prefix, "");
		assert_eq!(position.component_suffix, "");
		assert!(!position.token_in_basename);
	}

	#[test]
	fn parse_namespace_token_position_handles_prefix_and_suffix_around_token_in_directory() {
		let position = parse_namespace_token_position(Path::new("features/feat-{namespace}-mod/en.json")).unwrap();

		assert_eq!(position.discovery_directory, PathBuf::from("features"));
		assert_eq!(position.component_prefix, "feat-");
		assert_eq!(position.component_suffix, "-mod");
		assert!(!position.token_in_basename);
	}

	#[test]
	fn parse_namespace_token_position_handles_token_at_root_basename() {
		let position = parse_namespace_token_position(Path::new("{namespace}.json")).unwrap();

		assert_eq!(position.discovery_directory, PathBuf::from("."));
		assert_eq!(position.component_prefix, "");
		assert_eq!(position.component_suffix, ".json");
		assert!(position.token_in_basename);
	}

	#[tokio::test]
	async fn discover_namespaces_from_reference_lists_matching_files_in_parent_dir() {
		let dir = TempDir::new().unwrap();
		let locales_en = dir.path().join("locales").join("en");

		tokio::fs::create_dir_all(&locales_en).await.unwrap();
		tokio::fs::write(locales_en.join("common.json"), "{}").await.unwrap();
		tokio::fs::write(locales_en.join("auth.json"), "{}").await.unwrap();
		tokio::fs::write(locales_en.join("checkout.json"), "{}").await.unwrap();

		let template = locales_en.join("{namespace}.json");
		let namespaces = discover_namespaces_from_reference(&template).await.unwrap();

		assert_eq!(namespaces, vec!["auth", "checkout", "common"]);
	}

	#[tokio::test]
	async fn discover_namespaces_skips_dotfiles() {
		let dir = TempDir::new().unwrap();
		let locales_en = dir.path().join("locales").join("en");

		tokio::fs::create_dir_all(&locales_en).await.unwrap();
		tokio::fs::write(locales_en.join("common.json"), "{}").await.unwrap();
		tokio::fs::write(locales_en.join(".hash-cache.json"), "{}").await.unwrap();

		let template = locales_en.join("{namespace}.json");
		let namespaces = discover_namespaces_from_reference(&template).await.unwrap();

		assert_eq!(namespaces, vec!["common"]);
	}

	#[tokio::test]
	async fn discover_namespaces_respects_prefix_and_suffix_around_token() {
		let dir = TempDir::new().unwrap();
		let locales_en = dir.path().join("locales").join("en");

		tokio::fs::create_dir_all(&locales_en).await.unwrap();
		tokio::fs::write(locales_en.join("i18n-common-bundle.json"), "{}").await.unwrap();
		tokio::fs::write(locales_en.join("i18n-auth-bundle.json"), "{}").await.unwrap();
		tokio::fs::write(locales_en.join("README.md"), "ignored").await.unwrap();
		tokio::fs::write(locales_en.join("i18n-broken.json"), "ignored").await.unwrap();

		let template = locales_en.join("i18n-{namespace}-bundle.json");
		let namespaces = discover_namespaces_from_reference(&template).await.unwrap();

		assert_eq!(namespaces, vec!["auth", "common"]);
	}

	#[tokio::test]
	async fn discover_namespaces_handles_token_in_directory_component() {
		let dir = TempDir::new().unwrap();
		let features = dir.path().join("features");

		tokio::fs::create_dir_all(features.join("auth").join("i18n"))
			.await
			.unwrap();
		tokio::fs::create_dir_all(features.join("checkout").join("i18n"))
			.await
			.unwrap();
		tokio::fs::write(features.join("auth").join("i18n").join("en.json"), "{}")
			.await
			.unwrap();
		tokio::fs::write(features.join("checkout").join("i18n").join("en.json"), "{}")
			.await
			.unwrap();

		let template = features.join("{namespace}").join("i18n").join("en.json");
		let namespaces = discover_namespaces_from_reference(&template).await.unwrap();

		assert_eq!(namespaces, vec!["auth", "checkout"]);
	}

	#[tokio::test]
	async fn discover_namespaces_skips_directory_candidates_without_full_path() {
		let dir = TempDir::new().unwrap();
		let features = dir.path().join("features");

		tokio::fs::create_dir_all(features.join("with-i18n").join("i18n"))
			.await
			.unwrap();
		tokio::fs::create_dir_all(features.join("no-i18n"))
			.await
			.unwrap();
		tokio::fs::write(features.join("with-i18n").join("i18n").join("en.json"), "{}")
			.await
			.unwrap();

		let template = features.join("{namespace}").join("i18n").join("en.json");
		let namespaces = discover_namespaces_from_reference(&template).await.unwrap();

		assert_eq!(namespaces, vec!["with-i18n"]);
	}

	#[tokio::test]
	async fn discover_namespaces_errors_when_directory_missing() {
		let dir = TempDir::new().unwrap();
		let template = dir.path().join("does-not-exist").join("{namespace}.json");
		let result = discover_namespaces_from_reference(&template).await;

		assert!(result.is_err());
	}

	#[tokio::test]
	async fn discover_namespaces_errors_when_no_matches_found() {
		let dir = TempDir::new().unwrap();
		let locales_en = dir.path().join("locales").join("en");

		tokio::fs::create_dir_all(&locales_en).await.unwrap();
		tokio::fs::write(locales_en.join("README.md"), "ignored").await.unwrap();

		let template = locales_en.join("{namespace}.json");
		let result = discover_namespaces_from_reference(&template).await;

		assert!(result.is_err());
	}

	#[test]
	fn fixed_prefix_directory_for_token_in_basename_is_parent() {
		assert_eq!(
			fixed_prefix_directory(Path::new("locales/en/{namespace}.json")),
			PathBuf::from("locales/en"),
		);
	}

	#[test]
	fn fixed_prefix_directory_for_token_in_directory_component_is_grandparent() {
		assert_eq!(
			fixed_prefix_directory(Path::new("features/{namespace}/i18n.en.json")),
			PathBuf::from("features"),
		);
	}

	#[test]
	fn fixed_prefix_directory_falls_back_to_dot_when_no_directory_prefix() {
		assert_eq!(fixed_prefix_directory(Path::new("{namespace}.json")), PathBuf::from("."));
	}

	#[test]
	fn fixed_prefix_directory_for_path_without_token_is_parent() {
		assert_eq!(
			fixed_prefix_directory(Path::new("locales/en.json")),
			PathBuf::from("locales"),
		);
	}
}
