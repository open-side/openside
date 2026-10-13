use std::collections::HashSet;

use sha2::{Digest, Sha256};
use zeroize::{Zeroize, Zeroizing};

use crate::{ImportError, Result, SavedPassword};

/// Parsed credentials remain host-only; the original CSV is never retained as an upload.
pub struct CsvPasswords {
    /// Valid normalized sign-ins, deduped by origin and username.
    pub passwords: Vec<SavedPassword>,
    /// Source rows before filtering and deduplication.
    pub read: u64,
    /// Rows excluded as invalid, empty or duplicate.
    pub skipped: u64,
}

fn malformed() -> ImportError {
    ImportError::UnsupportedVersion {
        browser: "Browser",
        what: "password CSV",
        path: std::path::PathBuf::new(),
        detail: "The password CSV must include URL, username and password columns.".to_owned(),
    }
}

/// Parse browser-exported UTF-8 CSV without writing files or echoing malformed values.
pub fn parse_password_csv(input: &str) -> Result<CsvPasswords> {
    if input.len() > 2 * 1024 * 1024 {
        return Err(malformed());
    }
    let mut input = input.trim_start_matches('\u{feff}').as_bytes();
    let mut reader = csv_core::Reader::new();
    let mut output = Zeroizing::new(vec![0; input.len().max(1)]);
    let mut ends = vec![0; 32];
    let (mut written, mut fields) = (0, 0);
    let mut columns = None;
    let mut parsed = CsvPasswords {
        passwords: Vec::new(),
        read: 0,
        skipped: 0,
    };
    let mut identities = HashSet::new();
    loop {
        let (result, consumed, produced, completed) =
            reader.read_record(input, &mut output[written..], &mut ends[fields..]);
        input = &input[consumed..];
        written += produced;
        fields += completed;
        match result {
            csv_core::ReadRecordResult::InputEmpty => continue,
            csv_core::ReadRecordResult::OutputEndsFull => {
                ends.resize(ends.len().saturating_mul(2), 0);
                continue;
            }
            csv_core::ReadRecordResult::OutputFull => return Err(malformed()),
            csv_core::ReadRecordResult::End => break,
            csv_core::ReadRecordResult::Record => {}
        }
        let data = &output[..written];
        let offsets = &ends[..fields];
        if let Some((url, username, password)) = columns {
            parse_row(
                data,
                offsets,
                url,
                username,
                password,
                &mut parsed,
                &mut identities,
            );
        } else {
            let header = |names: &[&str]| {
                (0..fields).find(|index| {
                    names.iter().any(|name| {
                        field(data, offsets, *index)
                            .trim()
                            .eq_ignore_ascii_case(name)
                    })
                })
            };
            columns = Some((
                header(&["url", "website", "website address"]).ok_or_else(malformed)?,
                header(&["username", "user name", "login"]).ok_or_else(malformed)?,
                header(&["password"]).ok_or_else(malformed)?,
            ));
        }
        output[..written].zeroize();
        written = 0;
        fields = 0;
    }
    if columns.is_none() {
        return Err(malformed());
    }
    Ok(parsed)
}

fn field<'a>(data: &'a [u8], ends: &[usize], index: usize) -> &'a str {
    let start = if index == 0 {
        0
    } else {
        *ends.get(index - 1).unwrap_or(&data.len())
    };
    let end = *ends.get(index).unwrap_or(&start);
    data.get(start..end)
        .and_then(|bytes| std::str::from_utf8(bytes).ok())
        .unwrap_or_default()
}

fn parse_row(
    data: &[u8],
    ends: &[usize],
    url: usize,
    username: usize,
    password: usize,
    parsed: &mut CsvPasswords,
    identities: &mut HashSet<[u8; 32]>,
) {
    parsed.read = parsed.read.saturating_add(1);
    let address = field(data, ends, url).trim();
    let username = field(data, ends, username);
    let password = field(data, ends, password);
    if !crate::model::is_web_url(address)
        || password.is_empty()
        || address.contains('\0')
        || username.contains('\0')
        || password.contains('\0')
    {
        parsed.skipped = parsed.skipped.saturating_add(1);
        return;
    }
    let url = match url::Url::parse(address) {
        Ok(url)
            if url.username().is_empty()
                && url.password().is_none()
                && matches!(url.scheme(), "http" | "https") =>
        {
            url
        }
        _ => {
            parsed.skipped += 1;
            return;
        }
    };
    let origin = url.origin().ascii_serialization();
    let mut identity = Sha256::new();
    identity.update(origin.as_bytes());
    identity.update([0]);
    identity.update(username.as_bytes());
    if !identities.insert(<[u8; 32]>::from(identity.finalize())) {
        parsed.skipped += 1;
        return;
    }
    parsed.passwords.push(SavedPassword {
        origin,
        username: Zeroizing::new(username.to_owned()),
        password: Zeroizing::new(password.to_owned()),
    });
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn browser_exports_support_quoted_values_and_dedupe() {
        for input in [
            "name,url,username,password,note\r\nExample,https://example.test/path,person,\"a,b\",x\r\nExample,https://example.test/other,person,duplicate,x\r\n",
            "url,username,password,httpRealm,formActionOrigin,guid\nhttps://example.test/path,person,\"a,b\",,,\nhttps://example.test/other,person,duplicate,,,\n",
            "Title,URL,Username,Password,OTPAuth,Notes\nExample,https://example.test/path,person,\"a,b\",,\nExample,https://example.test/other,person,duplicate,,\n",
        ] {
            let parsed = parse_password_csv(input).unwrap();
            assert_eq!((parsed.read, parsed.skipped, parsed.passwords.len()), (2, 1, 1));
            assert_eq!(parsed.passwords[0].origin, "https://example.test");
            assert_eq!(parsed.passwords[0].password.as_str(), "a,b");
            assert!(!format!("{:?}", parsed.passwords).contains("a,b"));
        }
    }

    #[test]
    fn invalid_exports_fail_without_repeating_values() {
        let error = parse_password_csv("password\nCSV-secret-canary\n")
            .err()
            .unwrap();
        assert!(!error.to_string().contains("CSV-secret-canary"));
        let parsed = parse_password_csv("url,username,password\nhttps://person:secret@example.test/,person,password\nabout:blank,person,password\n").unwrap();
        assert_eq!(parsed.skipped, 2);
        assert!(parsed.passwords.is_empty());
    }

    #[test]
    fn reusable_zeroizing_csv_records_handle_bom_multiline_and_wide_exports() {
        let extras = (0..80)
            .map(|number| format!("extra{number}"))
            .collect::<Vec<_>>()
            .join(",");
        let empty = ",".repeat(80);
        let input = format!("\u{feff}URL,User Name,Password,{extras}\r\nhttps://example.test/login,person,\"first,\r\nsecond \"\"quoted\"\"\",{empty}\r\nhttps://other.test/,other,last,{empty}");
        let parsed = parse_password_csv(&input).unwrap();
        assert_eq!(
            (parsed.read, parsed.skipped, parsed.passwords.len()),
            (2, 0, 2)
        );
        assert_eq!(
            parsed.passwords[0].password.as_str(),
            "first,\r\nsecond \"quoted\""
        );
        assert_eq!(parsed.passwords[1].password.as_str(), "last");
        assert_eq!(parsed.passwords[1].username.as_str(), "other");
    }
}
