use crate::model::{AnthropicMessage, MessagesRequest, MessagesResponse};
use async_trait::async_trait;
use j18n_core::{ContentFormat, J18nError, J18nResult};
use j18n_translator::{build_json_array_prompt, I18nTranslator, JSON_ARRAY_SYSTEM_INSTRUCTIONS};
use reqwest::Client;
use std::time::Duration;

pub const ANTHROPIC_API_KEY_ENV_VAR: &str = "ANTHROPIC_API_KEY";
pub const DEFAULT_MODEL_NAME: &str = "claude-sonnet-4-5";
pub const ANTHROPIC_VERSION: &str = "2023-06-01";
/// Default output-token budget. Comfortably fits a batch of UI strings and is
/// supported by every current Claude model; override with
/// [`AnthropicApiI18nTranslator::with_max_tokens`] for very large batches.
pub const DEFAULT_MAX_TOKENS: u32 = 8192;

const MESSAGES_URL: &str = "https://api.anthropic.com/v1/messages";
const REQUEST_TIMEOUT: Duration = Duration::from_secs(180);

#[async_trait]
pub trait AnthropicTransport: Send + Sync {
	async fn create_message(&self, request: &MessagesRequest) -> J18nResult<MessagesResponse>;
}

pub struct DefaultAnthropicTransport {
	api_key: String,
	client: Client,
	timeout: Duration,
}

impl DefaultAnthropicTransport {
	pub fn new(api_key: String, timeout: Duration) -> J18nResult<Self> {
		let client = Client::builder()
			.timeout(timeout)
			.build()
			.map_err(|e| J18nError::translator(format!("failed to build HTTP client: {e}")))?;

		Ok(Self {
			api_key,
			client,
			timeout,
		})
	}
}

#[async_trait]
impl AnthropicTransport for DefaultAnthropicTransport {
	async fn create_message(&self, request: &MessagesRequest) -> J18nResult<MessagesResponse> {
		let response = self
			.client
			.post(MESSAGES_URL)
			.timeout(self.timeout)
			.header("content-type", "application/json")
			.header("x-api-key", &self.api_key)
			.header("anthropic-version", ANTHROPIC_VERSION)
			.json(request)
			.send()
			.await
			.map_err(|e| J18nError::translator(format!("Anthropic request failed: {e}")))?;

		if !response.status().is_success() {
			let status = response.status();
			let body = response.text().await.unwrap_or_default();

			return Err(J18nError::translator(format!(
				"Anthropic API returned HTTP {status}: {body}"
			)));
		}

		response
			.json()
			.await
			.map_err(|e| J18nError::translator(format!("failed to parse Anthropic response: {e}")))
	}
}

pub struct AnthropicApiI18nTranslator<T: AnthropicTransport = DefaultAnthropicTransport> {
	additional_prompts: Vec<String>,
	max_tokens: u32,
	model_name: String,
	transport: T,
}

impl AnthropicApiI18nTranslator<DefaultAnthropicTransport> {
	pub const TRANSLATOR_ID: &'static str = "anthropic-api";

	pub fn new(additional_prompts: Vec<String>) -> J18nResult<Self> {
		Self::with_settings(additional_prompts, DEFAULT_MODEL_NAME.to_string())
	}

	/// Build a translator against the Anthropic Messages API. Reads the API key
	/// from `ANTHROPIC_API_KEY`, failing fast if it is missing.
	pub fn with_settings(additional_prompts: Vec<String>, model_name: impl Into<String>) -> J18nResult<Self> {
		let api_key = std::env::var(ANTHROPIC_API_KEY_ENV_VAR).map_err(|_| J18nError::EnvVarMissing {
			name: ANTHROPIC_API_KEY_ENV_VAR,
		})?;
		let transport = DefaultAnthropicTransport::new(api_key, REQUEST_TIMEOUT)?;

		Ok(Self {
			additional_prompts,
			max_tokens: DEFAULT_MAX_TOKENS,
			model_name: model_name.into(),
			transport,
		})
	}
}

impl<T: AnthropicTransport> AnthropicApiI18nTranslator<T> {
	pub fn with_transport(transport: T) -> Self {
		Self {
			additional_prompts: Vec::new(),
			max_tokens: DEFAULT_MAX_TOKENS,
			model_name: DEFAULT_MODEL_NAME.to_string(),
			transport,
		}
	}

	pub fn with_additional_prompts(mut self, additional_prompts: Vec<String>) -> Self {
		self.additional_prompts = additional_prompts;
		self
	}

	pub fn with_model_name(mut self, model_name: impl Into<String>) -> Self {
		self.model_name = model_name.into();
		self
	}

	pub fn with_max_tokens(mut self, max_tokens: u32) -> Self {
		self.max_tokens = max_tokens;
		self
	}

	async fn complete_chat(&self, prompt: &str, values_serialized: &str) -> J18nResult<String> {
		let request = MessagesRequest {
			model: self.model_name.clone(),
			max_tokens: self.max_tokens,
			system: Some(JSON_ARRAY_SYSTEM_INSTRUCTIONS.to_string()),
			messages: vec![AnthropicMessage {
				role: "user".to_string(),
				content: format!("{prompt}\n{values_serialized}"),
			}],
		};
		let response = self.transport.create_message(&request).await?;
		let text: String = response
			.content
			.into_iter()
			.filter(|block| block.block_type == "text")
			.filter_map(|block| block.text)
			.collect::<Vec<_>>()
			.join("");

		if text.trim().is_empty() {
			return Err(J18nError::translator("Anthropic API returned no text content"));
		}

		Ok(text)
	}
}

#[async_trait]
impl<T: AnthropicTransport> I18nTranslator for AnthropicApiI18nTranslator<T> {
	fn translator_id(&self) -> &str {
		"anthropic-api"
	}

	async fn translate_values(
		&self,
		from_language: &str,
		to_language: &str,
		values: Vec<String>,
		format: ContentFormat,
	) -> J18nResult<Vec<String>> {
		let values_for_prompt_serialized = serde_json::to_string(&values)
			.map_err(|e| J18nError::translator(format!("failed to serialize prompt array: {e}")))?;
		let prompt = build_json_array_prompt(from_language, to_language, &self.additional_prompts, format);
		let response_text = self.complete_chat(&prompt, &values_for_prompt_serialized).await?;
		let parsed: Vec<String> = serde_json::from_str(response_text.trim()).map_err(|e| {
			J18nError::translator(format!(
				"Anthropic did not return a JSON array of strings: {e}\nResponse:\n{response_text}"
			))
		})?;

		Ok(parsed)
	}
}

#[cfg(test)]
mod tests {
	use super::*;
	use crate::model::ContentBlock;
	use std::sync::{Arc, Mutex};

	#[derive(Default)]
	struct CapturedRequest {
		max_tokens: u32,
		messages: Vec<String>,
		model: String,
		system: Option<String>,
	}

	struct MockTransport {
		captured: Arc<Mutex<Vec<CapturedRequest>>>,
		blocks: Vec<(String, Option<String>)>,
		should_fail: bool,
	}

	impl MockTransport {
		fn ok(text: impl Into<String>) -> (Self, Arc<Mutex<Vec<CapturedRequest>>>) {
			let captured = Arc::new(Mutex::new(Vec::new()));

			(
				Self {
					captured: Arc::clone(&captured),
					blocks: vec![("text".to_string(), Some(text.into()))],
					should_fail: false,
				},
				captured,
			)
		}

		fn with_blocks(blocks: Vec<(String, Option<String>)>) -> (Self, Arc<Mutex<Vec<CapturedRequest>>>) {
			let captured = Arc::new(Mutex::new(Vec::new()));

			(
				Self {
					captured: Arc::clone(&captured),
					blocks,
					should_fail: false,
				},
				captured,
			)
		}

		fn err() -> Self {
			Self {
				captured: Arc::new(Mutex::new(Vec::new())),
				blocks: Vec::new(),
				should_fail: true,
			}
		}
	}

	#[async_trait]
	impl AnthropicTransport for MockTransport {
		async fn create_message(&self, request: &MessagesRequest) -> J18nResult<MessagesResponse> {
			self.captured.lock().unwrap().push(CapturedRequest {
				max_tokens: request.max_tokens,
				messages: request.messages.iter().map(|message| message.content.clone()).collect(),
				model: request.model.clone(),
				system: request.system.clone(),
			});

			if self.should_fail {
				return Err(J18nError::translator("mock transport failure"));
			}

			Ok(MessagesResponse {
				content: self
					.blocks
					.iter()
					.map(|(block_type, text)| ContentBlock {
						block_type: block_type.clone(),
						text: text.clone(),
					})
					.collect(),
				stop_reason: Some("end_turn".to_string()),
			})
		}
	}

	const ENGLISH: &str = "English";
	const PORTUGUESE: &str = "Portuguese";

	#[tokio::test]
	async fn parses_json_array_response_into_translated_values() {
		let (transport, _) = MockTransport::ok(r#"["olá","mundo"]"#);
		let translator = AnthropicApiI18nTranslator::with_transport(transport);

		let translated = translator
			.translate_values(
				ENGLISH,
				PORTUGUESE,
				vec!["hello".into(), "world".into()],
				ContentFormat::Json,
			)
			.await
			.unwrap();

		assert_eq!(translated, vec!["olá".to_string(), "mundo".to_string()]);
	}

	#[tokio::test]
	async fn concatenates_multiple_text_blocks_and_ignores_non_text() {
		let (transport, _) = MockTransport::with_blocks(vec![
			("text".to_string(), Some(r#"["o"#.to_string())),
			("thinking".to_string(), Some("ignored".to_string())),
			("text".to_string(), Some(r#"lá"]"#.to_string())),
		]);
		let translator = AnthropicApiI18nTranslator::with_transport(transport);

		let translated = translator
			.translate_values(ENGLISH, PORTUGUESE, vec!["hello".into()], ContentFormat::Json)
			.await
			.unwrap();

		assert_eq!(translated, vec!["olá".to_string()]);
	}

	#[tokio::test]
	async fn sends_system_instruction_model_and_max_tokens() {
		let (transport, captured) = MockTransport::ok(r#"["Olá [0]"]"#);
		let translator = AnthropicApiI18nTranslator::with_transport(transport);

		translator
			.translate_values(ENGLISH, PORTUGUESE, vec!["Hi [0]".into()], ContentFormat::Json)
			.await
			.unwrap();

		let captured = captured.lock().unwrap();
		let request = &captured[0];

		assert_eq!(request.model, DEFAULT_MODEL_NAME);
		assert_eq!(request.max_tokens, DEFAULT_MAX_TOKENS);
		assert!(request.system.as_deref().unwrap().contains("JSON array"));
		assert_eq!(request.messages.len(), 1);
		assert!(request.messages[0].contains("from English to Portuguese"));
		assert!(request.messages[0].contains("[\"Hi [0]\"]"));
	}

	#[tokio::test]
	async fn with_model_name_and_max_tokens_override_defaults() {
		let (transport, captured) = MockTransport::ok(r#"["X"]"#);
		let translator = AnthropicApiI18nTranslator::with_transport(transport)
			.with_model_name("claude-opus-4-5")
			.with_max_tokens(32000);

		translator
			.translate_values(ENGLISH, PORTUGUESE, vec!["x".into()], ContentFormat::Json)
			.await
			.unwrap();

		let captured = captured.lock().unwrap();
		assert_eq!(captured[0].model, "claude-opus-4-5");
		assert_eq!(captured[0].max_tokens, 32000);
	}

	#[tokio::test]
	async fn fails_when_response_is_not_a_json_array() {
		let (transport, _) = MockTransport::ok("not json at all");
		let translator = AnthropicApiI18nTranslator::with_transport(transport);

		let err = translator
			.translate_values(ENGLISH, PORTUGUESE, vec!["x".into()], ContentFormat::Json)
			.await
			.unwrap_err();

		assert!(matches!(err, J18nError::Translator(_)));
	}

	#[tokio::test]
	async fn fails_when_no_text_content_returned() {
		let (transport, _) =
			MockTransport::with_blocks(vec![("thinking".to_string(), Some("only thinking".to_string()))]);
		let translator = AnthropicApiI18nTranslator::with_transport(transport);

		let err = translator
			.translate_values(ENGLISH, PORTUGUESE, vec!["x".into()], ContentFormat::Json)
			.await
			.unwrap_err();

		match err {
			J18nError::Translator(message) => assert!(message.contains("no text content")),
			other => panic!("unexpected error: {other:?}"),
		}
	}

	#[tokio::test]
	async fn propagates_transport_errors() {
		let translator = AnthropicApiI18nTranslator::with_transport(MockTransport::err());

		let err = translator
			.translate_values(ENGLISH, PORTUGUESE, vec!["x".into()], ContentFormat::Json)
			.await
			.unwrap_err();

		match err {
			J18nError::Translator(message) => assert!(message.contains("mock transport failure")),
			other => panic!("unexpected error: {other:?}"),
		}
	}

	#[test]
	fn translator_id_is_anthropic_api() {
		let (transport, _) = MockTransport::ok(r#"["x"]"#);
		let translator = AnthropicApiI18nTranslator::with_transport(transport);

		assert_eq!(translator.translator_id(), "anthropic-api");
	}

	#[tokio::test]
	async fn additional_prompts_are_injected_between_placeholder_warnings() {
		let (transport, captured) = MockTransport::ok(r#"["X"]"#);
		let translator = AnthropicApiI18nTranslator::with_transport(transport)
			.with_additional_prompts(vec!["INJECTED-CONTEXT-A".to_string(), "INJECTED-CONTEXT-B".to_string()]);

		translator
			.translate_values(ENGLISH, PORTUGUESE, vec!["x".into()], ContentFormat::Json)
			.await
			.unwrap();

		let captured = captured.lock().unwrap();
		let user_message = &captured[0].messages[0];
		let placeholder_position = user_message
			.find("DO NOT remove, skip or modify placeholders")
			.expect("first placeholder warning must be present");
		let injected_a_position = user_message
			.find("INJECTED-CONTEXT-A")
			.expect("injected line A missing");
		let injected_b_position = user_message
			.find("INJECTED-CONTEXT-B")
			.expect("injected line B missing");
		let reminder_position = user_message
			.find("Once again, DO NOT remove placeholders")
			.expect("placeholder reminder must be present");

		assert!(placeholder_position < injected_a_position);
		assert!(injected_a_position < injected_b_position);
		assert!(injected_b_position < reminder_position);
	}

	#[tokio::test]
	async fn markdown_prompt_preserves_syntax_and_keeps_json_array_response_contract() {
		let (transport, captured) = MockTransport::ok(r##"["# Olá\n"]"##);
		let translator = AnthropicApiI18nTranslator::with_transport(transport);

		let translated = translator
			.translate_values(ENGLISH, PORTUGUESE, vec!["# Hi\n".into()], ContentFormat::Markdown)
			.await
			.unwrap();

		assert_eq!(translated, vec!["# Olá\n".to_string()]);

		let captured = captured.lock().unwrap();
		let user_message = &captured[0].messages[0];

		assert!(user_message.contains("Translate the Markdown/MDX document(s)"));
		assert!(user_message.contains("Preserve ALL Markdown and MDX syntax"));
		assert!(!user_message.contains("Translate the values in the following JSON array"));
	}
}
