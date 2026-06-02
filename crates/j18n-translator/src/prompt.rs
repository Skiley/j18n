//! Shared prompt construction for translators that talk to an HTTP chat API and
//! expect a JSON-array response (Gemini, OpenAI, Anthropic, OpenRouter). The
//! request framing — instruction lines, placeholder guardrails, injected
//! `additionalPrompts`, and the answer contract — is identical across these
//! providers; only the transport differs. CLI-based backends (Claude Code,
//! Codex) use a different, separator-based answer contract and keep their own
//! prompt builders.

use j18n_core::ContentFormat;

/// System instruction for JSON-array API translators: the model must answer with
/// a bare JSON array of translated strings, one per input, in order.
pub const JSON_ARRAY_SYSTEM_INSTRUCTIONS: &str =
	"Answer ONLY with a JSON array containing string elements, one for each translated value, \
	in the same order as their inputs. Do NOT embed the JSON array in Markdown, do NOT write \
	'```json' or equivalents; answer with a JSON array directly.";

/// Builds the instruction prompt for API translators that expect a JSON-array
/// response. The values to translate are NOT included here — callers send them
/// as a separate message (e.g. the serialized JSON array) right after this text.
pub fn build_json_array_prompt(
	from_language: &str,
	to_language: &str,
	additional_prompts: &[String],
	format: ContentFormat,
) -> String {
	let mut lines: Vec<String> = instruction_lines(from_language, to_language, format);

	lines.push("DO NOT remove, skip or modify placeholders, like [1], [2], [3], etc.".to_string());

	for prompt in additional_prompts {
		lines.push(prompt.clone());
	}

	lines.push("Once again, DO NOT remove placeholders like '[1]', '[2]', '[3]', '[4]', etc.".to_string());
	lines.extend(answer_lines(format));

	lines.join("\n")
}

fn instruction_lines(from_language: &str, to_language: &str, format: ContentFormat) -> Vec<String> {
	match format {
		ContentFormat::Json => vec![
			format!("Translate the values in the following JSON array, from {from_language} to {to_language}."),
			"DO NOT remove or modify HTML tags.".to_string(),
		],
		ContentFormat::Markdown => vec![
			format!("Translate the Markdown/MDX document(s) in the following JSON array, from {from_language} to {to_language}."),
			"Preserve ALL Markdown and MDX syntax exactly: headings, lists, tables, blockquotes, emphasis, and horizontal rules.".to_string(),
			"DO NOT translate or alter fenced or inline code, code block contents, URLs, link targets, image paths, HTML/JSX tags and attributes, JSX/React component names, or import/export statements.".to_string(),
			"For YAML front matter, translate only human-readable string values (e.g. title, description); never translate front matter keys.".to_string(),
			"Translate only human-readable prose: headings, paragraphs, list items, table cells, link text, and image alt text.".to_string(),
			"DO NOT add, remove, or reflow whitespace, blank lines, or indentation beyond what translating the prose itself requires.".to_string(),
		],
	}
}

fn answer_lines(format: ContentFormat) -> Vec<String> {
	match format {
		ContentFormat::Json => vec![
			"Answer ONLY with a JSON array containing string elements, one for each translated value, in the same order as their inputs.".to_string(),
			"Do NOT embed the JSON array in Markdown, do NOT write '```json' or equivalents.".to_string(),
			"Answer with a JSON array directly.".to_string(),
			"The JSON array is:".to_string(),
		],
		ContentFormat::Markdown => vec![
			"Answer ONLY with a JSON array of strings, one fully translated document per element, in the same order as their inputs.".to_string(),
			"Each element must be the entire translated document encoded as a single JSON string (newlines escaped as \\n).".to_string(),
			"Do NOT wrap the array or any element in a Markdown code fence; do NOT write '```json', '```', or any commentary.".to_string(),
			"The JSON array of documents is:".to_string(),
		],
	}
}

#[cfg(test)]
mod tests {
	use super::*;

	const ENGLISH: &str = "English";
	const PORTUGUESE: &str = "Portuguese";

	#[test]
	fn json_prompt_mentions_languages_and_json_array_contract() {
		let prompt = build_json_array_prompt(ENGLISH, PORTUGUESE, &[], ContentFormat::Json);

		assert!(prompt.contains("from English to Portuguese"));
		assert!(prompt.contains("Translate the values in the following JSON array"));
		assert!(prompt.contains("The JSON array is:"));
	}

	#[test]
	fn additional_prompts_are_injected_between_placeholder_warnings() {
		let prompt = build_json_array_prompt(
			ENGLISH,
			PORTUGUESE,
			&["INJECTED-A".to_string(), "INJECTED-B".to_string()],
			ContentFormat::Json,
		);

		let first_warning = prompt.find("DO NOT remove, skip or modify placeholders").unwrap();
		let injected_a = prompt.find("INJECTED-A").unwrap();
		let injected_b = prompt.find("INJECTED-B").unwrap();
		let reminder = prompt.find("Once again, DO NOT remove placeholders").unwrap();

		assert!(first_warning < injected_a);
		assert!(injected_a < injected_b);
		assert!(injected_b < reminder);
	}

	#[test]
	fn markdown_prompt_preserves_syntax_and_keeps_json_array_contract() {
		let prompt = build_json_array_prompt(ENGLISH, PORTUGUESE, &[], ContentFormat::Markdown);

		assert!(prompt.contains("Translate the Markdown/MDX document(s)"));
		assert!(prompt.contains("Preserve ALL Markdown and MDX syntax"));
		assert!(prompt.contains("one fully translated document per element"));
		assert!(!prompt.contains("Translate the values in the following JSON array"));
	}

	#[test]
	fn system_instruction_demands_bare_json_array() {
		assert!(JSON_ARRAY_SYSTEM_INSTRUCTIONS.contains("JSON array"));
		assert!(JSON_ARRAY_SYSTEM_INSTRUCTIONS.contains("directly"));
	}
}
