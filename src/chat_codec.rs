//! Base-236 binary-to-chat encoding.
//!
//! Round-trips arbitrary bytes through a 236-character alphabet of CP437
//! glyphs chosen to survive every transformation MCGalaxy applies to a
//! chat line on its way from the sender, through server-side storage
//! (e.g. `/mb sign <blob>`), and back out as echoed chat to a recipient
//! plugin. See `FORBIDDEN` below for the per-character rationale and
//! MCGalaxy source-file references.
//!
//! The encoded form is a Rust `String` of Unicode chars (each char is
//! the canonical CP437→Unicode mapping of the corresponding alphabet
//! byte), so it also survives logs, JSON, and disk storage.
//!
//! Algorithm is the Base58 recipe retargeted to base 236: treat input
//! bytes as a big-endian BigInt, repeatedly mod by 236 to extract
//! digits, and prepend `ALPHABET[0]` for each leading 0x00 input byte.

use std::{collections::HashMap, error::Error, fmt, sync::LazyLock};

const BASE: u32 = 236;

#[rustfmt::skip]
const FORBIDDEN: &[u8] = &[
    // Server-side command parsers split args on space, so a blob
    // containing space would truncate the surrounding command's arg
    // list. LineWrapper.Wordwrap (LineWrapper.cs:53-61) also picks
    // spaces as preferred line-break points, and FilterChat
    // (Player.Handlers.cs:482) collapses runs of spaces via
    // `Regex.Replace(text, "  +", " ")`.
    b' ',
    // ASCII control bytes 0x09..=0x0D (TAB, LF, VT, FF, CR) survive
    // the wire and the input pipeline, but MessageBlock.Get
    // (Blocks/Extended/MessageBlock.cs:151) does `msg.Trim()` on the
    // stored CP437 bytes before `Cp437ToUnicode()`, and .NET's
    // `String.Trim()` strips all of these as Unicode whitespace.
    // CP437 maps these to ○ (0x09), ◙ (0x0A), ♂ (0x0B), ♀ (0x0C),
    // ♪ (0x0D) — so a leading/trailing one of those glyphs in a
    // `/mb sign <blob>` payload silently disappears on click-out.
    0x09, 0x0A, 0x0B, 0x0C, 0x0D,
    // Partial-message tails: FilterChat (Player.Handlers.cs:486-492)
    // hijacks any message ending in " >", " /", " <", or " \\" as a
    // continuation buffer. Even though our blob never contains a
    // space, the surrounding command form (e.g. `/mb sign <blob>`)
    // puts a space immediately before the blob — so a length-1 blob
    // consisting solely of one of these chars would trigger it.
    b'/', b'\\', b'<', b'>',
    // EmotesHandler.Replace (EmotesHandler.cs:53-90) walks the body
    // looking for `(keyword)` and substitutes the matching CP437 glyph
    // (`(smile)` → ☻, `(heart)` → ♥, etc.) on every echoed message.
    b'(', b')',
    // MCGalaxy's "Not Awesome Script" scripting layer uses `{var}` for
    // variable substitution in scripted commands.
    b'{', b'}',
    // MessageBlock.Handle (Blocks/Extended/MessageBlock.cs:31) does
    // `message.Replace("@p", p.name)` before display, and the same
    // `@p` convention shows up in every death-message template
    // (Blocks/Block.CoreProps.cs, Games/Weapons/*, Player.Handlers.cs).
    // Substring match — `@p` anywhere in the blob gets clobbered.
    b'@',
    // Color codes: `&X` is the wire color marker; Colors.Escape
    // (Colors.cs:189-202) rewrites `%X` → `&X` before send, and
    // LineWrapper.CleanupColors (LineWrapper.cs:169-220) dedupes
    // adjacent codes and turns a lone `&` into `%`.
    b'%', b'&',
    // ChatTokens.Apply (ChatTokens.cs:33-47) substring-replaces 27
    // standard `$tokens` (`$name`, `$time`, `$server`, …) plus any
    // admin-defined custom tokens via `StringBuilder.Replace`. Runs
    // on every outgoing chat through Chat.Format → pl.Message
    // (Player.Networking.cs:47), so blobs echoed back from sign
    // content are not exempt.
    b'$',
];

pub const ALLOWED_BYTES: [u8; 236] = build_allowed();

const fn build_allowed() -> [u8; 236] {
    let mut out = [0u8; 236];
    let mut idx = 0;
    let mut b: u8 = 0x01;
    loop {
        let mut forbidden = false;
        let mut i = 0;
        while i < FORBIDDEN.len() {
            if FORBIDDEN[i] == b {
                forbidden = true;
                break;
            }
            i += 1;
        }
        if !forbidden {
            out[idx] = b;
            idx += 1;
        }
        if b == 0xFE {
            break;
        }
        b += 1;
    }
    assert!(idx == 236);
    out
}

/// CP437 → Unicode mapping. Mirrors `controlChars` + ASCII identity +
/// `extendedChars` from `rust-classicube-sys/src/string.rs`; canonical
/// source is `ClassiCube/src/String.c` lines 505–529.
const CP437: [char; 256] = build_cp437();

const CONTROL_CHARS: [u16; 32] = [
    0x0000, 0x263A, 0x263B, 0x2665, 0x2666, 0x2663, 0x2660, 0x2022, 0x25D8, 0x25CB, 0x25D9, 0x2642,
    0x2640, 0x266A, 0x266B, 0x263C, 0x25BA, 0x25C4, 0x2195, 0x203C, 0x00B6, 0x00A7, 0x25AC, 0x21A8,
    0x2191, 0x2193, 0x2192, 0x2190, 0x221F, 0x2194, 0x25B2, 0x25BC,
];

const EXTENDED_CHARS: [u16; 129] = [
    0x2302, 0x00C7, 0x00FC, 0x00E9, 0x00E2, 0x00E4, 0x00E0, 0x00E5, 0x00E7, 0x00EA, 0x00EB, 0x00E8,
    0x00EF, 0x00EE, 0x00EC, 0x00C4, 0x00C5, 0x00C9, 0x00E6, 0x00C6, 0x00F4, 0x00F6, 0x00F2, 0x00FB,
    0x00F9, 0x00FF, 0x00D6, 0x00DC, 0x00A2, 0x00A3, 0x00A5, 0x20A7, 0x0192, 0x00E1, 0x00ED, 0x00F3,
    0x00FA, 0x00F1, 0x00D1, 0x00AA, 0x00BA, 0x00BF, 0x2310, 0x00AC, 0x00BD, 0x00BC, 0x00A1, 0x00AB,
    0x00BB, 0x2591, 0x2592, 0x2593, 0x2502, 0x2524, 0x2561, 0x2562, 0x2556, 0x2555, 0x2563, 0x2551,
    0x2557, 0x255D, 0x255C, 0x255B, 0x2510, 0x2514, 0x2534, 0x252C, 0x251C, 0x2500, 0x253C, 0x255E,
    0x255F, 0x255A, 0x2554, 0x2569, 0x2566, 0x2560, 0x2550, 0x256C, 0x2567, 0x2568, 0x2564, 0x2565,
    0x2559, 0x2558, 0x2552, 0x2553, 0x256B, 0x256A, 0x2518, 0x250C, 0x2588, 0x2584, 0x258C, 0x2590,
    0x2580, 0x03B1, 0x00DF, 0x0393, 0x03C0, 0x03A3, 0x03C3, 0x00B5, 0x03C4, 0x03A6, 0x0398, 0x03A9,
    0x03B4, 0x221E, 0x03C6, 0x03B5, 0x2229, 0x2261, 0x00B1, 0x2265, 0x2264, 0x2320, 0x2321, 0x00F7,
    0x2248, 0x00B0, 0x2219, 0x00B7, 0x221A, 0x207F, 0x00B2, 0x25A0, 0x00A0,
];

const fn build_cp437() -> [char; 256] {
    let mut t = ['\0'; 256];

    let mut i = 0;
    while i < 32 {
        t[i] = match char::from_u32(CONTROL_CHARS[i] as u32) {
            Some(c) => c,
            None => '\0',
        };
        i += 1;
    }

    let mut b: u8 = 0x20;
    while b <= 0x7E {
        t[b as usize] = b as char;
        b += 1;
    }

    let mut i = 0;
    while i < 129 {
        t[i + 0x7F] = match char::from_u32(EXTENDED_CHARS[i] as u32) {
            Some(c) => c,
            None => '\0',
        };
        i += 1;
    }

    t
}

pub const ALPHABET: [char; 236] = build_alphabet();

const fn build_alphabet() -> [char; 236] {
    let mut out = ['\0'; 236];
    let mut i = 0;
    while i < 236 {
        out[i] = CP437[ALLOWED_BYTES[i] as usize];
        i += 1;
    }
    out
}

static DECODE: LazyLock<HashMap<char, u8>> = LazyLock::new(|| {
    ALPHABET
        .iter()
        .enumerate()
        .map(|(i, &c)| (c, u8::try_from(i).expect("alphabet index fits in u8")))
        .collect()
});

#[derive(Debug, Eq, PartialEq, Clone, Copy)]
pub struct DecodeError {
    pub char: char,
}

impl fmt::Display for DecodeError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "character {:?} is not in the alphabet", self.char)
    }
}

impl Error for DecodeError {}

pub fn encode(input: &[u8]) -> String {
    let zeros = input.iter().take_while(|&&b| b == 0).count();

    let mut digits: Vec<u16> = Vec::with_capacity(input.len() * 138 / 100 + 1);
    for &byte in &input[zeros..] {
        let mut carry = u32::from(byte);
        for d in &mut digits {
            carry += u32::from(*d) << 8;
            *d = (carry % BASE) as u16;
            carry /= BASE;
        }
        while carry > 0 {
            digits.push((carry % BASE) as u16);
            carry /= BASE;
        }
    }

    let mut s = String::with_capacity((zeros + digits.len()) * 2);
    for _ in 0..zeros {
        s.push(ALPHABET[0]);
    }
    for &d in digits.iter().rev() {
        s.push(ALPHABET[d as usize]);
    }
    s
}

#[expect(
    clippy::cast_possible_truncation,
    reason = "carry & 0xFF is range-checked to fit in u16/u8"
)]
pub fn decode(input: &str) -> Result<Vec<u8>, DecodeError> {
    let table = &*DECODE;

    let digits: Vec<u8> = input
        .chars()
        .map(|c| table.get(&c).copied().ok_or(DecodeError { char: c }))
        .collect::<Result<Vec<_>, _>>()?;

    let zeros = digits.iter().take_while(|&&d| d == 0).count();

    let mut bytes: Vec<u16> = Vec::with_capacity(digits.len());
    for &digit in &digits[zeros..] {
        let mut carry = u32::from(digit);
        for b in &mut bytes {
            carry += u32::from(*b) * BASE;
            *b = (carry & 0xFF) as u16;
            carry >>= 8;
        }
        while carry > 0 {
            bytes.push((carry & 0xFF) as u16);
            carry >>= 8;
        }
    }

    let mut out = Vec::with_capacity(zeros + bytes.len());
    out.resize(zeros, 0u8);
    out.extend(bytes.iter().rev().map(|&b| b as u8));
    Ok(out)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn allowed_bytes_length_is_236() {
        assert_eq!(ALLOWED_BYTES.len(), 236);
    }

    #[test]
    fn allowed_bytes_exclude_forbidden() {
        for &b in FORBIDDEN {
            assert!(
                !ALLOWED_BYTES.contains(&b),
                "forbidden byte {:#04x} found in alphabet",
                b
            );
        }
        assert!(!ALLOWED_BYTES.contains(&0x00));
        assert!(!ALLOWED_BYTES.contains(&0xFF));
    }

    #[test]
    fn allowed_bytes_are_ascending_and_unique() {
        for w in ALLOWED_BYTES.windows(2) {
            assert!(w[0] < w[1], "not strictly ascending: {:?}", w);
        }
    }

    #[test]
    fn alphabet_length_is_236() {
        assert_eq!(ALPHABET.len(), 236);
    }

    #[test]
    fn alphabet_chars_are_unique() {
        let set: std::collections::HashSet<char> = ALPHABET.iter().copied().collect();
        assert_eq!(set.len(), ALPHABET.len(), "alphabet has duplicate chars");
    }

    #[test]
    fn alphabet_chars_match_cp437() {
        for (i, &b) in ALLOWED_BYTES.iter().enumerate() {
            assert_eq!(ALPHABET[i], CP437[b as usize]);
        }
    }

    #[test]
    fn round_trip_empty() {
        assert_eq!(encode(&[]), "");
        assert_eq!(decode("").unwrap(), Vec::<u8>::new());
    }

    #[test]
    fn round_trip_each_single_byte() {
        for b in 0u8..=255 {
            let encoded = encode(&[b]);
            let decoded = decode(&encoded).unwrap();
            assert_eq!(decoded, [b], "round-trip failed for byte {:#04x}", b);
        }
    }

    #[test]
    fn round_trip_all_bytes_in_order() {
        let input: Vec<u8> = (0u8..=255).collect();
        let encoded = encode(&input);
        let decoded = decode(&encoded).unwrap();
        assert_eq!(decoded, input);
    }

    #[test]
    fn round_trip_leading_zeros() {
        let input = [0u8, 0, 0, 1, 2, 3];
        let encoded = encode(&input);
        let prefix: String = std::iter::repeat_n(ALPHABET[0], 3).collect();
        assert!(
            encoded.starts_with(&prefix),
            "expected {} leading-zero markers, got encoded {:?}",
            3,
            encoded
        );
        let decoded = decode(&encoded).unwrap();
        assert_eq!(decoded, input);
    }

    #[test]
    fn round_trip_random() {
        let mut state: u64 = 0x9E37_79B9_7F4A_7C15;
        for _ in 0..100 {
            state = state
                .wrapping_mul(6364136223846793005)
                .wrapping_add(1442695040888963407);
            let len = (state >> 32) as usize % 513;

            let mut input = Vec::with_capacity(len);
            for _ in 0..len {
                state = state
                    .wrapping_mul(6364136223846793005)
                    .wrapping_add(1442695040888963407);
                input.push((state >> 56) as u8);
            }

            let encoded = encode(&input);
            let decoded = decode(&encoded).unwrap();
            assert_eq!(decoded, input, "random round-trip failed at len={}", len);
        }
    }

    #[test]
    fn decode_rejects_forbidden_ascii() {
        for &b in FORBIDDEN {
            let s = (b as char).to_string();
            let err = decode(&s).unwrap_err();
            assert_eq!(err.char, b as char, "expected error char {:?}", b as char);
        }
    }

    #[test]
    fn decode_rejects_unknown_unicode() {
        let err = decode("中").unwrap_err();
        assert_eq!(err.char, '中');
    }

    #[test]
    fn decode_rejects_null_byte() {
        let err = decode("\0").unwrap_err();
        assert_eq!(err.char, '\0');
    }

    #[rustfmt::skip]
    const SINGLE_BYTE_FIXTURES: [&str; 256] = [
        "☺", "☻", "♥", "♦", "♣", "♠", "•", "◘",
        "♫", "☼", "►", "◄", "↕", "‼", "¶", "§",
        "▬", "↨", "↑", "↓", "→", "←", "∟", "↔",
        "▲", "▼", "!", "\"", "#", "'", "*", "+",
        ",", "-", ".", "0", "1", "2", "3", "4",
        "5", "6", "7", "8", "9", ":", ";", "=",
        "?", "A", "B", "C", "D", "E", "F", "G",
        "H", "I", "J", "K", "L", "M", "N", "O",
        "P", "Q", "R", "S", "T", "U", "V", "W",
        "X", "Y", "Z", "[", "]", "^", "_", "`",
        "a", "b", "c", "d", "e", "f", "g", "h",
        "i", "j", "k", "l", "m", "n", "o", "p",
        "q", "r", "s", "t", "u", "v", "w", "x",
        "y", "z", "|", "~", "⌂", "Ç", "ü", "é",
        "â", "ä", "à", "å", "ç", "ê", "ë", "è",
        "ï", "î", "ì", "Ä", "Å", "É", "æ", "Æ",
        "ô", "ö", "ò", "û", "ù", "ÿ", "Ö", "Ü",
        "¢", "£", "¥", "₧", "ƒ", "á", "í", "ó",
        "ú", "ñ", "Ñ", "ª", "º", "¿", "⌐", "¬",
        "½", "¼", "¡", "«", "»", "░", "▒", "▓",
        "│", "┤", "╡", "╢", "╖", "╕", "╣", "║",
        "╗", "╝", "╜", "╛", "┐", "└", "┴", "┬",
        "├", "─", "┼", "╞", "╟", "╚", "╔", "╩",
        "╦", "╠", "═", "╬", "╧", "╨", "╤", "╥",
        "╙", "╘", "╒", "╓", "╫", "╪", "┘", "┌",
        "█", "▄", "▌", "▐", "▀", "α", "ß", "Γ",
        "π", "Σ", "σ", "µ", "τ", "Φ", "Θ", "Ω",
        "δ", "∞", "φ", "ε", "∩", "≡", "±", "≥",
        "≤", "⌠", "⌡", "÷", "≈", "°", "∙", "·",
        "√", "ⁿ", "²", "■", "☻☺", "☻☻", "☻♥", "☻♦",
        "☻♣", "☻♠", "☻•", "☻◘", "☻♫", "☻☼", "☻►", "☻◄",
        "☻↕", "☻‼", "☻¶", "☻§", "☻▬", "☻↨", "☻↑", "☻↓",
    ];

    #[test]
    fn encode_each_single_byte_matches_fixture() {
        for b in 0u8..=255 {
            let encoded = encode(&[b]);
            assert_eq!(
                encoded, SINGLE_BYTE_FIXTURES[b as usize],
                "encode mismatch for byte {:#04x}",
                b
            );
        }
    }
}
