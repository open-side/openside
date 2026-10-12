//! Conservative secret detection applied to every memory write. A false positive only
//! rejects a write; a false negative would persist a credential, so patterns err wide.
//!
//! Bounded normalization: besides the raw text, one decoding pass of each supported
//! encoding is screened (invisible characters removed, percent-encoding, JSON/backslash
//! escapes, HTML entities), alone and after invisible-character removal. Nested or other
//! encodings (base64, compression, splitting a secret across separate writes) are not
//! detected; this is a guard against accidental storage, not an exfiltration filter.

use std::borrow::Cow;

use std::sync::LazyLock;

use regex::{Regex, RegexSet};

const PATTERNS: &[&str] = &[
    // PEM / PGP private key blocks.
    r"-----BEGIN [A-Z0-9 ]*PRIVATE KEY( BLOCK)?-----",
    // JSON Web Tokens (header.payload.signature, base64url).
    r"\beyJ[A-Za-z0-9_-]{5,}\.[A-Za-z0-9_-]{5,}\.[A-Za-z0-9_-]{5,}",
    // Provider API keys and tokens.
    r"\bsk-[A-Za-z0-9_-]{16,}",
    r"\b[sr]k_(live|test)_[A-Za-z0-9]{16,}",
    r"\b(AKIA|ASIA)[0-9A-Z]{16}\b",
    r"\bgh[pousr]_[A-Za-z0-9]{30,}",
    r"\bgithub_pat_[A-Za-z0-9_]{30,}",
    r"\bxox[abposr]-[A-Za-z0-9-]{10,}",
    r"\bAIza[0-9A-Za-z_-]{35}",
    r"\bglpat-[A-Za-z0-9_-]{20,}",
    r"\bhf_[A-Za-z0-9]{30,}",
    r"\bnpm_[A-Za-z0-9]{36}",
    r"(?i)\bbearer\s+[A-Za-z0-9._~+/=-]{20,}",
    // `password=...` style pairs, including quoted JSON/YAML keys and values.
    r#"(?i)\b(password|passwd|pwd|passphrase|비밀번호)["']?\s*[:=]\s*["']?[^\s"']+"#,
    // Generic credential assignments with a token-like value.
    r#"(?i)\b(api[_-]?key|secret[_-]?key|client[_-]?secret|access[_-]?token|auth[_-]?token|refresh[_-]?token|private[_-]?key|token|secret)["']?\s*[:=]\s*["']?[A-Za-z0-9._~+/-]{12,}"#,
];

// Patterns are compile-time constants; `every_pattern_compiles` proves the fallbacks unused.
static SET: LazyLock<RegexSet> =
    LazyLock::new(|| RegexSet::new(PATTERNS).unwrap_or_else(|_| RegexSet::empty()));
static CARD: LazyLock<Option<Regex>> =
    LazyLock::new(|| Regex::new(r"\b\d(?:[ -]?\d){12,18}\b").ok());

/// Whether `text`, or a supported canonical form of it, contains a known secret shape or a
/// Luhn-valid payment card number.
pub fn looks_secret(text: &str) -> bool {
    variants(text).iter().any(|form| raw_secret(form))
}

fn raw_secret(text: &str) -> bool {
    SET.is_match(text) || contains_card(text)
}

const INVISIBLE: &[char] = &[
    '\u{200B}', '\u{200C}', '\u{200D}', '\u{2060}', '\u{FEFF}', '\u{00AD}',
];

/// Raw text plus one decoding pass of each supported representation (see module docs).
pub(crate) fn variants(text: &str) -> Vec<Cow<'_, str>> {
    let mut bases = vec![Cow::Borrowed(text)];
    if text.contains(INVISIBLE) {
        bases.push(Cow::Owned(text.replace(INVISIBLE, "")));
    }
    let mut forms = Vec::with_capacity(bases.len() * 4);
    for base in &bases {
        if base.contains('%') {
            forms.push(Cow::Owned(percent_decode(base)));
        }
        if base.contains('\\') {
            forms.push(Cow::Owned(unescape(base)));
        }
        if base.contains('&') {
            forms.push(Cow::Owned(html_decode(base)));
        }
    }
    bases.extend(forms);
    bases
}

fn percent_decode(text: &str) -> String {
    let bytes = text.as_bytes();
    let mut out = Vec::with_capacity(bytes.len());
    let mut i = 0;
    while i < bytes.len() {
        let hex = (bytes[i] == b'%' && i + 2 < bytes.len())
            .then(|| std::str::from_utf8(&bytes[i + 1..i + 3]).ok())
            .flatten()
            .and_then(|digits| u8::from_str_radix(digits, 16).ok());
        match hex {
            Some(byte) => {
                out.push(byte);
                i += 3;
            }
            None => {
                out.push(if bytes[i] == b'+' { b' ' } else { bytes[i] });
                i += 1;
            }
        }
    }
    String::from_utf8_lossy(&out).into_owned()
}

fn unescape(text: &str) -> String {
    let mut out = String::with_capacity(text.len());
    let mut chars = text.chars().peekable();
    while let Some(c) = chars.next() {
        if c != '\\' {
            out.push(c);
            continue;
        }
        match chars.next() {
            Some('n') => out.push('\n'),
            Some('t') => out.push('\t'),
            Some('r') => out.push('\r'),
            Some(radix @ ('u' | 'x')) => {
                let width = if radix == 'u' { 4 } else { 2 };
                let digits: String = (0..width)
                    .filter_map(|_| chars.next_if(char::is_ascii_hexdigit))
                    .collect();
                match u32::from_str_radix(&digits, 16)
                    .ok()
                    .and_then(char::from_u32)
                {
                    Some(decoded) if digits.len() == width => out.push(decoded),
                    _ => {
                        out.push(radix);
                        out.push_str(&digits);
                    }
                }
            }
            Some(other) => out.push(other),
            None => out.push('\\'),
        }
    }
    out
}

fn html_decode(text: &str) -> String {
    let mut out = String::with_capacity(text.len());
    let mut rest = text;
    while let Some(at) = rest.find('&') {
        out.push_str(&rest[..at]);
        rest = &rest[at..];
        // Search bytes: a fixed byte window may end inside a multibyte character, and ';' is
        // ASCII, so any position found is a valid boundary.
        let window = &rest.as_bytes()[..rest.len().min(12)];
        let Some(end) = window.iter().position(|&byte| byte == b';') else {
            out.push('&');
            rest = &rest[1..];
            continue;
        };
        let entity = &rest[1..end];
        let decoded = match entity {
            "quot" => Some('"'),
            "apos" => Some('\''),
            "amp" => Some('&'),
            "lt" => Some('<'),
            "gt" => Some('>'),
            "colon" => Some(':'),
            "equals" => Some('='),
            _ => entity
                .strip_prefix("#x")
                .or_else(|| entity.strip_prefix("#X"))
                .and_then(|hex| u32::from_str_radix(hex, 16).ok())
                .or_else(|| entity.strip_prefix('#').and_then(|dec| dec.parse().ok()))
                .and_then(char::from_u32),
        };
        match decoded {
            Some(c) => {
                out.push(c);
                rest = &rest[end + 1..];
            }
            None => {
                out.push('&');
                rest = &rest[1..];
            }
        }
    }
    out.push_str(rest);
    out
}

fn contains_card(text: &str) -> bool {
    let Some(regex) = CARD.as_ref() else {
        return false;
    };
    regex.find_iter(text).any(|candidate| {
        let digits: Vec<u32> = candidate
            .as_str()
            .chars()
            .filter_map(|c| c.to_digit(10))
            .collect();
        (13..=19).contains(&digits.len())
            && digits.iter().any(|digit| *digit != digits[0])
            && luhn(&digits)
    })
}

fn luhn(digits: &[u32]) -> bool {
    let sum: u32 = digits
        .iter()
        .rev()
        .enumerate()
        .map(|(index, digit)| {
            if index % 2 == 1 {
                let doubled = digit * 2;
                if doubled > 9 {
                    doubled - 9
                } else {
                    doubled
                }
            } else {
                *digit
            }
        })
        .sum();
    sum % 10 == 0
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn every_pattern_compiles() {
        assert_eq!(SET.len(), PATTERNS.len());
        assert!(CARD.is_some());
    }

    #[test]
    fn detects_secret_shapes_and_allows_ordinary_text() {
        for secret in [
            "key sk-ant-api03-abcdefghijklmnopqrstuvwx",
            "aws AKIAABCDEFGHIJKLMNOP here",
            "ghp_abcdefghijklmnopqrstuvwxyz0123456789",
            "eyJhbGciOiJIUzI1NiJ9.eyJzdWIiOiIxMjM0In0.c2lnbmF0dXJlLXZhbHVl",
            "-----BEGIN OPENSSH PRIVATE KEY-----\nabc",
            "login with password=hunter2",
            "PASSWORD: correct-horse",
            "card 4111 1111 1111 1111 exp",
            "api_key = 'abcd1234efgh5678'",
            "Authorization: Bearer abcdefghijklmnopqrstuvwxyz012345",
        ] {
            assert!(looks_secret(secret), "missed {secret:?}");
        }
        for text in [
            "The user prefers dark mode and short answers.",
            "Order 1234567890123 arrives on 2026-10-11",
            "password manager recommendations",
            "call 010-1234-5678 tomorrow",
            "token budget is 4000",
        ] {
            assert!(!looks_secret(text), "false positive {text:?}");
        }
    }

    #[test]
    fn detects_structured_and_encoded_credentials() {
        for secret in [
            r#"{"password": "hunter2"}"#,
            r#"{'Passwd':'swordfish'}"#,
            "token%3Dghp_abcdefghijklmnopqrstuvwxyz0123456789",
            "gh\u{200B}p_abcdefghijklmnopqrstuvwxyz0123456789",
            r"\u0067hp_abcdefghijklmnopqrstuvwxyz0123456789",
            "password&#61;hunter2",
            "s%6B-ant-api03-abcdefghijklmnopqrstuvwx",
            "4111\u{00AD}1111 1111 1111",
        ] {
            assert!(looks_secret(secret), "missed {secret:?}");
        }
        for text in ["50% off, 100% done", r"C:\Users\docs", "Tom &amp; Jerry"] {
            assert!(!looks_secret(text), "false positive {text:?}");
        }
    }

    #[test]
    fn multilingual_text_around_ampersands_and_escapes_never_panics() {
        // Each case puts a multibyte character across the 12-byte entity window or next to
        // an escape introducer.
        for text in [
            "&가나다라마바사",
            "R&D 팀 회의록: 다음 주 화요일",
            "&é;",
            "東京&大阪&名古屋;",
            "ab&😀😀😀",
            "%가%E0%",
            "\\u가나 \\x한",
            "Tom &amp; 제리 &#xD55C; &#54620;",
        ] {
            assert!(!looks_secret(text), "false positive {text:?}");
        }
        assert!(looks_secret("비밀번호&#61;한글패스워드1"));
    }
}
