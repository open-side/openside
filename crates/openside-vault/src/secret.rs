use std::fmt;

use base64::{engine::general_purpose, Engine as _};
use hmac::{Hmac, Mac};
use serde::{Deserialize, Serialize};
use sha1::Sha1;
use sha2::{Sha256, Sha512};
use zeroize::Zeroizing;

use crate::Error;

/// A zeroizing UTF-8 secret with no serialization or public value accessor.
///
/// Plaintext is exposed only to a vault's trusted fill callback. Debug and Display
/// never expose the contents. Callers must move owned strings into this wrapper.
pub struct Secret(pub(crate) Zeroizing<String>);

impl Secret {
    /// Take ownership of a secret, zeroizing its allocation on drop.
    pub fn new(value: String) -> Self {
        Self(Zeroizing::new(value))
    }
}

impl fmt::Debug for Secret {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str("[REDACTED]")
    }
}

impl fmt::Display for Secret {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str("[REDACTED]")
    }
}

/// Scrubs exact secrets and common transport encodings from text.
///
/// This is not a general data-loss prevention system: unknown transformations,
/// partial secrets, images and adversarial multi-layer encodings need host-side
/// restrictions. Keep this object alive until all derived output has been scrubbed.
/// [`Self::new`] matches every non-empty explicit secret as a substring whatever its length;
/// callers preserve protocol identifiers by schema location, never by value shape.
/// [`Self::for_cookie`] instead filters short preferences and matches credential tokens.
pub struct Redactor {
    patterns: Vec<Zeroizing<String>>,
    token_boundaries: bool,
}

impl fmt::Debug for Redactor {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str("[REDACTED]")
    }
}

impl Redactor {
    /// Build an independently zeroizing filter for the supplied live secrets.
    pub fn new<'a>(secrets: impl IntoIterator<Item = &'a Secret>) -> Self {
        let mut patterns = Vec::new();
        for secret in secrets {
            let text = secret.0.as_str();
            if text.is_empty() {
                continue;
            }
            let mut push = |text: Zeroizing<String>| patterns.push(text);
            push(Zeroizing::new(text.to_owned()));
            for engine in [
                general_purpose::STANDARD,
                general_purpose::STANDARD_NO_PAD,
                general_purpose::URL_SAFE,
                general_purpose::URL_SAFE_NO_PAD,
            ] {
                push(Zeroizing::new(engine.encode(text)));
            }
            let json = Zeroizing::new(serde_json::to_string(text).unwrap_or_default());
            if json.len() >= 2 {
                push(Zeroizing::new(json[1..json.len() - 1].to_owned()));
            }
            for upper in [false, true] {
                for encode_all in [false, true] {
                    let mut encoded = Zeroizing::new(String::new());
                    for byte in text.bytes() {
                        if !encode_all && (byte.is_ascii_alphanumeric() || b"-._~".contains(&byte))
                        {
                            encoded.push(char::from(byte));
                        } else {
                            use fmt::Write as _;
                            if upper {
                                let _ = write!(encoded, "%{byte:02X}");
                            } else {
                                let _ = write!(encoded, "%{byte:02x}");
                            }
                        }
                    }
                    push(encoded);
                }
            }
            push(Zeroizing::new(
                url::form_urlencoded::byte_serialize(text.as_bytes()).collect(),
            ));
        }
        patterns.sort_unstable_by_key(|pattern| std::cmp::Reverse(pattern.len()));
        patterns.dedup();
        patterns.retain(|pattern| !pattern.is_empty());
        Self {
            patterns,
            token_boundaries: false,
        }
    }

    /// Cookie preferences and boolean flags are public state, not credentials. Longer
    /// cookie values are masked only as complete tokens, unlike explicit Vault secrets.
    pub fn for_cookie(value: &str) -> Option<Self> {
        if value.len() < 12 {
            return None;
        }
        let secret = Secret::new(value.to_owned());
        let mut filter = Self::new([&secret]);
        filter.token_boundaries = true;
        Some(filter)
    }

    /// End of the longest pattern matching at `at`.
    fn match_end(&self, input: &str, at: usize) -> Option<usize> {
        self.patterns
            .iter()
            .filter(|pattern| input[at..].starts_with(pattern.as_str()))
            .filter_map(|pattern| {
                let end = at + pattern.len();
                let token = |byte: u8| byte.is_ascii_alphanumeric() || b"_-.+/%".contains(&byte);
                (!self.token_boundaries
                    || (at == 0 || !token(input.as_bytes()[at - 1]))
                        && (end == input.len() || !token(input.as_bytes()[end])))
                .then_some(end)
            })
            .max()
    }

    /// Return text with matching bytes replaced by `[REDACTED]`.
    ///
    /// Matching is performed against the original input, so a replacement cannot
    /// accidentally be interpreted as another secret or reveal overlapping values.
    pub fn redact(&self, input: &str) -> String {
        let mut output = String::with_capacity(input.len());
        let mut cursor = 0;
        while cursor < input.len() {
            if let Some(mut end) = self.match_end(input, cursor) {
                // Merge overlapping matches rather than leaking the uncovered suffix.
                let mut scan = cursor + 1;
                while scan < end {
                    if input.is_char_boundary(scan) {
                        if let Some(further) = self.match_end(input, scan) {
                            end = end.max(further);
                        }
                    }
                    scan += 1;
                }
                output.push_str("[REDACTED]");
                cursor = end;
            } else if let Some(ch) = input[cursor..].chars().next() {
                output.push(ch);
                cursor += ch.len_utf8();
            }
        }
        output
    }

    /// Whether one of `installed` already masks everything this filter masks, so installing
    /// this one too (e.g. on every unlock of an unchanged vault) would only keep another
    /// plaintext copy. A single filter must hold every pattern: overlapping matches merge only
    /// within one filter.
    pub fn covered_by(&self, installed: &[Redactor]) -> bool {
        installed.iter().any(|other| {
            if !self.token_boundaries && other.token_boundaries {
                return false;
            }
            self.patterns
                .iter()
                .all(|pattern| other.patterns.contains(pattern))
        })
    }
}

/// RFC 6238 HMAC digest selection. Serialized as the `otpauth://` `algorithm` names.
#[derive(Clone, Copy, Debug, Eq, PartialEq, Serialize, Deserialize)]
pub enum TotpAlgorithm {
    /// HMAC-SHA-1.
    #[serde(rename = "SHA1")]
    Sha1,
    /// HMAC-SHA-256.
    #[serde(rename = "SHA256")]
    Sha256,
    /// HMAC-SHA-512.
    #[serde(rename = "SHA512")]
    Sha512,
}

impl TotpAlgorithm {
    /// Parse an `otpauth://` `algorithm` value (`SHA1`, `SHA256`, `SHA512`, any case).
    pub fn parse(name: &str) -> Result<Self, Error> {
        match name.to_ascii_uppercase().as_str() {
            "SHA1" => Ok(Self::Sha1),
            "SHA256" => Ok(Self::Sha256),
            "SHA512" => Ok(Self::Sha512),
            _ => Err(Error::InvalidSeed),
        }
    }
}

/// Non-secret RFC 6238 parameters. [`Default`] is what authenticator apps assume when an
/// `otpauth://` URI omits them, and what vault items written before these parameters existed
/// are read as: 6 digits, a 30-second step, HMAC-SHA-1.
#[derive(Clone, Copy, Debug, Eq, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct TotpParams {
    digits: u8,
    period: u32,
    algorithm: TotpAlgorithm,
}

impl Default for TotpParams {
    fn default() -> Self {
        Self {
            digits: 6,
            period: 30,
            algorithm: TotpAlgorithm::Sha1,
        }
    }
}

impl TotpParams {
    /// Validate explicit parameters: `digits` 6, 7 or 8 and a `period` of 1..=86400 seconds.
    pub fn new(digits: u8, period: u32, algorithm: TotpAlgorithm) -> Result<Self, Error> {
        if !(6..=8).contains(&digits) || !(1..=86_400).contains(&period) {
            return Err(Error::InvalidSeed);
        }
        Ok(Self {
            digits,
            period,
            algorithm,
        })
    }

    /// Re-validate parameters read from storage or another device.
    pub(crate) fn validated(self) -> Result<Self, Error> {
        Self::new(self.digits, self.period, self.algorithm)
    }

    /// Number of decimal digits in a code.
    pub fn digits(&self) -> u8 {
        self.digits
    }

    /// Time step in seconds.
    pub fn period(&self) -> u32 {
        self.period
    }

    /// HMAC digest.
    pub fn algorithm(&self) -> TotpAlgorithm {
        self.algorithm
    }

    pub(crate) fn is_default(&self) -> bool {
        *self == Self::default()
    }
}

/// A TOTP factor: base32 seed (secret) plus its non-secret parameters.
pub struct Totp {
    pub(crate) seed: Secret,
    params: TotpParams,
}

impl fmt::Debug for Totp {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str("[REDACTED]")
    }
}

impl Totp {
    /// A base32 seed with explicit parameters.
    pub fn new(seed: Secret, params: TotpParams) -> Result<Self, Error> {
        parse_base32_seed(&seed)?;
        Ok(Self {
            seed,
            params: params.validated()?,
        })
    }

    /// Accept what a user pastes from a site's 2FA setup: an `otpauth://totp/...` URI, whose
    /// `secret`, `digits`, `period` and `algorithm` are used (missing ones default as in
    /// [`TotpParams::default`]; label, `issuer` and other parameters are ignored), or a bare
    /// base32 seed with default parameters. Only the seed is kept as a secret.
    pub fn parse(text: Secret) -> Result<Self, Error> {
        const SCHEME: &str = "otpauth://";
        let raw = text.0.as_str();
        if !raw
            .get(..SCHEME.len())
            .is_some_and(|scheme| scheme.eq_ignore_ascii_case(SCHEME))
        {
            return Self::new(text, TotpParams::default());
        }
        let rest = &raw[SCHEME.len()..];
        let (kind, rest) = rest.split_once('/').ok_or(Error::InvalidSeed)?;
        if !kind.eq_ignore_ascii_case("totp") {
            return Err(Error::InvalidSeed);
        }
        let query = rest
            .split_once('?')
            .map(|(_, query)| query)
            .ok_or(Error::InvalidSeed)?;
        let query = query.split_once('#').map_or(query, |(query, _)| query);
        let mut seed = None;
        let (mut digits, mut period, mut algorithm) = (None, None, None);
        for (key, value) in url::form_urlencoded::parse(query.as_bytes()) {
            let value = Zeroizing::new(value.into_owned());
            let slot_taken = match key.as_ref() {
                "secret" => seed.replace(Secret::new(value.to_string())).is_some(),
                "digits" => digits
                    .replace(value.parse::<u8>().map_err(|_| Error::InvalidSeed)?)
                    .is_some(),
                "period" => period
                    .replace(value.parse::<u32>().map_err(|_| Error::InvalidSeed)?)
                    .is_some(),
                "algorithm" => algorithm.replace(TotpAlgorithm::parse(&value)?).is_some(),
                _ => false,
            };
            if slot_taken {
                return Err(Error::InvalidSeed);
            }
        }
        let defaults = TotpParams::default();
        Self::new(
            seed.ok_or(Error::InvalidSeed)?,
            TotpParams::new(
                digits.unwrap_or(defaults.digits),
                period.unwrap_or(defaults.period),
                algorithm.unwrap_or(defaults.algorithm),
            )?,
        )
    }

    /// Non-secret parameters.
    pub fn params(&self) -> TotpParams {
        self.params
    }

    /// The code for Unix time `unix_seconds` (T0 = 0), zeroized on drop.
    pub(crate) fn code(&self, unix_seconds: u64) -> Result<Secret, Error> {
        parse_base32_seed(&self.seed)?.code(self.params, unix_seconds)
    }

    /// Redaction filter for the seed as setup pages and apps display it: as stored, upper and
    /// lower case, unpadded, and in groups of four separated by spaces.
    pub fn redactor(&self) -> Redactor {
        let stored = self.seed.0.as_str();
        let bare = Zeroizing::new(stored.trim_end_matches('=').to_owned());
        let mut forms = vec![Secret::new(stored.to_owned())];
        for text in [bare.to_ascii_uppercase(), bare.to_ascii_lowercase()] {
            let text = Zeroizing::new(text);
            let mut grouped = Zeroizing::new(String::with_capacity(text.len() * 5 / 4 + 1));
            for (index, ch) in text.chars().enumerate() {
                if index > 0 && index % 4 == 0 {
                    grouped.push(' ');
                }
                grouped.push(ch);
            }
            forms.push(Secret::new(text.to_string()));
            forms.push(Secret::new(grouped.to_string()));
        }
        Redactor::new(&forms)
    }

    /// Whether `other` is the same factor (same seed bytes and parameters), however either
    /// seed was written. Lets a caller tell one key shown twice from two different keys
    /// without ever seeing a seed.
    pub fn same_key(&self, other: &Self) -> bool {
        match (
            parse_base32_seed(&self.seed),
            parse_base32_seed(&other.seed),
        ) {
            (Ok(mine), Ok(theirs)) => self.params == other.params && *mine.0 == *theirs.0,
            _ => false,
        }
    }
}

/// Parse a base32 seed (case-insensitive, optional correct RFC 4648 padding).
/// The returned seed remains redacted and zeroizing.
pub fn parse_base32_seed(seed: &Secret) -> Result<TotpSeed, Error> {
    let upper = Zeroizing::new(seed.0.to_ascii_uppercase());
    let bytes = if upper.contains('=') {
        data_encoding::BASE32.decode(upper.as_bytes())
    } else {
        data_encoding::BASE32_NOPAD.decode(upper.as_bytes())
    }
    .map_err(|_| Error::InvalidSeed)?;
    if bytes.is_empty() {
        return Err(Error::InvalidSeed);
    }
    Ok(TotpSeed(Zeroizing::new(bytes)))
}

/// Zeroizing decoded TOTP seed; only codes, not seed bytes, can be requested.
pub struct TotpSeed(Zeroizing<Vec<u8>>);

impl fmt::Debug for TotpSeed {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str("[REDACTED]")
    }
}

impl TotpSeed {
    /// Generate the RFC 6238 code at Unix time `unix_seconds` with T0 = 0, zero-padded to
    /// `params.digits()`.
    pub fn code(&self, params: TotpParams, unix_seconds: u64) -> Result<Secret, Error> {
        let params = params.validated()?;
        let counter = (unix_seconds / u64::from(params.period)).to_be_bytes();
        macro_rules! digest {
            ($hash:ty) => {{
                let mut mac =
                    Hmac::<$hash>::new_from_slice(&self.0).map_err(|_| Error::InvalidSeed)?;
                mac.update(&counter);
                Zeroizing::new(mac.finalize().into_bytes().to_vec())
            }};
        }
        let hash = match params.algorithm {
            TotpAlgorithm::Sha1 => digest!(Sha1),
            TotpAlgorithm::Sha256 => digest!(Sha256),
            TotpAlgorithm::Sha512 => digest!(Sha512),
        };
        let offset = usize::from(hash[hash.len() - 1] & 0x0f);
        let binary = u32::from_be_bytes([
            hash[offset] & 0x7f,
            hash[offset + 1],
            hash[offset + 2],
            hash[offset + 3],
        ]);
        let digits = usize::from(params.digits);
        Ok(Secret::new(format!(
            "{:0digits$}",
            binary % 10u32.pow(u32::from(params.digits))
        )))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn cookie_filters_ignore_preferences_and_match_complete_credentials() {
        for value in ["no", "1", "true", "light", "Asia/Seoul"] {
            assert!(Redactor::for_cookie(value).is_none());
        }
        let value = "session-cookie-canary-58f29d04a1";
        let cookie = Redactor::for_cookie(value).unwrap();
        assert_eq!(
            cookie.redact(&format!("session={value};")),
            "session=[REDACTED];"
        );
        assert_eq!(
            cookie.redact(&format!("prefix{value}suffix")),
            format!("prefix{value}suffix")
        );
        let secret = Secret::new(value.into());
        let vault = Redactor::new([&secret]);
        assert_eq!(
            vault.redact(&format!("prefix{value}suffix")),
            "prefix[REDACTED]suffix"
        );
        assert!(!vault.covered_by(&[cookie]));
        let cookie = Redactor::for_cookie(value).unwrap();
        assert!(cookie.covered_by(&[vault]));
    }

    const SEEDS: [&str; 3] = [
        "12345678901234567890",
        "12345678901234567890123456789012",
        "1234567890123456789012345678901234567890123456789012345678901234",
    ];
    const ALGORITHMS: [TotpAlgorithm; 3] = [
        TotpAlgorithm::Sha1,
        TotpAlgorithm::Sha256,
        TotpAlgorithm::Sha512,
    ];
    /// RFC 6238 Appendix B: 8 digits, 30-second step, one row per T, SHA-1/256/512 columns.
    const APPENDIX_B: [(u64, [&str; 3]); 6] = [
        (59, ["94287082", "46119246", "90693936"]),
        (1_111_111_109, ["07081804", "68084774", "25091201"]),
        (1_111_111_111, ["14050471", "67062674", "99943326"]),
        (1_234_567_890, ["89005924", "91819424", "93441116"]),
        (2_000_000_000, ["69279037", "90698825", "38618901"]),
        (20_000_000_000, ["65353130", "77737706", "47863826"]),
    ];

    fn totp(i: usize, digits: u8, period: u32) -> Totp {
        Totp::new(
            Secret::new(data_encoding::BASE32_NOPAD.encode(SEEDS[i].as_bytes())),
            TotpParams::new(digits, period, ALGORITHMS[i]).unwrap(),
        )
        .unwrap()
    }

    #[test]
    fn rfc6238_appendix_b_all_vectors() {
        for (time, expected) in APPENDIX_B {
            for (i, expected) in expected.iter().enumerate() {
                assert_eq!(totp(i, 8, 30).code(time).unwrap().0.as_str(), *expected);
            }
        }
    }

    /// Fewer digits are the low-order digits of the same truncated value (RFC 4226 5.3).
    #[test]
    fn six_and_seven_digit_codes_are_the_low_digits_of_appendix_b() {
        for (time, expected) in APPENDIX_B {
            for (i, expected) in expected.iter().enumerate() {
                assert_eq!(
                    totp(i, 6, 30).code(time).unwrap().0.as_str(),
                    &expected[2..]
                );
                assert_eq!(
                    totp(i, 7, 30).code(time).unwrap().0.as_str(),
                    &expected[1..]
                );
            }
        }
        // Leading zeros are kept: T=1111111109 SHA-1 is 07081804, six digits 081804.
        assert_eq!(
            totp(0, 6, 30).code(1_111_111_109).unwrap().0.as_str(),
            "081804"
        );
    }

    #[test]
    fn period_scales_the_counter() {
        // With a 60-second step, T=118 is counter 1, the same as T=59 with 30 seconds.
        assert_eq!(totp(0, 8, 60).code(118).unwrap().0.as_str(), "94287082");
        assert_eq!(
            totp(0, 6, 60).code(59).unwrap().0.as_str(),
            totp(0, 6, 30).code(29).unwrap().0.as_str()
        );
    }

    #[test]
    fn otpauth_uri_and_bare_seed_parsing() {
        let seed = data_encoding::BASE32_NOPAD.encode(SEEDS[1].as_bytes());
        let uri = format!(
            "otpauth://totp/Example:alice%40example.com?secret={seed}&issuer=Example&algorithm=sha256&digits=8&period=30"
        );
        let parsed = Totp::parse(Secret::new(uri)).unwrap();
        assert_eq!(
            parsed.params(),
            TotpParams::new(8, 30, TotpAlgorithm::Sha256).unwrap()
        );
        assert_eq!(parsed.code(59).unwrap().0.as_str(), "46119246");
        assert_eq!(parsed.seed.0.as_str(), seed);

        let defaults = Totp::parse(Secret::new(format!(
            "OTPAUTH://TOTP/x?secret={}",
            seed.to_lowercase()
        )))
        .unwrap();
        assert_eq!(defaults.params(), TotpParams::default());
        let bare = Totp::parse(Secret::new(seed.clone())).unwrap();
        assert_eq!(bare.params(), TotpParams::default());

        for bad in [
            format!("otpauth://hotp/x?secret={seed}&counter=1"),
            "otpauth://totp/x?issuer=Example".to_owned(),
            format!("otpauth://totp/x?secret={seed}&digits=5"),
            format!("otpauth://totp/x?secret={seed}&digits=9"),
            format!("otpauth://totp/x?secret={seed}&period=0"),
            format!("otpauth://totp/x?secret={seed}&algorithm=MD5"),
            format!("otpauth://totp/x?secret={seed}&secret={seed}"),
            format!("otpauth://totp/x?secret={seed}&digits=6&digits=8"),
            "otpauth://totp/x?secret=not-base32".to_owned(),
            "otpauth://totp".to_owned(),
        ] {
            assert!(Totp::parse(Secret::new(bad)).is_err());
        }
        assert!(TotpParams::new(6, 86_401, TotpAlgorithm::Sha1).is_err());
    }

    #[test]
    fn base32_validation() {
        assert_eq!(
            parse_base32_seed(&Secret::new("my======".into()))
                .unwrap()
                .0
                .as_slice(),
            b"f"
        );
        assert_eq!(
            parse_base32_seed(&Secret::new("my".into()))
                .unwrap()
                .0
                .as_slice(),
            b"f"
        );
        for value in ["", "!", "MZ", "MY=", "M Y", "MY======X"] {
            assert!(parse_base32_seed(&Secret::new(value.into())).is_err());
        }
    }

    #[test]
    fn redacts_encodings_and_overlaps() {
        let secret = Secret::new("a\" b/雪 long".into());
        let redactor = Redactor::new([&secret]);
        let json = serde_json::to_string(secret.0.as_str()).unwrap();
        let encoded = general_purpose::STANDARD.encode(secret.0.as_bytes());
        let url: String = url::form_urlencoded::byte_serialize(secret.0.as_bytes()).collect();
        for text in [secret.0.as_str(), &json[1..json.len() - 1], &encoded, &url] {
            assert_eq!(redactor.redact(text), "[REDACTED]");
        }
        let a = Secret::new("abcdefgh".into());
        let b = Secret::new("efghijklm".into());
        assert_eq!(
            Redactor::new([&a, &b]).redact("abcdefghijklm"),
            "[REDACTED]"
        );
        assert_eq!(
            Redactor::new([&Secret::new(String::new())]).redact("unchanged"),
            "unchanged"
        );
    }

    #[test]
    fn every_non_empty_secret_is_masked_as_a_substring_whatever_its_length() {
        for (secret, text, masked) in [
            ("4", "ab4c", "ab[REDACTED]c"),
            ("482", "id9ee482f", "id9ee[REDACTED]f"),
            ("482915", "t_482915", "t_[REDACTED]"),
            ("pw1", "title pw1x", "title [REDACTED]x"),
            ("2026", "2026-10-12T04:41:26Z", "[REDACTED]-10-12T04:41:26Z"),
        ] {
            let redactor = Redactor::new([&Secret::new(secret.into())]);
            assert_eq!(redactor.redact(text), masked, "{secret}");
        }
        let code = Redactor::new([&Secret::new("482915".into())]);
        let encoded = general_purpose::STANDARD.encode("482915");
        assert_eq!(code.redact(&format!("b64 {encoded}")), "b64 [REDACTED]");
    }

    #[test]
    fn totp_redactor_masks_the_seed_as_setup_pages_display_it() {
        let totp = Totp::parse(Secret::new(
            "otpauth://totp/Example:ann?secret=JBSWY3DPEHPK3PXP&issuer=Example".into(),
        ))
        .unwrap();
        let redactor = totp.redactor();
        for shown in [
            "JBSWY3DPEHPK3PXP",
            "jbswy3dpehpk3pxp",
            "JBSW Y3DP EHPK 3PXP",
            "key: jbsw y3dp ehpk 3pxp",
        ] {
            assert!(
                !redactor.redact(shown).to_ascii_uppercase().contains("Y3DP"),
                "{shown}"
            );
        }
    }
}
