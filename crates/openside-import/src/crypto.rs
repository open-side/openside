//! Chromium cookie decryption (`os_crypt`): AES-128-CBC, IV of 16 spaces, PKCS#7, key =
//! PBKDF2-HMAC-SHA1(password, "saltysalt").
//!
//! Linux: `v10` uses the fixed password `peanuts`; `v11` uses the password stored in the
//! desktop keyring ("Chrome Safe Storage"), which the caller must supply. macOS derives the
//! key from the Keychain password with 1003 iterations for `v10`; verified natively on
//! x86_64 macOS 26. Windows (DPAPI + AES-GCM, `v10`/`v20`) is not supported.

#[cfg(unix)]
use std::io::Read;
#[cfg(unix)]
use std::os::unix::process::CommandExt;
#[cfg(unix)]
use std::process::{Command, Stdio};
#[cfg(unix)]
use std::sync::mpsc;
use std::time::Duration;
#[cfg(unix)]
use std::time::Instant;

use aes::cipher::{block_padding::Pkcs7, BlockDecryptMut, KeyIvInit};
use sha2::{Digest, Sha256};
use zeroize::{Zeroize, Zeroizing};

use crate::error::{ImportError, Result};

type Aes128CbcDec = cbc::Decryptor<aes::Aes128>;

const SALT: &[u8] = b"saltysalt";
const IV: [u8; 16] = [b' '; 16];
const LINUX_V10_PASSWORD: &[u8] = b"peanuts";
const COMMAND_TIMEOUT: Duration = Duration::from_secs(30);
/// Chromium cookie DB schema version from which the plaintext starts with SHA-256(host_key).
pub(crate) const DOMAIN_HASH_VERSION: i64 = 24;

#[cfg(target_os = "macos")]
pub(crate) const ITERATIONS: u32 = 1003;
#[cfg(not(target_os = "macos"))]
pub(crate) const ITERATIONS: u32 = 1;

/// Where the OS keyring password comes from.
#[derive(Clone, Default)]
pub enum KeyringSource {
    /// No password: `v11` cookies are an error.
    #[default]
    None,
    /// A password the caller already holds.
    Password(Zeroizing<String>),
    /// A command line, split like a shell would but run directly (no shell); its stdout,
    /// minus trailing newlines, is the password.
    Command(String),
}

impl KeyringSource {
    /// A password the caller already holds; it is wiped from memory on drop.
    pub fn password(password: impl Into<String>) -> Self {
        Self::Password(Zeroizing::new(password.into()))
    }
}

impl std::fmt::Debug for KeyringSource {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(match self {
            Self::None => "KeyringSource::None",
            Self::Password(_) => "KeyringSource::Password([REDACTED])",
            Self::Command(_) => "KeyringSource::Command(..)",
        })
    }
}

pub(crate) fn derive_key(password: &[u8], iterations: u32) -> [u8; 16] {
    let mut key = [0u8; 16];
    pbkdf2::pbkdf2_hmac::<sha1::Sha1>(password, SALT, iterations, &mut key);
    key
}

fn decrypt_raw(key: &[u8; 16], ciphertext: &[u8]) -> Option<Vec<u8>> {
    Aes128CbcDec::new(key.into(), &IV.into())
        .decrypt_padded_vec_mut::<Pkcs7>(ciphertext)
        .ok()
}

/// Run a keyring password command without a shell, bounded in time. The command runs in its
/// own process group so a timeout also ends helpers that inherited its stdout.
pub(crate) fn run_command(line: &str) -> Result<Zeroizing<String>> {
    run_command_within(line, COMMAND_TIMEOUT)
}

#[cfg(not(unix))]
fn run_command_within(_line: &str, _timeout: Duration) -> Result<Zeroizing<String>> {
    Err(ImportError::KeyringCommand(
        "Keyring command execution is unsupported on this platform".into(),
    ))
}

#[cfg(unix)]
fn run_command_within(line: &str, timeout: Duration) -> Result<Zeroizing<String>> {
    let argv = shlex::split(line)
        .filter(|argv| !argv.is_empty())
        .ok_or_else(|| {
            ImportError::KeyringCommand("the command line is empty or has unbalanced quotes".into())
        })?;
    let mut child = Command::new(&argv[0])
        .args(&argv[1..])
        .stdin(Stdio::null())
        .stdout(Stdio::piped())
        .stderr(Stdio::null())
        .process_group(0)
        .spawn()
        .map_err(|error| {
            ImportError::KeyringCommand(format!("cannot run `{}`: {error}", argv[0]))
        })?;
    let group = i32::try_from(child.id())
        .ok()
        .map(nix::unistd::Pid::from_raw);
    let timed_out = |child: &mut std::process::Child| {
        if let Some(group) = group {
            let _ = nix::sys::signal::killpg(group, nix::sys::signal::Signal::SIGKILL);
        }
        let _ = child.kill();
        let _ = child.wait();
        ImportError::KeyringCommand(format!(
            "`{}` did not finish within {} seconds",
            argv[0],
            timeout.as_secs().max(1)
        ))
    };
    let mut stdout = child
        .stdout
        .take()
        .ok_or_else(|| ImportError::KeyringCommand("command output is not available".into()))?;
    let (sender, receiver) = mpsc::channel();
    std::thread::spawn(move || {
        let mut bytes = Zeroizing::new(Vec::new());
        // The password is a short string; bound the read so a runaway command cannot fill memory.
        let _ = stdout.by_ref().take(4096).read_to_end(&mut bytes);
        let _ = sender.send(bytes);
    });
    let deadline = Instant::now() + timeout;
    let status = loop {
        match child.try_wait() {
            Ok(Some(status)) => break status,
            Ok(None) if Instant::now() < deadline => std::thread::sleep(Duration::from_millis(20)),
            Ok(None) => return Err(timed_out(&mut child)),
            Err(error) => {
                return Err(ImportError::KeyringCommand(format!(
                    "cannot wait for `{}`: {error}",
                    argv[0]
                )))
            }
        }
    };
    // A helper holding the pipe open keeps the reader blocked past the command's own exit.
    let Ok(bytes) = receiver.recv_timeout(deadline.saturating_duration_since(Instant::now()))
    else {
        return Err(timed_out(&mut child));
    };
    if !status.success() {
        return Err(ImportError::KeyringCommand(format!(
            "`{}` exited with {status}",
            argv[0]
        )));
    }
    let password =
        Zeroizing::new(String::from_utf8(bytes.to_vec()).map_err(|_| {
            ImportError::KeyringCommand("the command printed non-UTF-8 output".into())
        })?);
    let trimmed = password.trim_end_matches(['\n', '\r']);
    if trimmed.is_empty() {
        return Err(ImportError::KeyringCommand(format!(
            "`{}` printed no password",
            argv[0]
        )));
    }
    Ok(Zeroizing::new(trimmed.to_owned()))
}

/// Decrypts cookie blobs, deriving each key once and running the keyring command lazily.
pub(crate) struct Decryptor {
    keyring: KeyringSource,
    v10: Option<[u8; 16]>,
    keyring_key: Option<[u8; 16]>,
    // Set once the keyring key has decrypted a cookie, so a later failure is that cookie's
    // damage rather than a wrong password.
    keyring_verified: bool,
}

impl Drop for Decryptor {
    fn drop(&mut self) {
        self.v10.zeroize();
        self.keyring_key.zeroize();
    }
}

#[derive(Clone, Copy, PartialEq, Eq)]
enum Scheme {
    V10,
    V11,
}

impl Decryptor {
    pub(crate) fn new(keyring: KeyringSource) -> Self {
        Self {
            keyring,
            v10: None,
            keyring_key: None,
            keyring_verified: false,
        }
    }

    fn keyring_key(&mut self, host: &str, name: &str) -> Result<[u8; 16]> {
        if let Some(key) = self.keyring_key {
            return Ok(key);
        }
        let password = match &self.keyring {
            KeyringSource::None => {
                return Err(ImportError::KeyringPasswordRequired {
                    host: host.to_owned(),
                    name: name.to_owned(),
                })
            }
            KeyringSource::Password(password) => password.clone(),
            KeyringSource::Command(line) => run_command(line)?,
        };
        let key = derive_key(password.as_bytes(), ITERATIONS);
        self.keyring_key = Some(key);
        Ok(key)
    }

    fn key(&mut self, scheme: Scheme, host: &str, name: &str) -> Result<[u8; 16]> {
        match scheme {
            // macOS wraps v10 with the Keychain password; Linux uses the fixed password.
            Scheme::V10 if !cfg!(target_os = "macos") => Ok(*self
                .v10
                .get_or_insert_with(|| derive_key(LINUX_V10_PASSWORD, ITERATIONS))),
            Scheme::V10 | Scheme::V11 => self.keyring_key(host, name),
        }
    }

    /// Decrypt one `encrypted_value` blob. `schema_version` is the cookie DB's `meta.version`.
    pub(crate) fn decrypt(
        &mut self,
        host: &str,
        name: &str,
        blob: &[u8],
        schema_version: i64,
    ) -> Result<String> {
        let scheme = match blob.get(..3) {
            Some(b"v10") => Scheme::V10,
            Some(b"v11") => Scheme::V11,
            prefix => {
                let version: String = prefix
                    .unwrap_or(blob)
                    .iter()
                    .map(|byte| {
                        if byte.is_ascii_graphic() {
                            char::from(*byte)
                        } else {
                            '?'
                        }
                    })
                    .collect();
                return Err(ImportError::UnknownCookieVersion {
                    host: host.to_owned(),
                    name: name.to_owned(),
                    version,
                });
            }
        };
        let key = self.key(scheme, host, name)?;
        let keyring_derived = scheme == Scheme::V11 || cfg!(target_os = "macos");
        let fail = |wrong_password_possible: bool| {
            if keyring_derived && wrong_password_possible {
                ImportError::WrongKeyringPassword {
                    host: host.to_owned(),
                    name: name.to_owned(),
                }
            } else {
                ImportError::CorruptCookie {
                    host: host.to_owned(),
                    name: name.to_owned(),
                }
            }
        };
        let ciphertext = &blob[3..];
        // A wrong key cannot change the ciphertext length: a blob that is not whole AES blocks
        // is damaged no matter which password was used.
        if ciphertext.is_empty() || ciphertext.len() % 16 != 0 {
            return Err(fail(false));
        }
        let wrong = !self.keyring_verified;
        let plain = Zeroizing::new(decrypt_raw(&key, ciphertext).ok_or_else(|| fail(wrong))?);
        let value = if schema_version >= DOMAIN_HASH_VERSION {
            let (hash, value) = plain.split_at_checked(32).ok_or_else(|| fail(wrong))?;
            if hash != Sha256::digest(host.as_bytes()).as_slice() {
                return Err(fail(wrong));
            }
            value
        } else {
            &plain
        };
        let value = String::from_utf8(value.to_vec()).map_err(|_| fail(wrong))?;
        if keyring_derived {
            self.keyring_verified = true;
        }
        Ok(value)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[cfg(not(unix))]
    #[test]
    fn unsupported_keyring_command_never_runs_a_process() {
        let error = run_command("not-a-command").unwrap_err();
        assert!(matches!(
            error,
            ImportError::KeyringCommand(message)
                if message == "Keyring command execution is unsupported on this platform"
        ));
    }

    #[test]
    fn chromium_linux_peanuts_key_matches_the_published_value() {
        // Widely documented derived key for password "peanuts", 1 iteration, 16 bytes.
        let key = derive_key(b"peanuts", 1);
        assert_eq!(
            key,
            [
                0xfd, 0x62, 0x1f, 0xe5, 0xa2, 0xb4, 0x02, 0x53, 0x9d, 0xfa, 0x14, 0x7c, 0xa9, 0x27,
                0x27, 0x78
            ]
        );
    }

    #[cfg(unix)]
    #[test]
    fn keyring_command_is_run_without_a_shell_and_trimmed() {
        assert_eq!(*run_command("echo secret").unwrap(), "secret");
        // Shell metacharacters stay literal arguments instead of being interpreted.
        assert_eq!(*run_command("echo 'a;b' '$HOME'").unwrap(), "a;b $HOME");
    }

    #[cfg(unix)]
    #[test]
    fn keyring_command_failures_are_specific() {
        assert!(run_command("").unwrap_err().to_string().contains("empty"));
        assert!(run_command("false")
            .unwrap_err()
            .to_string()
            .contains("exited with"));
        assert!(run_command("openside-no-such-command-xyz")
            .unwrap_err()
            .to_string()
            .contains("cannot run"));
        assert!(run_command("true")
            .unwrap_err()
            .to_string()
            .contains("printed no password"));
    }

    #[cfg(unix)]
    #[test]
    fn a_command_whose_helper_keeps_stdout_open_times_out_and_the_group_is_killed() {
        let dir = tempfile::tempdir().unwrap();
        let pidfile = dir.path().join("helper.pid");
        let line = format!(
            "sh -c 'sleep 30 & echo $! > {}; echo password'",
            pidfile.display()
        );
        let started = Instant::now();
        let error = run_command_within(&line, Duration::from_secs(1)).unwrap_err();
        assert!(
            error.to_string().contains("did not finish within"),
            "{error}"
        );
        assert!(started.elapsed() < Duration::from_secs(10));
        let helper: i32 = std::fs::read_to_string(&pidfile)
            .unwrap()
            .trim()
            .parse()
            .unwrap();
        let gone = (0..100).any(|_| {
            std::thread::sleep(Duration::from_millis(20));
            nix::sys::signal::kill(nix::unistd::Pid::from_raw(helper), None)
                == Err(nix::errno::Errno::ESRCH)
        });
        assert!(gone, "the helper process outlived the timeout");
    }
}
