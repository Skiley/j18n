use async_trait::async_trait;
use j18n_core::{J18nError, J18nResult};
use j18n_translator::I18nTranslator;
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
	additional_prompts: Vec<String>,
	executor: E,
}

impl ClaudeCodeBasedI18nTranslator<DefaultClaudeCodeExecutor> {
	pub const TRANSLATOR_ID: &'static str = "claude-code";

	pub fn new(additional_prompts: Vec<String>) -> Self {
		Self {
			additional_prompts,
			executor: DefaultClaudeCodeExecutor,
		}
	}
}

impl<E: ClaudeCodeExecutor> ClaudeCodeBasedI18nTranslator<E> {
	pub fn with_executor(executor: E) -> Self {
		Self {
			additional_prompts: Vec::new(),
			executor,
		}
	}

	pub fn with_additional_prompts(mut self, additional_prompts: Vec<String>) -> Self {
		self.additional_prompts = additional_prompts;
		self
	}
}

#[async_trait]
impl<E: ClaudeCodeExecutor> I18nTranslator for ClaudeCodeBasedI18nTranslator<E> {
	fn translator_id(&self) -> &str {
		"claude-code"
	}

	async fn translate_values(
		&self,
		from_language: &str,
		to_language: &str,
		values: Vec<String>,
	) -> J18nResult<Vec<String>> {
		let values_for_prompt_serialized = serde_json::to_string(&values)
			.map_err(|e| J18nError::translator(format!("failed to serialize prompt array: {e}")))?;
		let prompt = build_prompt(
			from_language,
			to_language,
			&self.additional_prompts,
			&values_for_prompt_serialized,
		);
		let response = self.executor.execute(&prompt).await?;

		Ok(response
			.split(ENTRY_SEPARATOR)
			.map(|s| s.trim().to_string())
			.filter(|s| !s.is_empty())
			.collect())
	}
}

fn build_prompt(
	from_language: &str,
	to_language: &str,
	additional_prompts: &[String],
	values_for_prompt_serialized: &str,
) -> String {
	let mut lines: Vec<String> = vec![
		format!("Translate the values in the following JSON array, from {from_language} to {to_language}."),
		"DO NOT remove or modify HTML tags.".to_string(),
		"DO NOT remove, skip or modify placeholders, like [1], [2], [3], etc.".to_string(),
	];

	for prompt in additional_prompts {
		lines.push(prompt.clone());
	}

	lines.extend([
		"Once again, DO NOT remove placeholders like '[1]', '[2]', '[3]', '[4]', etc.".to_string(),
		format!(
			"Answer ONLY with the translated values, one per line, each separated by the exact string '{ENTRY_SEPARATOR}' on its own line, in the same order as the inputs."
		),
		format!(
			"Do NOT include any other text, explanations, numbering, or formatting — only the translated values separated by '{ENTRY_SEPARATOR}'."
		),
		"The JSON array of values to translate is:".to_string(),
		values_for_prompt_serialized.to_string(),
	]);

	lines.join("\n")
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

	const ENGLISH: &str = "English";
	const PORTUGUESE: &str = "Portuguese";

	#[tokio::test]
	async fn translates_values_via_separator() {
		let (executor, captured) = MockExecutor::ok("olá<<<SEP>>>mundo");
		let translator = ClaudeCodeBasedI18nTranslator::with_executor(executor);

		let translated = translator
			.translate_values(ENGLISH, PORTUGUESE, vec!["hello".into(), "world".into()])
			.await
			.unwrap();

		assert_eq!(translated, vec!["olá".to_string(), "mundo".to_string()]);
		assert_eq!(captured.lock().unwrap().len(), 1);
	}

	#[tokio::test]
	async fn returns_response_strings_verbatim_after_trim() {
		let (executor, _) = MockExecutor::ok("  Olá [0]!  ");
		let translator = ClaudeCodeBasedI18nTranslator::with_executor(executor);

		let translated = translator
			.translate_values(ENGLISH, PORTUGUESE, vec!["Hello [0]!".into()])
			.await
			.unwrap();

		assert_eq!(translated, vec!["Olá [0]!".to_string()]);
	}

	#[tokio::test]
	async fn propagates_executor_errors() {
		let executor = MockExecutor::err("boom");
		let translator = ClaudeCodeBasedI18nTranslator::with_executor(executor);

		let err = translator
			.translate_values(ENGLISH, PORTUGUESE, vec!["a".into()])
			.await
			.unwrap_err();

		match err {
			J18nError::Translator(message) => assert!(message.contains("boom")),
			other => panic!("unexpected error: {other:?}"),
		}
	}

	#[tokio::test]
	async fn prompt_includes_language_names_and_values() {
		let (executor, captured) = MockExecutor::ok("Olá");
		let translator = ClaudeCodeBasedI18nTranslator::with_executor(executor);

		translator
			.translate_values(ENGLISH, PORTUGUESE, vec!["Hi".into()])
			.await
			.unwrap();

		let prompts = captured.lock().unwrap();
		let prompt = &prompts[0];

		assert!(prompt.contains("from English to Portuguese"));
		assert!(prompt.contains("[\"Hi\"]"));
		assert!(prompt.contains(ENTRY_SEPARATOR));
	}

	#[tokio::test]
	async fn prompt_no_longer_mentions_music_specific_terms() {
		let (executor, captured) = MockExecutor::ok("X");
		let translator = ClaudeCodeBasedI18nTranslator::with_executor(executor);

		translator
			.translate_values(ENGLISH, PORTUGUESE, vec!["x".into()])
			.await
			.unwrap();

		let prompts = captured.lock().unwrap();
		let prompt = &prompts[0];

		for forbidden in ["music", "playlist", "track", "song", "artwork", "touch"] {
			assert!(
				!prompt.to_lowercase().contains(forbidden),
				"prompt contains forbidden term \"{forbidden}\": {prompt}"
			);
		}
	}

	#[test]
	fn translator_id_is_claude_code() {
		let translator = ClaudeCodeBasedI18nTranslator::new(Vec::new());

		assert_eq!(translator.translator_id(), "claude-code");
	}

	#[tokio::test]
	async fn additional_prompts_are_injected_between_placeholder_warnings() {
		let (executor, captured) = MockExecutor::ok("X");
		let translator = ClaudeCodeBasedI18nTranslator::with_executor(executor)
			.with_additional_prompts(vec!["INJECTED-CONTEXT-A".to_string(), "INJECTED-CONTEXT-B".to_string()]);

		translator
			.translate_values(ENGLISH, PORTUGUESE, vec!["x".into()])
			.await
			.unwrap();

		let prompts = captured.lock().unwrap();
		let prompt = &prompts[0];
		let placeholder_position = prompt
			.find("DO NOT remove, skip or modify placeholders")
			.expect("first placeholder warning must be present");
		let injected_a_position = prompt.find("INJECTED-CONTEXT-A").expect("injected line A missing");
		let injected_b_position = prompt.find("INJECTED-CONTEXT-B").expect("injected line B missing");
		let reminder_position = prompt
			.find("Once again, DO NOT remove placeholders")
			.expect("placeholder reminder must be present");

		assert!(placeholder_position < injected_a_position);
		assert!(injected_a_position < injected_b_position);
		assert!(injected_b_position < reminder_position);
	}

	#[tokio::test]
	async fn no_additional_prompts_means_no_extra_lines_in_prompt() {
		let (executor, captured) = MockExecutor::ok("X");
		let translator = ClaudeCodeBasedI18nTranslator::with_executor(executor);

		translator
			.translate_values(ENGLISH, PORTUGUESE, vec!["x".into()])
			.await
			.unwrap();

		let prompts = captured.lock().unwrap();
		let prompt = &prompts[0];

		assert!(prompt.contains("DO NOT remove, skip or modify placeholders"));
		assert!(prompt.contains("Once again, DO NOT remove placeholders"));
	}
}
