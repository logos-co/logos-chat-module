//! The rules a display name meets before this module publishes it, as its own
//! account's name. They are the Accounts app's (logos-accounts-ui
//! `rust-core/src/manager.rs`), copied so that both write the same kind of name.

/// The longest name, in bytes of UTF-8. The log caps its payload, not an entry,
/// so the Accounts app caps a name where it stops being one.
const MAX_BYTES: usize = 64;

/// Why a display name is refused.
#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
pub(crate) enum DisplayNameError {
    #[error("a display name needs at least one visible character")]
    Empty,
    #[error(
        "a display name cannot hold line breaks, control characters or text-direction overrides"
    )]
    Characters,
    #[error("a display name is limited to 64 bytes of UTF-8, and this one is {0}")]
    TooLong(usize),
}

/// The name as it will be published: trimmed, and refused when it would render
/// as nothing, break a line, or turn the text around it.
pub(crate) fn validate(name: &str) -> Result<&str, DisplayNameError> {
    let name = name.trim();
    if name.chars().any(moves_text) {
        return Err(DisplayNameError::Characters);
    }
    // Invisible characters are allowed, since emoji sequences need the joiner
    // and the variation selectors, but a name made only of them shows nothing.
    if name.chars().all(|c| c.is_whitespace() || invisible(c)) {
        return Err(DisplayNameError::Empty);
    }
    if name.len() > MAX_BYTES {
        return Err(DisplayNameError::TooLong(name.len()));
    }
    Ok(name)
}

/// A character that breaks a line or turns the text around it.
fn moves_text(c: char) -> bool {
    c.is_control()
        || matches!(
            c,
            '\u{2028}' | '\u{2029}' | '\u{202A}'..='\u{202E}' | '\u{2066}'..='\u{2069}'
        )
}

/// A character that draws nothing: Unicode's Default_Ignorable_Code_Point set,
/// plus the blanks outside it.
fn invisible(c: char) -> bool {
    matches!(
        c,
        '\u{00AD}'
            | '\u{034F}'
            | '\u{061C}'
            | '\u{115F}'..='\u{1160}'
            | '\u{17B4}'..='\u{17B5}'
            | '\u{180B}'..='\u{180F}'
            | '\u{200B}'..='\u{200F}'
            | '\u{202A}'..='\u{202E}'
            | '\u{2060}'..='\u{206F}'
            | '\u{3164}'
            | '\u{FE00}'..='\u{FE0F}'
            | '\u{FEFF}'
            | '\u{FFA0}'
            | '\u{FFF0}'..='\u{FFF8}'
            | '\u{1BCA0}'..='\u{1BCA3}'
            | '\u{1D173}'..='\u{1D17A}'
            | '\u{E0000}'..='\u{E0FFF}'
            // The braille blank and the interlinear annotation marks.
            | '\u{2800}'
            | '\u{FFF9}'..='\u{FFFB}'
    )
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_name_that_shows_nothing_or_moves_text_is_refused() {
        for name in [
            "",
            " \u{200b} ",
            "\u{2060}\u{200d}",
            "\u{3164}",
            "\u{fe0f}",
            "\u{e0053}\u{e007f}",
            "\u{2800}",
            "\u{1d173}",
        ] {
            assert_eq!(validate(name), Err(DisplayNameError::Empty), "{name:?}");
        }
        for name in [
            "Saro\nRaya",
            "Saro\u{2028}Raya",
            "\u{202e}oraS",
            "Saro\u{2067}",
        ] {
            assert_eq!(
                validate(name),
                Err(DisplayNameError::Characters),
                "{name:?}"
            );
        }
        assert_eq!(validate(" 👩\u{200d}💻 Saro "), Ok("👩\u{200d}💻 Saro"));
        assert_eq!(validate("❤\u{fe0f}"), Ok("❤\u{fe0f}"));
    }

    #[test]
    fn a_name_over_64_bytes_is_refused() {
        assert_eq!(validate(&"a".repeat(64)), Ok("a".repeat(64).as_str()));
        assert_eq!(
            validate(&"é".repeat(33)),
            Err(DisplayNameError::TooLong(66))
        );
    }
}
