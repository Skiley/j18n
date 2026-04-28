use j18n_core::{I18nData, I18nDefinition, J18nError, J18nResult};
use j18n_io::read_i18n_data;
use regex::Regex;
use std::collections::HashMap;
use std::sync::OnceLock;
use tracing::{info, warn};

pub struct TranslationValidator;

impl TranslationValidator {
	pub async fn validate_translations(
		reference_i18n: &I18nDefinition,
		generated_i18ns: &[I18nDefinition],
	) -> J18nResult<()> {
		let reference_data = read_i18n_data(reference_i18n).await?;

		for generated in generated_i18ns {
			info!(
				"Validating {} ({}) with {} ({})...",
				generated.language.language_name(),
				generated.json_file_path.display(),
				reference_i18n.language.language_name(),
				reference_i18n.json_file_path.display()
			);

			let generated_data = read_i18n_data(generated).await?;

			Self::validate_data(&reference_data, &generated_data)?;
		}

		Ok(())
	}

	pub fn validate_data(reference_data: &I18nData, generated_data: &I18nData) -> J18nResult<()> {
		let generated_lookup: HashMap<&str, &str> = generated_data
			.walked_tree_map
			.iter()
			.map(|(key, value)| (key.as_str(), value.as_str()))
			.collect();

		for (key, reference_value) in &reference_data.walked_tree_map {
			let generated_value = generated_lookup
				.get(key.as_str())
				.ok_or_else(|| J18nError::MissingTranslation { key: key.clone() })?;

			check_interpolations(reference_value, generated_value);
		}

		Ok(())
	}

	pub fn validate_translation(reference_values: &[String], generated_values: &[String]) -> J18nResult<()> {
		if reference_values.len() != generated_values.len() {
			return Err(J18nError::validation(format!(
				"reference values size ({}) does not match generated values size ({})",
				reference_values.len(),
				generated_values.len()
			)));
		}

		for (reference, generated) in reference_values.iter().zip(generated_values.iter()) {
			check_interpolations(reference, generated);
		}

		Ok(())
	}
}

fn check_interpolations(reference: &str, generated: &str) {
	let reference_interpolations = find_interpolations(reference);
	let generated_interpolations = find_interpolations(generated);

	let same_count = reference_interpolations.len() == generated_interpolations.len();
	let contains_all = reference_interpolations
		.iter()
		.all(|i| generated_interpolations.contains(i));

	if !same_count || !contains_all {
		warn!("Wrong interpolations in \"{generated}\" (original: \"{reference}\")");
	}
}

fn find_interpolations(value: &str) -> Vec<String> {
	interpolations_regex()
		.find_iter(value)
		.map(|m| m.as_str().to_string())
		.collect()
}

fn interpolations_regex() -> &'static Regex {
	static INSTANCE: OnceLock<Regex> = OnceLock::new();

	INSTANCE.get_or_init(|| Regex::new(r"\{\{(.+?)\}\}").expect("valid interpolation regex"))
}

#[cfg(test)]
mod tests {
	use super::*;
	use j18n_core::Language;
	use tempfile::TempDir;

	fn definition_in(dir: &TempDir, code: &str) -> I18nDefinition {
		I18nDefinition::from_base_dir(dir.path(), Language::from_iso_639_code(code).unwrap())
	}

	#[test]
	fn validate_translation_passes_when_counts_match() {
		let result = TranslationValidator::validate_translation(&["a".into()], &["A".into()]);

		assert!(result.is_ok());
	}

	#[test]
	fn validate_translation_errors_when_counts_differ() {
		let err = TranslationValidator::validate_translation(&["a".into(), "b".into()], &["A".into()]).unwrap_err();

		assert!(matches!(err, J18nError::Validation(_)));
	}

	#[test]
	fn validate_data_returns_missing_translation_for_each_missing_key() {
		let reference = I18nData {
			json_dict: Default::default(),
			walked_tree_map: vec![("greeting".into(), "Hi".into())],
		};
		let generated = I18nData {
			json_dict: Default::default(),
			walked_tree_map: vec![],
		};

		let err = TranslationValidator::validate_data(&reference, &generated).unwrap_err();

		match err {
			J18nError::MissingTranslation { key } => assert_eq!(key, "greeting"),
			other => panic!("unexpected error: {other:?}"),
		}
	}

	#[test]
	fn validate_data_passes_when_all_keys_present() {
		let reference = I18nData {
			json_dict: Default::default(),
			walked_tree_map: vec![("greeting".into(), "Hi".into())],
		};
		let generated = I18nData {
			json_dict: Default::default(),
			walked_tree_map: vec![("greeting".into(), "Olá".into())],
		};

		assert!(TranslationValidator::validate_data(&reference, &generated).is_ok());
	}

	#[test]
	fn validate_data_does_not_care_about_interpolation_drift_for_correctness_only_warns() {
		let reference = I18nData {
			json_dict: Default::default(),
			walked_tree_map: vec![("greeting".into(), "Hi {{name}}".into())],
		};
		let generated = I18nData {
			json_dict: Default::default(),
			walked_tree_map: vec![("greeting".into(), "Olá".into())],
		};

		assert!(TranslationValidator::validate_data(&reference, &generated).is_ok());
	}

	#[tokio::test]
	async fn validate_translations_reads_files_and_passes_when_all_keys_present() {
		let dir = TempDir::new().unwrap();
		let reference = definition_in(&dir, "en");
		let generated = definition_in(&dir, "pt");

		tokio::fs::write(&reference.json_file_path, r#"{"a": "x"}"#).await.unwrap();
		tokio::fs::write(&generated.json_file_path, r#"{"a": "y"}"#).await.unwrap();

		TranslationValidator::validate_translations(&reference, &[generated]).await.unwrap();
	}

	#[tokio::test]
	async fn validate_translations_errors_when_a_target_is_missing_keys() {
		let dir = TempDir::new().unwrap();
		let reference = definition_in(&dir, "en");
		let generated = definition_in(&dir, "pt");

		tokio::fs::write(&reference.json_file_path, r#"{"a": "x", "b": "y"}"#)
			.await
			.unwrap();
		tokio::fs::write(&generated.json_file_path, r#"{"a": "z"}"#).await.unwrap();

		let err = TranslationValidator::validate_translations(&reference, &[generated])
			.await
			.unwrap_err();

		match err {
			J18nError::MissingTranslation { key } => assert_eq!(key, "b"),
			other => panic!("unexpected error: {other:?}"),
		}
	}

	#[test]
	fn find_interpolations_returns_each_match() {
		let interpolations = find_interpolations("hello {{a}} and {{b}}!");

		assert_eq!(interpolations, vec!["{{a}}".to_string(), "{{b}}".to_string()]);
	}

	#[test]
	fn find_interpolations_returns_empty_when_no_matches() {
		assert!(find_interpolations("plain text").is_empty());
	}
}
