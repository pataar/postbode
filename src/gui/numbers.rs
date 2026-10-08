//! Counts grouped the way the device's region writes numbers.
use std::sync::OnceLock;

/// The thousands separator for a BCP 47 or POSIX locale name; None when unknown, which shows plain digits.
pub(crate) fn separator_for(locale: &str) -> Option<char> {
    let language = locale
        .split(['-', '_', '.'])
        .next()
        .unwrap_or("")
        .to_ascii_lowercase();
    // ponytail: language-only table; region overrides such as de-CH (apostrophe) or Indian grouping when someone asks
    match language.as_str() {
        "da" | "de" | "el" | "es" | "id" | "it" | "nl" | "pt" | "tr" => Some('.'),
        "en" | "he" | "ja" | "ko" | "th" | "zh" => Some(','),
        "cs" | "fi" | "fr" | "nb" | "pl" | "ru" | "sk" | "sv" | "uk" => Some('\u{202f}'),
        _ => None,
    }
}

pub(crate) fn group(n: u64, separator: Option<char>) -> String {
    let digits = n.to_string();
    let Some(separator) = separator else {
        return digits;
    };
    let mut out = String::with_capacity(digits.len() + digits.len() / 3);
    for (index, digit) in digits.chars().enumerate() {
        if index > 0 && (digits.len() - index).is_multiple_of(3) {
            out.push(separator);
        }
        out.push(digit);
    }
    out
}

/// `n` grouped for this device's locale, read once.
pub(crate) fn count(n: u64) -> String {
    static SEPARATOR: OnceLock<Option<char>> = OnceLock::new();
    let separator =
        *SEPARATOR.get_or_init(|| sys_locale::get_locale().as_deref().and_then(separator_for));
    group(n, separator)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn locales_pick_their_separator_and_unknown_ones_none() {
        assert_eq!(separator_for("nl-NL"), Some('.'));
        assert_eq!(separator_for("nl_NL.UTF-8"), Some('.'));
        assert_eq!(separator_for("de"), Some('.'));
        assert_eq!(separator_for("en-US"), Some(','));
        assert_eq!(separator_for("fr-FR"), Some('\u{202f}'));
        assert_eq!(separator_for("C"), None);
        assert_eq!(separator_for("POSIX"), None);
        assert_eq!(separator_for(""), None);
        assert_eq!(separator_for("xx"), None);
    }

    #[test]
    fn group_inserts_the_separator_every_three_digits() {
        assert_eq!(group(0, Some('.')), "0");
        assert_eq!(group(999, Some('.')), "999");
        assert_eq!(group(1_284, Some('.')), "1.284");
        assert_eq!(group(1_234_567, Some(',')), "1,234,567");
        assert_eq!(group(1_234_567, None), "1234567");
    }
}
