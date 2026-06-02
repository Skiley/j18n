use crate::model::{ChatCompletionRequest, ChatCompletionResponse, ChatMessage};
use async_trait::async_trait;
use j18n_core::{ContentFormat, J18nError, J18nResult};
use j18n_translator::{build_json_array_prompt, I18nTranslator, JSON_ARRAY_SYSTEM_INSTRUCTIONS};
use reqwest::Client;
use std::time::Duration;

pub const OPENAI_API_KEY_ENV_VAR: &str = "OPENAI_API_KEY";
pub const OPENAI_BASE_URL: &str = "https://api.openai.com/v1";
pub const OPENAI_DEFAULT_MODEL: &str = "gpt-5.1";

pub const OPENROUTER_API_KEY_ENV_VAR: &str = "OPENROUTER_API_KEY";
pub const OPENROUTER_BASE_URL: &str = "https://openrouter.ai/api/v1";
pub const OPENROUTER_DEFAULT_MODEL: &str = "openai/gpt-5.1";

const REQUEST_TIMEOUT: Duration = Duration::from_secs(180);

#[async_trait]
pub trait OpenAiTransport: Send + Sync {
	async fn chat_completion(&self, request: &ChatCompletionRequest) -> J18nResult<ChatCompletionResponse>;

	/// Human-friendly provider name used in error messages ("OpenAI"/"OpenRouter").
	fn provider_label(&self) -> &str;
}

/// Default transport for any OpenAI-compatible `/chat/completions` endpoint.
/// Construct it with [`DefaultOpenAiTransport::openai`] or
/// [`DefaultOpenAiTransport::openrouter`].
pub struct DefaultOpenAiTransport {
	api_key: String,
	base_url: String,
	client: Client,
	extra_headers: Vec<(&'static str, String)>,
	provider_label: &'static str,
	timeout: Duration,
}

impl DefaultOpenAiTransport {
	/// Talk to the official OpenAI API.
	pub fn openai(api_key: String, timeout: Duration) -> J18nResult<Self> {
		Self::build(api_key, OPENAI_BASE_URL, "OpenAI", Vec::new(), timeout)
	}

	/// Talk to OpenRouter. Adds the optional `HTTP-Referer`/`X-Title` attribution
	/// headers OpenRouter uses to identify the calling app on its leaderboards.
	pub fn openrouter(api_key: String, timeout: Duration) -> J18nResult<Self> {
		let extra_headers = vec![
			("HTTP-Referer", "https://github.com/Skiley/j18n".to_string()),
			("X-Title", "j18n".to_string()),
		];

		Self::build(api_key, OPENROUTER_BASE_URL, "OpenRouter", extra_headers, timeout)
	}

	fn build(
		api_key: String,
		base_url: &str,
		provider_label: &'static str,
		extra_headers: Vec<(&'static str, String)>,
		timeout: Duration,
	) -> J18nResult<Self> {
		let client = Client::builder()
			.timeout(timeout)
			.build()
			.map_err(|e| J18nError::translator(format!("failed to build HTTP client: {e}")))?;

		Ok(Self {
			api_key,
			base_url: base_url.to_string(),
			client,
			extra_headers,
			provider_label,
			timeout,
		})
	}
}

#[async_trait]
impl OpenAiTransport for DefaultOpenAiTransport {
	async fn chat_completion(&self, request: &ChatCompletionRequest) -> J18nResult<ChatCompletionResponse> {
		let url = format!("{}/chat/completions", self.base_url);
		let mut builder = self
			.client
			.post(&url)
			.timeout(self.timeout)
			.header("content-type", "application/json")
			.bearer_auth(&self.api_key);

		for (name, value) in &self.extra_headers {
			builder = builder.header(*name, value);
		}

		let response = builder
			.json(request)
			.send()
			.await
			.map_err(|e| J18nError::translator(format!("{} request failed: {e}", self.provider_label)))?;

		if !response.status().is_success() {
			let status = response.status();
			let body = response.text().await.unwrap_or_default();

			return Err(J18nError::translator(format!(
				"{} API returned HTTP {status}: {body}",
				self.provider_label
			)));
		}

		response
			.json()
			.await
			.map_err(|e| J18nError::translator(format!("failed to parse {} response: {e}", self.provider_label)))
	}

	fn provider_label(&self) -> &str {
		self.provider_label
	}
}

pub struct OpenAiApiI18nTranslator<T: OpenAiTransport = DefaultOpenAiTransport> {
	additional_prompts: Vec<String>,
	model_name: String,
	translator_id: &'static str,
	transport: T,
}

impl OpenAiApiI18nTranslator<DefaultOpenAiTransport> {
	pub const OPENAI_TRANSLATOR_ID: &'static str = "openai-api";
	pub const OPENROUTER_TRANSLATOR_ID: &'static str = "openrouter-api";

	/// Build a translator against the official OpenAI API. Reads the API key from
	/// `OPENAI_API_KEY`, failing fast if it is missing.
	pub fn openai(additional_prompts: Vec<String>, model_name: impl Into<String>) -> J18nResult<Self> {
		let api_key = std::env::var(OPENAI_API_KEY_ENV_VAR).map_err(|_| J18nError::EnvVarMissing {
			name: OPENAI_API_KEY_ENV_VAR,
		})?;
		let transport = DefaultOpenAiTransport::openai(api_key, REQUEST_TIMEOUT)?;

		Ok(Self {
			additional_prompts,
			model_name: model_name.into(),
			translator_id: Self::OPENAI_TRANSLATOR_ID,
			transport,
		})
	}

	/// Build a translator against OpenRouter. Reads the API key from
	/// `OPENROUTER_API_KEY`, failing fast if it is missing.
	pub fn openrouter(additional_prompts: Vec<String>, model_name: impl Into<String>) -> J18nResult<Self> {
		let api_key = std::env::var(OPENROUTER_API_KEY_ENV_VAR).map_err(|_| J18nError::EnvVarMissing {
			name: OPENROUTER_API_KEY_ENV_VAR,
		})?;
		let transport = DefaultOpenAiTransport::openrouter(api_key, REQUEST_TIMEOUT)?;

		Ok(Self {
			additional_prompts,
			model_name: model_name.into(),
			translator_id: Self::OPENROUTER_TRANSLATOR_ID,
			transport,
		})
	}
}

impl<T: OpenAiTransport> OpenAiApiI18nTranslator<T> {
	pub fn with_transport(transport: T) -> Self {
		Self {
			additional_prompts: Vec::new(),
			model_name: OPENAI_DEFAULT_MODEL.to_string(),
			translator_id: OpenAiApiI18nTranslator::<DefaultOpenAiTransport>::OPENAI_TRANSLATOR_ID,
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

	pub fn with_translator_id(mut self, translator_id: &'static str) -> Self {
		self.translator_id = translator_id;
		self
	}

	async fn complete_chat(&self, prompt: &str, values_serialized: &str) -> J18nResult<String> {
		let request = ChatCompletionRequest {
			model: self.model_name.clone(),
			messages: vec![
				ChatMessage {
					role: "system".to_string(),
					content: JSON_ARRAY_SYSTEM_INSTRUCTIONS.to_string(),
				},
				ChatMessage {
					role: "user".to_string(),
					content: format!("{prompt}\n{values_serialized}"),
				},
			],
		};
		let label = self.transport.provider_label();
		let response = self.transport.chat_completion(&request).await?;
		let message = response
			.choices
			.into_iter()
			.next()
			.map(|choice| choice.message)
			.ok_or_else(|| J18nError::translator(format!("no choices returned by {label} API")))?;

		if let Some(refusal) = message.refusal.filter(|refusal| !refusal.trim().is_empty()) {
			return Err(J18nError::translator(format!("{label} refused the request: {refusal}")));
		}

		message
			.content
			.filter(|content| !content.trim().is_empty())
			.ok_or_else(|| J18nError::translator(format!("empty response from {label} API")))
	}
}

#[async_trait]
impl<T: OpenAiTransport> I18nTranslator for OpenAiApiI18nTranslator<T> {
	fn translator_id(&self) -> &str {
		self.translator_id
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
				"{} did not return a JSON array of strings: {e}\nResponse:\n{response_text}",
				self.translator_id
			))
		})?;

		Ok(parsed)
	}
}

#[cfg(test)]
mod tests {
	use super::*;
	use crate::model::{ChatChoice, ChatResponseMessage};
	use std::sync::{Arc, Mutex};

	#[derive(Default)]
	struct CapturedRequest {
		messages: Vec<(String, String)>,
		model: String,
	}

	struct MockTransport {
		captured: Arc<Mutex<Vec<CapturedRequest>>>,
		content: Option<String>,
		refusal: Option<String>,
		empty_choices: bool,
		should_fail: bool,
	}

	impl MockTransport {
		fn ok(content: impl Into<String>) -> (Self, Arc<Mutex<Vec<CapturedRequest>>>) {
			let captured = Arc::new(Mutex::new(Vec::new()));

			(
				Self {
					captured: Arc::clone(&captured),
					content: Some(content.into()),
					refusal: None,
					empty_choices: false,
					should_fail: false,
				},
				captured,
			)
		}

		fn err() -> Self {
			Self {
				captured: Arc::new(Mutex::new(Vec::new())),
				content: None,
				refusal: None,
				empty_choices: false,
				should_fail: true,
			}
		}

		fn refusal(message: impl Into<String>) -> Self {
			Self {
				captured: Arc::new(Mutex::new(Vec::new())),
				content: None,
				refusal: Some(message.into()),
				empty_choices: false,
				should_fail: false,
			}
		}

		fn no_choices() -> Self {
			Self {
				captured: Arc::new(Mutex::new(Vec::new())),
				content: None,
				refusal: None,
				empty_choices: true,
				should_fail: false,
			}
		}
	}

	#[async_trait]
	impl OpenAiTransport for MockTransport {
		async fn chat_completion(&self, request: &ChatCompletionRequest) -> J18nResult<ChatCompletionResponse> {
			self.captured.lock().unwrap().push(CapturedRequest {
				messages: request
					.messages
					.iter()
					.map(|message| (message.role.clone(), message.content.clone()))
					.collect(),
				model: request.model.clone(),
			});

			if self.should_fail {
				return Err(J18nError::translator("mock transport failure"));
			}

			let choices = if self.empty_choices {
				Vec::new()
			} else {
				vec![ChatChoice {
					message: ChatResponseMessage {
						content: self.content.clone(),
						refusal: self.refusal.clone(),
					},
				}]
			};

			Ok(ChatCompletionResponse { choices })
		}

		fn provider_label(&self) -> &str {
			"OpenAI"
		}
	}

	const ENGLISH: &str = "English";
	const PORTUGUESE: &str = "Portuguese";

	#[tokio::test]
	async fn parses_json_array_response_into_translated_values() {
		let (transport, _) = MockTransport::ok(r#"["olá","mundo"]"#);
		let translator = OpenAiApiI18nTranslator::with_transport(transport);

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
	async fn sends_system_instruction_and_combined_user_message() {
		let (transport, captured) = MockTransport::ok(r#"["Olá [0]"]"#);
		let translator = OpenAiApiI18nTranslator::with_transport(transport);

		translator
			.translate_values(ENGLISH, PORTUGUESE, vec!["Hi [0]".into()], ContentFormat::Json)
			.await
			.unwrap();

		let captured = captured.lock().unwrap();
		let request = &captured[0];

		assert_eq!(request.model, OPENAI_DEFAULT_MODEL);
		assert_eq!(request.messages.len(), 2);
		assert_eq!(request.messages[0].0, "system");
		assert!(request.messages[0].1.contains("JSON array"));
		assert_eq!(request.messages[1].0, "user");
		assert!(request.messages[1].1.contains("from English to Portuguese"));
		assert!(request.messages[1].1.contains("[\"Hi [0]\"]"));
	}

	#[tokio::test]
	async fn with_model_name_overrides_default() {
		let (transport, captured) = MockTransport::ok(r#"["X"]"#);
		let translator = OpenAiApiI18nTranslator::with_transport(transport).with_model_name("gpt-4.1-mini");

		translator
			.translate_values(ENGLISH, PORTUGUESE, vec!["x".into()], ContentFormat::Json)
			.await
			.unwrap();

		assert_eq!(captured.lock().unwrap()[0].model, "gpt-4.1-mini");
	}

	#[tokio::test]
	async fn fails_when_response_is_not_a_json_array() {
		let (transport, _) = MockTransport::ok("not json at all");
		let translator = OpenAiApiI18nTranslator::with_transport(transport);

		let err = translator
			.translate_values(ENGLISH, PORTUGUESE, vec!["x".into()], ContentFormat::Json)
			.await
			.unwrap_err();

		assert!(matches!(err, J18nError::Translator(_)));
	}

	#[tokio::test]
	async fn propagates_transport_errors() {
		let translator = OpenAiApiI18nTranslator::with_transport(MockTransport::err());

		let err = translator
			.translate_values(ENGLISH, PORTUGUESE, vec!["x".into()], ContentFormat::Json)
			.await
			.unwrap_err();

		match err {
			J18nError::Translator(message) => assert!(message.contains("mock transport failure")),
			other => panic!("unexpected error: {other:?}"),
		}
	}

	#[tokio::test]
	async fn fails_when_no_choices_returned() {
		let translator = OpenAiApiI18nTranslator::with_transport(MockTransport::no_choices());

		let err = translator
			.translate_values(ENGLISH, PORTUGUESE, vec!["x".into()], ContentFormat::Json)
			.await
			.unwrap_err();

		match err {
			J18nError::Translator(message) => assert!(message.contains("no choices")),
			other => panic!("unexpected error: {other:?}"),
		}
	}

	#[tokio::test]
	async fn surfaces_model_refusal() {
		let translator = OpenAiApiI18nTranslator::with_transport(MockTransport::refusal("cannot comply"));

		let err = translator
			.translate_values(ENGLISH, PORTUGUESE, vec!["x".into()], ContentFormat::Json)
			.await
			.unwrap_err();

		match err {
			J18nError::Translator(message) => assert!(message.contains("refused") && message.contains("cannot comply")),
			other => panic!("unexpected error: {other:?}"),
		}
	}

	#[test]
	fn translator_id_defaults_to_openai_api() {
		let (transport, _) = MockTransport::ok(r#"["x"]"#);
		let translator = OpenAiApiI18nTranslator::with_transport(transport);

		assert_eq!(translator.translator_id(), "openai-api");
	}

	#[test]
	fn translator_id_is_configurable_for_openrouter() {
		let (transport, _) = MockTransport::ok(r#"["x"]"#);
		let translator = OpenAiApiI18nTranslator::with_transport(transport).with_translator_id("openrouter-api");

		assert_eq!(translator.translator_id(), "openrouter-api");
	}

	#[tokio::test]
	async fn additional_prompts_are_injected_between_placeholder_warnings() {
		let (transport, captured) = MockTransport::ok(r#"["X"]"#);
		let translator = OpenAiApiI18nTranslator::with_transport(transport)
			.with_additional_prompts(vec!["INJECTED-CONTEXT-A".to_string(), "INJECTED-CONTEXT-B".to_string()]);

		translator
			.translate_values(ENGLISH, PORTUGUESE, vec!["x".into()], ContentFormat::Json)
			.await
			.unwrap();

		let captured = captured.lock().unwrap();
		let user_message = &captured[0].messages[1].1;
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
		let translator = OpenAiApiI18nTranslator::with_transport(transport);

		let translated = translator
			.translate_values(ENGLISH, PORTUGUESE, vec!["# Hi\n".into()], ContentFormat::Markdown)
			.await
			.unwrap();

		assert_eq!(translated, vec!["# Olá\n".to_string()]);

		let captured = captured.lock().unwrap();
		let user_message = &captured[0].messages[1].1;

		assert!(user_message.contains("Translate the Markdown/MDX document(s)"));
		assert!(user_message.contains("Preserve ALL Markdown and MDX syntax"));
		assert!(!user_message.contains("Translate the values in the following JSON array"));
	}
}
