use crate::model::{
	GeminiContent, GeminiPart, GenerateContentRequest, GenerateContentResponse, GenerationConfig,
};
use async_trait::async_trait;
use j18n_core::{J18nError, J18nResult, Language};
use j18n_translator::{create_extrapolated_values, restore_extrapolated_values, ExtrapolatedValue, I18nTranslator};
use reqwest::Client;
use std::time::Duration;

pub const GEMINI_API_KEY_ENV_VAR: &str = "GEMINI_API_KEY";

const DEFAULT_MODEL_NAME: &str = "gemini-3.1-pro-preview";
const SYSTEM_INSTRUCTIONS: &str =
	"Answer ONLY with a JSON array containing string elements, one for each translated value, \
	in the same order as their inputs. Do NOT embed the JSON array in Markdown, do NOT write \
	'```json' or equivalents; answer with a JSON array directly.";

#[async_trait]
pub trait GeminiTransport: Send + Sync {
	async fn generate_content(
		&self,
		model_name: &str,
		request: &GenerateContentRequest,
	) -> J18nResult<GenerateContentResponse>;
}

pub struct DefaultGeminiTransport {
	api_key: String,
	client: Client,
	timeout: Duration,
}

impl DefaultGeminiTransport {
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
impl GeminiTransport for DefaultGeminiTransport {
	async fn generate_content(
		&self,
		model_name: &str,
		request: &GenerateContentRequest,
	) -> J18nResult<GenerateContentResponse> {
		let url = format!("https://generativelanguage.googleapis.com/v1beta/models/{model_name}:generateContent");
		let response = self
			.client
			.post(&url)
			.timeout(self.timeout)
			.header("content-type", "application/json")
			.header("x-goog-api-key", &self.api_key)
			.json(request)
			.send()
			.await
			.map_err(|e| J18nError::translator(format!("Gemini request failed: {e}")))?;

		if !response.status().is_success() {
			let status = response.status();
			let body = response.text().await.unwrap_or_default();

			return Err(J18nError::translator(format!(
				"Gemini API returned HTTP {status}: {body}"
			)));
		}

		response
			.json()
			.await
			.map_err(|e| J18nError::translator(format!("failed to parse Gemini response: {e}")))
	}
}

pub struct GeminiApiI18nTranslator<T: GeminiTransport = DefaultGeminiTransport> {
	model_name: String,
	transport: T,
}

impl GeminiApiI18nTranslator<DefaultGeminiTransport> {
	pub const TRANSLATOR_ID: &'static str = "gemini-api";

	pub fn new() -> J18nResult<Self> {
		let api_key = std::env::var(GEMINI_API_KEY_ENV_VAR).map_err(|_| J18nError::EnvVarMissing {
			name: GEMINI_API_KEY_ENV_VAR,
		})?;
		let transport = DefaultGeminiTransport::new(api_key, Duration::from_secs(180))?;

		Ok(Self {
			model_name: DEFAULT_MODEL_NAME.to_string(),
			transport,
		})
	}
}

impl<T: GeminiTransport> GeminiApiI18nTranslator<T> {
	pub fn with_transport(transport: T) -> Self {
		Self {
			model_name: DEFAULT_MODEL_NAME.to_string(),
			transport,
		}
	}

	pub fn with_model_name(mut self, model_name: impl Into<String>) -> Self {
		self.model_name = model_name.into();
		self
	}

	async fn translate_extrapolated_values(
		&self,
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
		let prompt = build_prompt(from, to);
		let response_text = self.complete_chat(vec![prompt, values_for_prompt_serialized]).await?;
		let parsed: Vec<String> = serde_json::from_str(response_text.trim()).map_err(|e| {
			J18nError::translator(format!(
				"Gemini did not return a JSON array of strings: {e}\nResponse:\n{response_text}"
			))
		})?;

		Ok(parsed)
	}

	async fn complete_chat(&self, messages: Vec<String>) -> J18nResult<String> {
		let contents: Vec<GeminiContent> = messages
			.into_iter()
			.map(|message| GeminiContent {
				parts: vec![GeminiPart { text: message }],
				role: Some("user".to_string()),
			})
			.collect();
		let request = GenerateContentRequest {
			contents,
			generation_config: Some(GenerationConfig {
				temperature: Some(1.0),
			}),
			system_instruction: Some(GeminiContent {
				parts: vec![GeminiPart {
					text: SYSTEM_INSTRUCTIONS.to_string(),
				}],
				role: None,
			}),
		};
		let parsed = self.transport.generate_content(&self.model_name, &request).await?;
		let first_candidate = parsed
			.candidates
			.into_iter()
			.next()
			.ok_or_else(|| J18nError::translator("no content candidate returned by Gemini API"))?;
		let joined_output: String = first_candidate
			.content
			.parts
			.into_iter()
			.map(|part| part.text)
			.collect::<Vec<_>>()
			.join("\n");

		Ok(joined_output)
	}
}

#[async_trait]
impl<T: GeminiTransport> I18nTranslator for GeminiApiI18nTranslator<T> {
	fn translator_id(&self) -> &str {
		"gemini-api"
	}

	async fn translate_i18n_values(
		&self,
		from: Language,
		to: Language,
		values: Vec<String>,
	) -> J18nResult<Vec<String>> {
		let extrapolated_values = create_extrapolated_values(&values);
		let translated_values = self
			.translate_extrapolated_values(&extrapolated_values, from, to)
			.await?;

		restore_extrapolated_values(&extrapolated_values, &translated_values)
	}
}

fn build_prompt(from: Language, to: Language) -> String {
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
		"Answer ONLY with a JSON array containing string elements, one for each translated value, in the same order as their inputs.".to_string(),
		"Do NOT embed the JSON array in Markdown, do NOT write '```json' or equivalents.".to_string(),
		"Answer with a JSON array directly.".to_string(),
		"The JSON array is:".to_string(),
	]
	.join("\n")
}

#[cfg(test)]
mod tests {
	use super::*;
	use crate::model::GenerateContentCandidate;
	use std::sync::{Arc, Mutex};

	#[derive(Default)]
	struct CapturedRequest {
		messages: Vec<String>,
		model_name: String,
		system_instruction: Option<String>,
	}

	struct MockTransport {
		captured: Arc<Mutex<Vec<CapturedRequest>>>,
		response_array: Arc<Mutex<String>>,
		should_fail: bool,
	}

	impl MockTransport {
		fn ok(response_array: impl Into<String>) -> (Self, Arc<Mutex<Vec<CapturedRequest>>>) {
			let captured = Arc::new(Mutex::new(Vec::new()));

			(
				Self {
					captured: Arc::clone(&captured),
					response_array: Arc::new(Mutex::new(response_array.into())),
					should_fail: false,
				},
				captured,
			)
		}

		fn err() -> Self {
			Self {
				captured: Arc::new(Mutex::new(Vec::new())),
				response_array: Arc::new(Mutex::new(String::new())),
				should_fail: true,
			}
		}
	}

	#[async_trait]
	impl GeminiTransport for MockTransport {
		async fn generate_content(
			&self,
			model_name: &str,
			request: &GenerateContentRequest,
		) -> J18nResult<GenerateContentResponse> {
			let messages = request
				.contents
				.iter()
				.flat_map(|content| content.parts.iter())
				.map(|part| part.text.clone())
				.collect();
			let system_instruction = request.system_instruction.as_ref().map(|content| {
				content
					.parts
					.iter()
					.map(|part| part.text.clone())
					.collect::<Vec<_>>()
					.join("\n")
			});

			self.captured.lock().unwrap().push(CapturedRequest {
				messages,
				model_name: model_name.to_string(),
				system_instruction,
			});

			if self.should_fail {
				return Err(J18nError::translator("mock transport failure"));
			}

			Ok(GenerateContentResponse {
				candidates: vec![GenerateContentCandidate {
					content: GeminiContent {
						parts: vec![GeminiPart {
							text: self.response_array.lock().unwrap().clone(),
						}],
						role: Some("model".to_string()),
					},
				}],
			})
		}
	}

	fn portuguese() -> Language {
		Language::from_iso_639_code("pt").unwrap()
	}

	#[tokio::test]
	async fn parses_json_array_response_into_translated_values() {
		let (transport, _) = MockTransport::ok(r#"["olá","mundo"]"#);
		let translator = GeminiApiI18nTranslator::with_transport(transport);

		let translated = translator
			.translate_i18n_values(
				Language::ENGLISH,
				portuguese(),
				vec!["hello".into(), "world".into()],
			)
			.await
			.unwrap();

		assert_eq!(translated, vec!["olá".to_string(), "mundo".to_string()]);
	}

	#[tokio::test]
	async fn restores_interpolations_after_translation() {
		let (transport, _) = MockTransport::ok(r#"["Olá [0]!"]"#);
		let translator = GeminiApiI18nTranslator::with_transport(transport);

		let translated = translator
			.translate_i18n_values(Language::ENGLISH, portuguese(), vec!["Hi {{name}}!".into()])
			.await
			.unwrap();

		assert_eq!(translated, vec!["Olá {{name}}!".to_string()]);
	}

	#[tokio::test]
	async fn passes_default_model_name_to_transport() {
		let (transport, captured) = MockTransport::ok(r#"["X"]"#);
		let translator = GeminiApiI18nTranslator::with_transport(transport);

		translator
			.translate_i18n_values(Language::ENGLISH, portuguese(), vec!["x".into()])
			.await
			.unwrap();

		let captured = captured.lock().unwrap();
		assert_eq!(captured.len(), 1);
		assert_eq!(captured[0].model_name, DEFAULT_MODEL_NAME);
	}

	#[tokio::test]
	async fn with_model_name_overrides_default() {
		let (transport, captured) = MockTransport::ok(r#"["X"]"#);
		let translator = GeminiApiI18nTranslator::with_transport(transport).with_model_name("custom-model");

		translator
			.translate_i18n_values(Language::ENGLISH, portuguese(), vec!["x".into()])
			.await
			.unwrap();

		let captured = captured.lock().unwrap();
		assert_eq!(captured[0].model_name, "custom-model");
	}

	#[tokio::test]
	async fn sends_system_instruction_and_prompt_messages() {
		let (transport, captured) = MockTransport::ok(r#"["Olá [0]"]"#);
		let translator = GeminiApiI18nTranslator::with_transport(transport);

		translator
			.translate_i18n_values(Language::ENGLISH, portuguese(), vec!["Hi {{name}}".into()])
			.await
			.unwrap();

		let captured = captured.lock().unwrap();
		let request = &captured[0];

		assert!(request.system_instruction.as_deref().unwrap().contains("JSON array"));
		assert_eq!(request.messages.len(), 2);
		assert!(request.messages[0].contains("from English to Portuguese"));
		assert_eq!(request.messages[1], "[\"Hi [0]\"]");
	}

	#[tokio::test]
	async fn fails_when_response_is_not_a_json_array() {
		let (transport, _) = MockTransport::ok("not json at all");
		let translator = GeminiApiI18nTranslator::with_transport(transport);

		let err = translator
			.translate_i18n_values(Language::ENGLISH, portuguese(), vec!["x".into()])
			.await
			.unwrap_err();

		assert!(matches!(err, J18nError::Translator(_)));
	}

	#[tokio::test]
	async fn propagates_transport_errors() {
		let translator = GeminiApiI18nTranslator::with_transport(MockTransport::err());

		let err = translator
			.translate_i18n_values(Language::ENGLISH, portuguese(), vec!["x".into()])
			.await
			.unwrap_err();

		match err {
			J18nError::Translator(message) => assert!(message.contains("mock transport failure")),
			other => panic!("unexpected error: {other:?}"),
		}
	}

	#[tokio::test]
	async fn fails_when_no_candidates_returned() {
		struct EmptyTransport;

		#[async_trait]
		impl GeminiTransport for EmptyTransport {
			async fn generate_content(
				&self,
				_model_name: &str,
				_request: &GenerateContentRequest,
			) -> J18nResult<GenerateContentResponse> {
				Ok(GenerateContentResponse { candidates: vec![] })
			}
		}

		let translator = GeminiApiI18nTranslator::with_transport(EmptyTransport);

		let err = translator
			.translate_i18n_values(Language::ENGLISH, portuguese(), vec!["x".into()])
			.await
			.unwrap_err();

		match err {
			J18nError::Translator(message) => assert!(message.contains("no content candidate")),
			other => panic!("unexpected error: {other:?}"),
		}
	}

	#[test]
	fn translator_id_is_gemini_api() {
		let (transport, _) = MockTransport::ok(r#"["x"]"#);
		let translator = GeminiApiI18nTranslator::with_transport(transport);

		assert_eq!(translator.translator_id(), "gemini-api");
	}
}
