use serde::{Deserialize, Serialize};

/// One message in a Chat Completions request (`role` + plain-text `content`).
#[derive(Debug, Serialize)]
pub struct ChatMessage {
	pub role: String,
	pub content: String,
}

/// Minimal Chat Completions request body. Only `model` and `messages` are sent —
/// no `temperature`/`max_tokens` — so the same shape works for both classic chat
/// models and reasoning models (which reject those parameters), on OpenAI and on
/// any OpenAI-compatible gateway such as OpenRouter.
#[derive(Debug, Serialize)]
pub struct ChatCompletionRequest {
	pub model: String,
	pub messages: Vec<ChatMessage>,
}

#[derive(Debug, Deserialize)]
pub struct ChatCompletionResponse {
	#[serde(default)]
	pub choices: Vec<ChatChoice>,
}

#[derive(Debug, Deserialize)]
pub struct ChatChoice {
	pub message: ChatResponseMessage,
}

#[derive(Debug, Deserialize)]
pub struct ChatResponseMessage {
	#[serde(default)]
	pub content: Option<String>,
	/// Present when the model declined to answer (OpenAI structured refusal).
	#[serde(default)]
	pub refusal: Option<String>,
}
