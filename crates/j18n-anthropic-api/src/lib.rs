//! Translator backed by the Anthropic Messages HTTP API (distinct from the
//! `claude-code` backend, which shells out to the local `claude` CLI). Requires
//! an `ANTHROPIC_API_KEY` and uses the same JSON-array response contract as the
//! other API translators.

pub mod model;
pub mod translator;

pub use model::{AnthropicMessage, ContentBlock, MessagesRequest, MessagesResponse};
pub use translator::{
	AnthropicApiI18nTranslator, AnthropicTransport, DefaultAnthropicTransport, ANTHROPIC_API_KEY_ENV_VAR,
	ANTHROPIC_VERSION, DEFAULT_MAX_TOKENS, DEFAULT_MODEL_NAME,
};
