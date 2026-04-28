use async_trait::async_trait;
use j18n_core::{J18nError, J18nResult, Language};
use j18n_translator::{create_extrapolated_values, restore_extrapolated_values, ExtrapolatedValue, I18nTranslator};
use std::process::Stdio;
use tokio::io::AsyncWriteExt;
use tokio::process::Command;

const ENTRY_SEPARATOR: &str = "<<<SEP>>>";

#[async_trait]
pub trait ClaudeCodeExecutor: Send + Sync {
	async fn execute(&self, prompt: &str) -> J18nResult<String>;
}

pub struct DefaultClaudeCodeExecutor;

#[async_trait]
impl ClaudeCodeExecutor for DefaultClaudeCodeExecutor {
	async fn execute(&self, prompt: &str) -> J18nResult<String> {
		execute_claude_code(prompt).await
	}
}

pub struct ClaudeCodeBasedI18nTranslator<E: ClaudeCodeExecutor = DefaultClaudeCodeExecutor> {
	executor: E,
}

impl ClaudeCodeBasedI18nTranslator<DefaultClaudeCodeExecutor> {
	pub const TRANSLATOR_ID: &'static str = "claude-code";

	pub fn new() -> Self {
		Self {
			executor: DefaultClaudeCodeExecutor,
		}
	}
}

impl<E: ClaudeCodeExecutor> ClaudeCodeBasedI18nTranslator<E> {
	pub fn with_executor(executor: E) -> Self {
		Self { executor }
	}
}

impl Default for ClaudeCodeBasedI18nTranslator<DefaultClaudeCodeExecutor> {
	fn default() -> Self {
		Self::new()
	}
}

#[async_trait]
impl<E: ClaudeCodeExecutor> I18nTranslator for ClaudeCodeBasedI18nTranslator<E> {
	fn translator_id(&self) -> &str {
		"claude-code"
	}

	async fn translate_i18n_values(
		&self,
		from: Language,
		to: Language,
		values: Vec<String>,
	) -> J18nResult<Vec<String>> {
		let extrapolated_values = create_extrapolated_values(&values);
		let translated_values = translate_extrapolated_values(&self.executor, &extrapolated_values, from, to).await?;

		restore_extrapolated_values(&extrapolated_values, &translated_values)
	}
}

async fn translate_extrapolated_values<E: ClaudeCodeExecutor>(
	executor: &E,
	extrapolated_values: &[ExtrapolatedValue],
	from: Language,
	to: Language,
) -> J18nResult<Vec<String>> {
	let extrapolated_for_prompt: Vec<&str> = extrapolated_values
		.iter()
		.map(|v| v.extrapolated_value.as_str())
		.collect();
	let values_for_prompt_serialized = serde_json::to_string(&extrapolated_for_prompt)
		.map_err(|e| J18nError::translator(format!("failed to serialize prompt array: {e}")))?;
	let prompt = build_prompt(from, to, &values_for_prompt_serialized);
	let response = executor.execute(&prompt).await?;

	Ok(response
		.split(ENTRY_SEPARATOR)
		.map(|s| s.trim().to_string())
		.filter(|s| !s.is_empty())
		.collect())
}

fn build_prompt(from: Language, to: Language, values_for_prompt_serialized: &str) -> String {
	[
		format!(
			"Translate the values in the following JSON array, from {} to {}.",
			from.language_name(),
			to.language_name()
		),
		"Consider that the context for the translation is a music streaming app.".to_string(),
		"DO NOT remove or modify HTML tags.".to_string(),
		"DO NOT remove, skip or modify placeholders, like [1], [2], [3], etc.".to_string(),
		"DO NOT translate the words 'artwork', 'feedback', 'playlist' and 'playlists'.".to_string(),
		"DO NOT translate the words 'touch', 'touch name', or anything else that might resemble a click or touch."
			.to_string(),
		"The word 'track' should be interpreted as 'song' when translating it.".to_string(),
		"Once again, DO NOT remove placeholders like '[1]', '[2]', '[3]', '[4]', etc.".to_string(),
		format!(
			"Answer ONLY with the translated values, one per line, each separated by the exact string '{ENTRY_SEPARATOR}' on its own line, in the same order as the inputs."
		),
		format!(
			"Do NOT include any other text, explanations, numbering, or formatting — only the translated values separated by '{ENTRY_SEPARATOR}'."
		),
		"The JSON array of values to translate is:".to_string(),
		values_for_prompt_serialized.to_string(),
	]
	.join("\n")
}

async fn execute_claude_code(prompt: &str) -> J18nResult<String> {
	let mut command = if cfg!(target_os = "windows") {
		let mut command = Command::new("cmd");

		command.args(["/C", "claude", "--model=opus", "-p"]);
		command
	} else {
		let mut command = Command::new("claude");

		command.args(["--model=opus", "-p"]);
		command
	};

	command
		.stdin(Stdio::piped())
		.stdout(Stdio::piped())
		.stderr(Stdio::piped());

	let mut child = command
		.spawn()
		.map_err(|e| J18nError::translator(format!("failed to spawn Claude Code process: {e}")))?;

	if let Some(stdin) = child.stdin.as_mut() {
		stdin
			.write_all(prompt.as_bytes())
			.await
			.map_err(|e| J18nError::translator(format!("failed to write prompt to Claude Code: {e}")))?;
		stdin
			.shutdown()
			.await
			.map_err(|e| J18nError::translator(format!("failed to close Claude Code stdin: {e}")))?;
	}

	let output = child
		.wait_with_output()
		.await
		.map_err(|e| J18nError::translator(format!("failed to wait for Claude Code: {e}")))?;

	if !output.status.success() {
		let stderr = String::from_utf8_lossy(&output.stderr).trim().to_string();
		let exit_code = output
			.status
			.code()
			.map(|c| c.to_string())
			.unwrap_or_else(|| "<signal>".to_string());

		return Err(J18nError::translator(format!(
			"Claude Code process exited with code {exit_code}. Stderr: {stderr}"
		)));
	}

	Ok(String::from_utf8_lossy(&output.stdout).trim().to_string())
}

#[cfg(test)]
mod tests {
	use super::*;
	use std::sync::{Arc, Mutex};

	struct MockExecutor {
		captured: Arc<Mutex<Vec<String>>>,
		response: J18nResult<String>,
	}

	impl MockExecutor {
		fn ok(response: impl Into<String>) -> (Self, Arc<Mutex<Vec<String>>>) {
			let captured = Arc::new(Mutex::new(Vec::new()));

			(
				Self {
					captured: Arc::clone(&captured),
					response: Ok(response.into()),
				},
				captured,
			)
		}

		fn err(message: impl Into<String>) -> Self {
			Self {
				captured: Arc::new(Mutex::new(Vec::new())),
				response: Err(J18nError::translator(message.into())),
			}
		}
	}

	#[async_trait]
	impl ClaudeCodeExecutor for MockExecutor {
		async fn execute(&self, prompt: &str) -> J18nResult<String> {
			self.captured.lock().unwrap().push(prompt.to_string());

			match &self.response {
				Ok(value) => Ok(value.clone()),
				Err(J18nError::Translator(message)) => Err(J18nError::translator(message.clone())),
				Err(_) => Err(J18nError::translator("mock executor failure")),
			}
		}
	}

	fn portuguese() -> Language {
		Language::from_iso_639_code("pt").unwrap()
	}

	#[tokio::test]
	async fn translates_simple_values_via_separator() {
		let (executor, captured) = MockExecutor::ok("olá<<<SEP>>>mundo");
		let translator = ClaudeCodeBasedI18nTranslator::with_executor(executor);

		let translated = translator
			.translate_i18n_values(
				Language::ENGLISH,
				portuguese(),
				vec!["hello".into(), "world".into()],
			)
			.await
			.unwrap();

		assert_eq!(translated, vec!["olá".to_string(), "mundo".to_string()]);
		assert_eq!(captured.lock().unwrap().len(), 1);
	}

	#[tokio::test]
	async fn restores_interpolations_after_translation() {
		let (executor, _) = MockExecutor::ok("Olá [0]!");
		let translator = ClaudeCodeBasedI18nTranslator::with_executor(executor);

		let translated = translator
			.translate_i18n_values(Language::ENGLISH, portuguese(), vec!["Hello {{name}}!".into()])
			.await
			.unwrap();

		assert_eq!(translated, vec!["Olá {{name}}!".to_string()]);
	}

	#[tokio::test]
	async fn fails_when_response_count_does_not_match_input() {
		let (executor, _) = MockExecutor::ok("only-one");
		let translator = ClaudeCodeBasedI18nTranslator::with_executor(executor);

		let err = translator
			.translate_i18n_values(
				Language::ENGLISH,
				portuguese(),
				vec!["a".into(), "b".into()],
			)
			.await
			.unwrap_err();

		assert!(matches!(err, J18nError::Translator(_)));
	}

	#[tokio::test]
	async fn propagates_executor_errors() {
		let executor = MockExecutor::err("boom");
		let translator = ClaudeCodeBasedI18nTranslator::with_executor(executor);

		let err = translator
			.translate_i18n_values(Language::ENGLISH, portuguese(), vec!["a".into()])
			.await
			.unwrap_err();

		match err {
			J18nError::Translator(message) => assert!(message.contains("boom")),
			other => panic!("unexpected error: {other:?}"),
		}
	}

	#[tokio::test]
	async fn prompt_includes_language_names_and_extrapolated_values() {
		let (executor, captured) = MockExecutor::ok("Olá [0]");
		let translator = ClaudeCodeBasedI18nTranslator::with_executor(executor);

		translator
			.translate_i18n_values(Language::ENGLISH, portuguese(), vec!["Hi {{name}}".into()])
			.await
			.unwrap();

		let prompts = captured.lock().unwrap();
		let prompt = &prompts[0];

		assert!(prompt.contains("from English to Portuguese"));
		assert!(prompt.contains("[\"Hi [0]\"]"));
		assert!(prompt.contains(ENTRY_SEPARATOR));
	}

	#[test]
	fn translator_id_is_claude_code() {
		let translator = ClaudeCodeBasedI18nTranslator::new();

		assert_eq!(translator.translator_id(), "claude-code");
	}
}
