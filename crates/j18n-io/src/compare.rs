use std::cmp::Ordering;

pub fn natural_key_cmp(a: &str, b: &str) -> Ordering {
	let a_bytes = a.as_bytes();
	let b_bytes = b.as_bytes();
	let mut ai = 0;
	let mut bi = 0;
	let mut case_tiebreaker = Ordering::Equal;

	while ai < a_bytes.len() && bi < b_bytes.len() {
		let ac = a_bytes[ai];
		let bc = b_bytes[bi];

		if ac.is_ascii_digit() && bc.is_ascii_digit() {
			let a_start = ai;
			let b_start = bi;

			while ai < a_bytes.len() && a_bytes[ai].is_ascii_digit() {
				ai += 1;
			}
			while bi < b_bytes.len() && b_bytes[bi].is_ascii_digit() {
				bi += 1;
			}

			let a_num = strip_leading_zeros(&a_bytes[a_start..ai]);
			let b_num = strip_leading_zeros(&b_bytes[b_start..bi]);

			match a_num.len().cmp(&b_num.len()) {
				Ordering::Equal => {}
				non_eq => return non_eq,
			}

			match a_num.cmp(b_num) {
				Ordering::Equal => {}
				non_eq => return non_eq,
			}

			let a_run = ai - a_start;
			let b_run = bi - b_start;

			match a_run.cmp(&b_run) {
				Ordering::Equal => {}
				non_eq => return non_eq,
			}
		} else {
			let a_lower = ac.to_ascii_lowercase();
			let b_lower = bc.to_ascii_lowercase();

			match a_lower.cmp(&b_lower) {
				Ordering::Equal => {
					if case_tiebreaker.is_eq() && ac != bc {
						case_tiebreaker = ac.cmp(&bc);
					}
					ai += 1;
					bi += 1;
				}
				non_eq => return non_eq,
			}
		}
	}

	if ai < a_bytes.len() {
		if !case_tiebreaker.is_eq() {
			return case_tiebreaker;
		}

		return Ordering::Greater;
	}

	if bi < b_bytes.len() {
		if !case_tiebreaker.is_eq() {
			return case_tiebreaker;
		}

		return Ordering::Less;
	}

	case_tiebreaker
}

fn strip_leading_zeros(bytes: &[u8]) -> &[u8] {
	let mut index = 0;

	while index < bytes.len() && bytes[index] == b'0' {
		index += 1;
	}

	if index == bytes.len() && !bytes.is_empty() {
		&bytes[bytes.len() - 1..]
	} else {
		&bytes[index..]
	}
}

#[cfg(test)]
mod tests {
	use super::*;

	#[test]
	fn digit_runs_compare_numerically() {
		let mut keys = vec!["0", "1", "10", "11", "2", "3"];

		keys.sort_by(|a, b| natural_key_cmp(a, b));

		assert_eq!(keys, vec!["0", "1", "2", "3", "10", "11"]);
	}

	#[test]
	fn different_letters_at_diverging_position_use_case_insensitive_order() {
		assert_eq!(natural_key_cmp("none", "noSuggestions"), Ordering::Less);
		assert_eq!(natural_key_cmp("noSuggestions", "none"), Ordering::Greater);
	}

	#[test]
	fn camel_case_variant_orders_before_pure_lowercase_when_one_is_prefix_of_other() {
		assert_eq!(natural_key_cmp("typeSelection", "types"), Ordering::Less);
		assert_eq!(natural_key_cmp("types", "typeSelection"), Ordering::Greater);
	}

	#[test]
	fn returns_equal_for_identical_strings() {
		assert_eq!(natural_key_cmp("abc", "abc"), Ordering::Equal);
	}

	#[test]
	fn shorter_string_with_shared_prefix_comes_first_when_no_case_difference() {
		assert_eq!(natural_key_cmp("foo", "foobar"), Ordering::Less);
		assert_eq!(natural_key_cmp("foobar", "foo"), Ordering::Greater);
	}

	#[test]
	fn mixed_letter_and_digit_keys_sort_naturally() {
		let mut keys = vec!["item10", "item2", "item1", "item20"];

		keys.sort_by(|a, b| natural_key_cmp(a, b));

		assert_eq!(keys, vec!["item1", "item2", "item10", "item20"]);
	}

	#[test]
	fn equal_numbers_with_different_leading_zeros_break_tie_by_run_length() {
		assert_eq!(natural_key_cmp("01", "1"), Ordering::Greater);
		assert_eq!(natural_key_cmp("1", "01"), Ordering::Less);
		assert_eq!(natural_key_cmp("0", "00"), Ordering::Less);
	}

	#[test]
	fn case_only_differences_resolve_with_uppercase_first() {
		assert_eq!(natural_key_cmp("Foo", "foo"), Ordering::Less);
		assert_eq!(natural_key_cmp("foo", "Foo"), Ordering::Greater);
	}

	#[test]
	fn keynames_zero_through_eleven_sort_naturally() {
		let mut keys: Vec<String> = (0..=11).map(|n| n.to_string()).collect();

		keys.sort_by(|a, b| natural_key_cmp(a, b));

		assert_eq!(
			keys,
			vec!["0", "1", "2", "3", "4", "5", "6", "7", "8", "9", "10", "11"]
		);
	}
}
