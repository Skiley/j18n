use j18n_core::{I18nData, I18nDefinition, J18nError, J18nResult, PathPattern};
use j18n_io::read_i18n_data;
use regex::Regex;
use std::collections::HashMap;
use tracing::{debug, warn};

pub struct TranslationValidator;

impl TranslationValidator {
	pub async fn validate_translations(
		reference_i18n: &I18nDefinition,
		generated_i18ns: &[I18nDefinition],
		exclude_patterns: &[PathPattern],
		interpolation_patterns: &[Regex],
	) -> J18nResult<()> {
		let reference_data = read_i18n_data(reference_i18n, exclude_patterns).await?;

		for generated in generated_i18ns {
			debug!(
				"Validating {} ({}) with {} ({})...",
				generated.language,
				generated.file.display(),
				reference_i18n.language,
				reference_i18n.file.display()
			);

			let generated_data = read_i18n_data(generated, exclude_patterns).await?;

			Self::validate_data(&reference_data, &generated_data, interpolation_patterns)?;
		}

		Ok(())
	}

	pub fn validate_data(
		reference_data: &I18nData,
		generated_data: &I18nData,
		interpolation_patterns: &[Regex],
	) -> J18nResult<()> {
		let generated_lookup: HashMap<&str, &str> = generated_data
			.walked_tree_map
			.iter()
			.map(|(key, value)| (key.as_str(), value.as_str()))
			.collect();

		for (key, reference_value) in &reference_data.walked_tree_map {
			let generated_value = generated_lookup
				.get(key.as_str())
				.ok_or_else(|| J18nError::MissingTranslation { key: key.clone() })?;

			check_interpolations(reference_value, generated_value, interpolation_patterns);
		}

		Ok(())
	}

	pub fn validate_translation(
		reference_values: &[String],
		generated_values: &[String],
		interpolation_patterns: &[Regex],
	) -> J18nResult<()> {
		if reference_values.len() != generated_values.len() {
			return Err(J18nError::validation(format!(
				"reference values size ({}) does not match generated values size ({})",
				reference_values.len(),
				generated_values.len()
			)));
		}

		for (reference, generated) in reference_values.iter().zip(generated_values.iter()) {
			check_interpolations(reference, generated, interpolation_patterns);
		}

		Ok(())
	}
}

fn check_interpolations(reference: &str, generated: &str, interpolation_patterns: &[Regex]) {
	let reference_interpolations = find_interpolations(reference, interpolation_patterns);
	let generated_interpolations = find_interpolations(generated, interpolation_patterns);

	let same_count = reference_interpolations.len() == generated_interpolations.len();
	let contains_all = reference_interpolations
		.iter()
		.all(|i| generated_interpolations.contains(i));

	if !same_count || !contains_all {
		warn!("Wrong interpolations in \"{generated}\" (original: \"{reference}\")");
	}
}

fn find_interpolations(value: &str, interpolation_patterns: &[Regex]) -> Vec<String> {
	let mut matches = Vec::new();

	for pattern in interpolation_patterns {
		for found in pattern.find_iter(value) {
			matches.push(found.as_str().to_string());
		}
	}

	matches
}

#[cfg(test)]
mod tests {
	use super::*;
	use tempfile::TempDir;

	fn handlebars() -> Vec<Regex> {
		vec![Regex::new(r"\{\{(.+?)\}\}").unwrap()]
	}

	fn definition_in(dir: &TempDir, code: &str) -> I18nDefinition {
		let file = dir.path().join(format!("{code}.json"));
		let id = format!("{code}.json");

		I18nDefinition {
			file,
			id,
			language: code.to_string(),
		}
	}

	#[test]
	fn validate_translation_passes_when_counts_match() {
		let result = TranslationValidator::validate_translation(&["a".into()], &["A".into()], &handlebars());

		assert!(result.is_ok());
	}

	#[test]
	fn validate_translation_errors_when_counts_differ() {
		let err = TranslationValidator::validate_translation(&["a".into(), "b".into()], &["A".into()], &handlebars())
			.unwrap_err();

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

		let err = TranslationValidator::validate_data(&reference, &generated, &handlebars()).unwrap_err();

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

		assert!(TranslationValidator::validate_data(&reference, &generated, &handlebars()).is_ok());
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

		assert!(TranslationValidator::validate_data(&reference, &generated, &handlebars()).is_ok());
	}

	#[tokio::test]
	async fn validate_translations_reads_files_and_passes_when_all_keys_present() {
		let dir = TempDir::new().unwrap();
		let reference = definition_in(&dir, "en");
		let generated = definition_in(&dir, "pt");

		tokio::fs::write(&reference.file, r#"{"a": "x"}"#).await.unwrap();
		tokio::fs::write(&generated.file, r#"{"a": "y"}"#).await.unwrap();

		TranslationValidator::validate_translations(&reference, &[generated], &[], &handlebars())
			.await
			.unwrap();
	}

	#[tokio::test]
	async fn validate_translations_errors_when_a_target_is_missing_keys() {
		let dir = TempDir::new().unwrap();
		let reference = definition_in(&dir, "en");
		let generated = definition_in(&dir, "pt");

		tokio::fs::write(&reference.file, r#"{"a": "x", "b": "y"}"#)
			.await
			.unwrap();
		tokio::fs::write(&generated.file, r#"{"a": "z"}"#).await.unwrap();

		let err = TranslationValidator::validate_translations(&reference, &[generated], &[], &handlebars())
			.await
			.unwrap_err();

		match err {
			J18nError::MissingTranslation { key } => assert_eq!(key, "b"),
			other => panic!("unexpected error: {other:?}"),
		}
	}

	#[tokio::test]
	async fn validate_translations_skips_excluded_keys() {
		let dir = TempDir::new().unwrap();
		let reference = definition_in(&dir, "en");
		let generated = definition_in(&dir, "pt");

		tokio::fs::write(&reference.file, r#"{"sample": "X", "real": "Y"}"#)
			.await
			.unwrap();
		tokio::fs::write(&generated.file, r#"{"real": "Z"}"#).await.unwrap();

		let exclude = vec![PathPattern::parse("sample").unwrap()];

		TranslationValidator::validate_translations(&reference, &[generated], &exclude, &handlebars())
			.await
			.unwrap();
	}

	#[test]
	fn find_interpolations_returns_each_match() {
		let interpolations = find_interpolations("hello {{a}} and {{b}}!", &handlebars());

		assert_eq!(interpolations, vec!["{{a}}".to_string(), "{{b}}".to_string()]);
	}

	#[test]
	fn find_interpolations_returns_empty_when_no_matches() {
		assert!(find_interpolations("plain text", &handlebars()).is_empty());
	}

	#[test]
	fn find_interpolations_supports_multiple_patterns() {
		let patterns = vec![Regex::new(r"\{\{(.+?)\}\}").unwrap(), Regex::new(r"%\w+%").unwrap()];

		let interpolations = find_interpolations("Hi {{name}} %SITE%", &patterns);

		assert!(interpolations.contains(&"{{name}}".to_string()));
		assert!(interpolations.contains(&"%SITE%".to_string()));
	}
}
