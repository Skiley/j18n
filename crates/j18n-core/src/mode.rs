use std::fmt;

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum GenerationMode {
	Regenerate,
	Sync,
}

impl fmt::Display for GenerationMode {
	fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
		match self {
			Self::Regenerate => f.write_str("REGENERATE"),
			Self::Sync => f.write_str("SYNC"),
		}
	}
}

#[cfg(test)]
mod tests {
	use super::*;

	#[test]
	fn display_uses_uppercase_constant_form() {
		assert_eq!(GenerationMode::Sync.to_string(), "SYNC");
		assert_eq!(GenerationMode::Regenerate.to_string(), "REGENERATE");
	}

	#[test]
	fn modes_are_distinct() {
		assert_ne!(GenerationMode::Sync, GenerationMode::Regenerate);
	}
}
