//! Translator backed by the OpenAI Chat Completions HTTP API.
//!
//! Because the wire format is shared, the same translator also drives
//! [OpenRouter](https://openrouter.ai) — an OpenAI-compatible gateway to many
//! models — via the [`OpenAiApiI18nTranslator::openrouter`] constructor, which
//! only swaps the base URL, the API-key env var, and adds attribution headers.

pub mod model;
pub mod translator;

pub use model::{ChatChoice, ChatCompletionRequest, ChatCompletionResponse, ChatMessage, ChatResponseMessage};
pub use translator::{
	DefaultOpenAiTransport, OpenAiApiI18nTranslator, OpenAiTransport, OPENAI_API_KEY_ENV_VAR, OPENAI_BASE_URL,
	OPENAI_DEFAULT_MODEL, OPENROUTER_API_KEY_ENV_VAR, OPENROUTER_BASE_URL, OPENROUTER_DEFAULT_MODEL,
};
