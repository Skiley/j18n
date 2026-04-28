use crate::error::J18nError;

#[derive(Clone, Copy, Debug, Eq, Hash, PartialEq)]
pub struct Language {
	iso_639_code: &'static str,
	language_name: &'static str,
}

impl Language {
	pub fn iso_639_code(&self) -> &'static str {
		self.iso_639_code
	}

	pub fn language_name(&self) -> &'static str {
		self.language_name
	}

	pub fn from_iso_639_code(code: &str) -> Result<Self, J18nError> {
		ALL_LANGUAGES
			.iter()
			.find(|language| language.iso_639_code == code)
			.copied()
			.ok_or_else(|| J18nError::LanguageNotFound { code: code.to_string() })
	}

	pub const ENGLISH: Self = Self {
		iso_639_code: "en",
		language_name: "English",
	};
}

const fn lang(iso_639_code: &'static str, language_name: &'static str) -> Language {
	Language {
		iso_639_code,
		language_name,
	}
}

pub const ALL_LANGUAGES: &[Language] = &[
	lang("af", "Afrikaans"),
	lang("sq", "Albanian"),
	lang("am", "Amharic"),
	lang("ar", "Arabic"),
	lang("hy", "Armenian"),
	lang("as", "Assamese"),
	lang("ay", "Aymara"),
	lang("az", "Azerbaijani"),
	lang("bm", "Bambara"),
	lang("eu", "Basque"),
	lang("be", "Belarusian"),
	lang("bn", "Bengali"),
	lang("bho", "Bhojpuri"),
	lang("bs", "Bosnian"),
	lang("bg", "Bulgarian"),
	lang("ca", "Catalan"),
	lang("ceb", "Cebuano"),
	lang("zh-CN", "Chinese (Simplified)"),
	lang("zh-TW", "Chinese (Traditional)"),
	lang("co", "Corsican"),
	lang("hr", "Croatian"),
	lang("cs", "Czech"),
	lang("da", "Danish"),
	lang("dv", "Dhivehi"),
	lang("doi", "Dogri"),
	lang("nl", "Dutch"),
	lang("en", "English"),
	lang("eo", "Esperanto"),
	lang("et", "Estonian"),
	lang("ee", "Ewe"),
	lang("fil", "Filipino (Tagalog)"),
	lang("fi", "Finnish"),
	lang("fr", "French"),
	lang("fy", "Frisian"),
	lang("gl", "Galician"),
	lang("ka", "Georgian"),
	lang("de", "German"),
	lang("el", "Greek"),
	lang("gn", "Guarani"),
	lang("gu", "Gujarati"),
	lang("ht", "Haitian Creole"),
	lang("ha", "Hausa"),
	lang("haw", "Hawaiian"),
	lang("he", "Hebrew"),
	lang("hi", "Hindi"),
	lang("hmn", "Hmong"),
	lang("hu", "Hungarian"),
	lang("is", "Icelandic"),
	lang("ig", "Igbo"),
	lang("ilo", "Ilocano"),
	lang("id", "Indonesian"),
	lang("ga", "Irish"),
	lang("it", "Italian"),
	lang("ja", "Japanese"),
	lang("jv", "Javanese"),
	lang("kn", "Kannada"),
	lang("kk", "Kazakh"),
	lang("km", "Khmer"),
	lang("rw", "Kinyarwanda"),
	lang("gom", "Konkani"),
	lang("ko", "Korean"),
	lang("kri", "Krio"),
	lang("ku", "Kurdish"),
	lang("ckb", "Kurdish (Sorani)"),
	lang("ky", "Kyrgyz"),
	lang("lo", "Lao"),
	lang("la", "Latin"),
	lang("lv", "Latvian"),
	lang("ln", "Lingala"),
	lang("lt", "Lithuanian"),
	lang("lg", "Luganda"),
	lang("lb", "Luxembourgish"),
	lang("mk", "Macedonian"),
	lang("mai", "Maithili"),
	lang("mg", "Malagasy"),
	lang("ms", "Malay"),
	lang("ml", "Malayalam"),
	lang("mt", "Maltese"),
	lang("mi", "Maori"),
	lang("mr", "Marathi"),
	lang("mni-Mtei", "Meiteilon (Manipuri)"),
	lang("lus", "Mizo"),
	lang("mn", "Mongolian"),
	lang("my", "Myanmar (Burmese)"),
	lang("ne", "Nepali"),
	lang("no", "Norwegian"),
	lang("ny", "Nyanja (Chichewa)"),
	lang("or", "Odia (Oriya)"),
	lang("om", "Oromo"),
	lang("ps", "Pashto"),
	lang("fa", "Persian"),
	lang("pl", "Polish"),
	lang("pt", "Portuguese"),
	lang("pa", "Punjabi"),
	lang("qu", "Quechua"),
	lang("ro", "Romanian"),
	lang("ru", "Russian"),
	lang("sm", "Samoan"),
	lang("sa", "Sanskrit"),
	lang("gd", "Scots Gaelic"),
	lang("nso", "Sepedi"),
	lang("sr", "Serbian"),
	lang("st", "Sesotho"),
	lang("sn", "Shona"),
	lang("sd", "Sindhi"),
	lang("si", "Sinhala (Sinhalese)"),
	lang("sk", "Slovak"),
	lang("sl", "Slovenian"),
	lang("so", "Somali"),
	lang("es", "Spanish"),
	lang("su", "Sundanese"),
	lang("sw", "Swahili"),
	lang("sv", "Swedish"),
	lang("tl", "Tagalog (Filipino)"),
	lang("tg", "Tajik"),
	lang("ta", "Tamil"),
	lang("tt", "Tatar"),
	lang("te", "Telugu"),
	lang("th", "Thai"),
	lang("ti", "Tigrinya"),
	lang("ts", "Tsonga"),
	lang("tr", "Turkish"),
	lang("tk", "Turkmen"),
	lang("ak", "Twi (Akan)"),
	lang("uk", "Ukrainian"),
	lang("ur", "Urdu"),
	lang("ug", "Uyghur"),
	lang("uz", "Uzbek"),
	lang("vi", "Vietnamese"),
	lang("cy", "Welsh"),
	lang("xh", "Xhosa"),
	lang("yi", "Yiddish"),
	lang("yo", "Yoruba"),
	lang("zu", "Zulu"),
];

#[cfg(test)]
mod tests {
	use super::*;
	use std::collections::HashSet;

	#[test]
	fn from_iso_639_code_finds_known_language() {
		let language = Language::from_iso_639_code("en").unwrap();

		assert_eq!(language.iso_639_code(), "en");
		assert_eq!(language.language_name(), "English");
	}

	#[test]
	fn from_iso_639_code_distinguishes_chinese_variants() {
		let simplified = Language::from_iso_639_code("zh-CN").unwrap();
		let traditional = Language::from_iso_639_code("zh-TW").unwrap();

		assert_eq!(simplified.language_name(), "Chinese (Simplified)");
		assert_eq!(traditional.language_name(), "Chinese (Traditional)");
	}

	#[test]
	fn from_iso_639_code_returns_error_for_unknown_code() {
		let err = Language::from_iso_639_code("xx").unwrap_err();

		match err {
			crate::J18nError::LanguageNotFound { code } => assert_eq!(code, "xx"),
			other => panic!("unexpected error: {other:?}"),
		}
	}

	#[test]
	fn english_constant_matches_lookup() {
		let looked_up = Language::from_iso_639_code("en").unwrap();

		assert_eq!(Language::ENGLISH, looked_up);
	}

	#[test]
	fn all_languages_have_unique_iso_codes() {
		let mut seen = HashSet::new();

		for language in ALL_LANGUAGES {
			assert!(
				seen.insert(language.iso_639_code()),
				"duplicate ISO-639 code: {}",
				language.iso_639_code()
			);
		}
	}

	#[test]
	fn portuguese_filipino_and_hebrew_are_present() {
		for code in ["pt", "fil", "he"] {
			assert!(
				Language::from_iso_639_code(code).is_ok(),
				"missing language for code {code}"
			);
		}
	}
}
