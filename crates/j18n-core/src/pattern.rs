use crate::error::J18nError;

#[derive(Clone, Debug)]
pub struct PathPattern {
	parts: Vec<PatternPart>,
	source: String,
}

#[derive(Clone, Debug)]
enum PatternPart {
	DoubleStar,
	Component(ComponentMatcher),
}

#[derive(Clone, Debug)]
enum ComponentMatcher {
	Any,
	Exact(String),
	Glob(Vec<GlobPart>),
}

#[derive(Clone, Debug)]
enum GlobPart {
	Literal(String),
	Star,
	Question,
}

impl PathPattern {
	pub fn parse(pattern: &str) -> Result<Self, J18nError> {
		if pattern.is_empty() {
			return Err(J18nError::InvalidPattern {
				pattern: pattern.to_string(),
				reason: "pattern must not be empty".to_string(),
			});
		}

		let mut parts: Vec<PatternPart> = Vec::new();

		for raw_segment in pattern.split('.') {
			if raw_segment.is_empty() {
				return Err(J18nError::InvalidPattern {
					pattern: pattern.to_string(),
					reason: "empty segment between dots".to_string(),
				});
			}

			if raw_segment == "**" {
				parts.push(PatternPart::DoubleStar);

				continue;
			}

			parts.push(PatternPart::Component(parse_component(raw_segment)));
		}

		Ok(Self {
			parts,
			source: pattern.to_string(),
		})
	}

	pub fn source(&self) -> &str {
		&self.source
	}

	pub fn matches(&self, key: &str) -> bool {
		let segments: Vec<&str> = key.split('.').collect();

		matches_parts(&self.parts, &segments)
	}
}

fn parse_component(segment: &str) -> ComponentMatcher {
	if segment == "*" {
		return ComponentMatcher::Any;
	}

	if !segment.contains('*') && !segment.contains('?') {
		return ComponentMatcher::Exact(segment.to_string());
	}

	let mut parts: Vec<GlobPart> = Vec::new();
	let mut buffer = String::new();

	for character in segment.chars() {
		match character {
			'*' | '?' => {
				if !buffer.is_empty() {
					parts.push(GlobPart::Literal(std::mem::take(&mut buffer)));
				}

				parts.push(if character == '*' {
					GlobPart::Star
				} else {
					GlobPart::Question
				});
			}
			other => buffer.push(other),
		}
	}

	if !buffer.is_empty() {
		parts.push(GlobPart::Literal(buffer));
	}

	ComponentMatcher::Glob(parts)
}

fn matches_parts(parts: &[PatternPart], segments: &[&str]) -> bool {
	match (parts.first(), segments.first()) {
		(None, None) => true,
		(None, Some(_)) => false,
		(Some(PatternPart::DoubleStar), _) => {
			for skipped in 0..=segments.len() {
				if matches_parts(&parts[1..], &segments[skipped..]) {
					return true;
				}
			}

			false
		}
		(Some(PatternPart::Component(_)), None) => false,
		(Some(PatternPart::Component(component)), Some(segment)) => {
			matches_component(component, segment) && matches_parts(&parts[1..], &segments[1..])
		}
	}
}

fn matches_component(component: &ComponentMatcher, segment: &str) -> bool {
	match component {
		ComponentMatcher::Any => true,
		ComponentMatcher::Exact(text) => text == segment,
		ComponentMatcher::Glob(parts) => matches_glob(parts, segment),
	}
}

fn matches_glob(parts: &[GlobPart], input: &str) -> bool {
	match parts.first() {
		None => input.is_empty(),
		Some(GlobPart::Literal(text)) => match input.strip_prefix(text.as_str()) {
			Some(rest) => matches_glob(&parts[1..], rest),
			None => false,
		},
		Some(GlobPart::Question) => {
			let mut chars = input.chars();

			match chars.next() {
				Some(_) => matches_glob(&parts[1..], chars.as_str()),
				None => false,
			}
		}
		Some(GlobPart::Star) => {
			for taken in 0..=input.len() {
				if !input.is_char_boundary(taken) {
					continue;
				}

				if matches_glob(&parts[1..], &input[taken..]) {
					return true;
				}
			}

			false
		}
	}
}

pub fn key_matches_any(key: &str, patterns: &[PathPattern]) -> bool {
	patterns.iter().any(|pattern| pattern.matches(key))
}

#[cfg(test)]
mod tests {
	use super::*;

	#[test]
	fn exact_segment_matches_only_that_key() {
		let pattern = PathPattern::parse("sample").unwrap();

		assert!(pattern.matches("sample"));
		assert!(!pattern.matches("sampler"));
		assert!(!pattern.matches("sample.foo"));
	}

	#[test]
	fn double_star_at_end_matches_zero_or_more_segments() {
		let pattern = PathPattern::parse("sample.**").unwrap();

		assert!(pattern.matches("sample"));
		assert!(pattern.matches("sample.a"));
		assert!(pattern.matches("sample.a.b.c"));
		assert!(!pattern.matches("other"));
		assert!(!pattern.matches("other.sample"));
	}

	#[test]
	fn double_star_in_the_middle_matches_any_depth() {
		let pattern = PathPattern::parse("a.**.b").unwrap();

		assert!(pattern.matches("a.b"));
		assert!(pattern.matches("a.x.b"));
		assert!(pattern.matches("a.x.y.b"));
		assert!(!pattern.matches("a.b.c"));
	}

	#[test]
	fn single_star_matches_one_segment() {
		let pattern = PathPattern::parse("a.*.c").unwrap();

		assert!(pattern.matches("a.b.c"));
		assert!(pattern.matches("a.xx.c"));
		assert!(!pattern.matches("a.c"));
		assert!(!pattern.matches("a.b.b.c"));
	}

	#[test]
	fn star_within_segment_matches_any_chars_within_segment_only() {
		let pattern = PathPattern::parse("foo*").unwrap();

		assert!(pattern.matches("foo"));
		assert!(pattern.matches("foobar"));
		assert!(!pattern.matches("foo.bar"));
	}

	#[test]
	fn question_within_segment_matches_single_char() {
		let pattern = PathPattern::parse("a?").unwrap();

		assert!(pattern.matches("ab"));
		assert!(!pattern.matches("a"));
		assert!(!pattern.matches("abc"));
	}

	#[test]
	fn parse_rejects_empty_pattern() {
		assert!(matches!(PathPattern::parse(""), Err(J18nError::InvalidPattern { .. })));
	}

	#[test]
	fn parse_rejects_double_dot() {
		assert!(matches!(
			PathPattern::parse("a..b"),
			Err(J18nError::InvalidPattern { .. })
		));
	}

	#[test]
	fn key_matches_any_returns_true_when_any_pattern_matches() {
		let patterns = vec![PathPattern::parse("a").unwrap(), PathPattern::parse("b.**").unwrap()];

		assert!(key_matches_any("a", &patterns));
		assert!(key_matches_any("b.x.y", &patterns));
		assert!(!key_matches_any("c", &patterns));
	}
}
