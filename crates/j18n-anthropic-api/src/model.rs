use serde::{Deserialize, Serialize};

/// One message in a Messages request. `content` is sent as a plain string, which
/// the Anthropic API accepts as a single text block.
#[derive(Debug, Serialize)]
pub struct AnthropicMessage {
	pub role: String,
	pub content: String,
}

/// Anthropic Messages request body. `max_tokens` is required by the API; the
/// JSON-array instruction is passed via the top-level `system` field.
#[derive(Debug, Serialize)]
pub struct MessagesRequest {
	pub model: String,
	pub max_tokens: u32,
	#[serde(skip_serializing_if = "Option::is_none")]
	pub system: Option<String>,
	pub messages: Vec<AnthropicMessage>,
}

#[derive(Debug, Deserialize)]
pub struct MessagesResponse {
	#[serde(default)]
	pub content: Vec<ContentBlock>,
	#[serde(default)]
	pub stop_reason: Option<String>,
}

#[derive(Debug, Deserialize)]
pub struct ContentBlock {
	#[serde(rename = "type")]
	pub block_type: String,
	#[serde(default)]
	pub text: Option<String>,
}
