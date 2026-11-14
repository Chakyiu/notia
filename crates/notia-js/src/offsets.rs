//! Offsets into text as JavaScript counts them, in UTF-16 code units, and
//! as Rust does, in bytes.

/// The UTF-16 offset of a byte offset. A byte offset inside a character
/// means the start of that character.
pub(crate) fn to_utf16(text: &str, byte: usize) -> usize {
    let mut byte = byte.min(text.len());
    while !text.is_char_boundary(byte) {
        byte -= 1;
    }
    text[..byte].encode_utf16().count()
}

/// The byte offset of a UTF-16 offset, clamped to the text. An offset
/// between the halves of a surrogate pair means the start of the character
/// they encode.
pub(crate) fn from_utf16(text: &str, units: usize) -> usize {
    let mut counted = 0;
    for (byte, ch) in text.char_indices() {
        counted += ch.len_utf16();
        if counted > units {
            return byte;
        }
    }
    text.len()
}

#[cfg(test)]
mod tests {
    use super::*;

    // Bytes: a 0, é 1..3, 😀 3..7, b 7..8. UTF-16: a 0, é 1, 😀 2..4, b 4.
    const TEXT: &str = "aé😀b";

    #[test]
    fn counts_utf16_code_units() {
        let pairs = [(0, 0), (1, 1), (3, 2), (7, 4), (8, 5)];
        for (byte, units) in pairs {
            assert_eq!(to_utf16(TEXT, byte), units, "byte {byte}");
            assert_eq!(from_utf16(TEXT, units), byte, "unit {units}");
        }
    }

    #[test]
    fn rounds_down_inside_a_character_and_clamps() {
        assert_eq!(to_utf16(TEXT, 2), 1);
        assert_eq!(to_utf16(TEXT, 99), 5);
        assert_eq!(from_utf16(TEXT, 3), 3);
        assert_eq!(from_utf16(TEXT, 99), 8);
        assert_eq!(from_utf16("", 1), 0);
    }
}
