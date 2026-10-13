use super::{
    storage::{self, Observed},
    Error,
};
use std::path::Path;
use unicode_normalization::UnicodeNormalization;
use zeroize::Zeroizing;

fn account() -> Result<String, Error> {
    nix::unistd::User::from_uid(nix::unistd::Uid::current())
        .map_err(|_| Error::Io)?
        .map(|u| u.name)
        .filter(|s| !s.is_empty())
        .ok_or(Error::Io)
}

fn service(home: &Path, managed: bool) -> Result<String, Error> {
    let raw = if managed {
        Some(home.to_str().ok_or(Error::Io)?.to_owned())
    } else {
        std::env::var("CLAUDE_SECURESTORAGE_CONFIG_DIR")
            .ok()
            .or_else(|| std::env::var("CLAUDE_CONFIG_DIR").ok())
    };
    let suffix = raw
        .filter(|s| !s.is_empty())
        .map(|raw| {
            let normalized: String = raw.nfc().collect();
            format!("-{}", &storage::digest(normalized.as_bytes())[..8])
        })
        .unwrap_or_default();
    Ok(format!("Claude Code-credentials{suffix}"))
}

pub(super) fn read(home: &Path, managed: bool) -> Result<Option<Observed>, Error> {
    let Some(bytes) = openside_vault::read_cli_keychain(&service(home, managed)?, &account()?)
        .map_err(|_| Error::Io)?
    else {
        return storage::read_file(&home.join(".credentials.json")).map(|observed| {
            observed.map(|mut observed| {
                observed.private = false;
                observed
            })
        });
    };
    if bytes.len() > 262_144 {
        return Err(Error::Io);
    }
    let text = std::str::from_utf8(&bytes).map_err(|_| Error::Io)?;
    // Keyring exposes no modification date; the exact-item content digest is the native revision fence.
    Ok(Some(Observed {
        bytes: Zeroizing::new(text.to_owned()),
        digest: storage::digest(&bytes),
        mtime: 0,
        private: true,
    }))
}

pub(super) fn write(home: &Path, raw: &str) -> Result<(), Error> {
    openside_vault::write_cli_keychain(&service(home, true)?, &account()?, raw.as_bytes())
        .map_err(|_| Error::Io)
}

pub(super) fn wipe(home: &Path) -> Result<(), Error> {
    openside_vault::delete_cli_keychain(&service(home, true)?, &account()?).map_err(|_| Error::Io)
}

#[cfg(test)]
mod tests {
    use super::*;
    struct ItemCleanup(std::path::PathBuf);
    impl Drop for ItemCleanup {
        fn drop(&mut self) {
            let _ = wipe(&self.0);
        }
    }
    #[test]
    fn namespace_matches_cli_raw_nfc_hash() {
        assert_eq!(
            service(Path::new("/abs/path"), true).unwrap(),
            "Claude Code-credentials-6d80187b"
        );
        assert_eq!(
            service(Path::new("/opt/cc"), true).unwrap(),
            "Claude Code-credentials-34fd9c6e"
        );
        assert_eq!(
            service(Path::new("/tmp/caf\u{e9}"), true).unwrap(),
            service(Path::new("/tmp/cafe\u{301}"), true).unwrap()
        );
    }
    #[test]
    #[ignore = "Requires an isolated macOS test user and an unlocked test Keychain"]
    fn isolated_fake_cli_item_roundtrip_and_wipe() {
        let dir = tempfile::tempdir().unwrap();
        let home = dir.path().canonicalize().unwrap().join("isolated-claude");
        storage::create(&home).unwrap();
        let _cleanup = ItemCleanup(home.clone());
        let output = std::process::Command::new(std::env::current_exe().unwrap())
            .args([
                "--exact",
                "managed_subscriptions::keychain::tests::fake_cli_keychain_writer",
                "--ignored",
            ])
            .env("OPENSIDE_FAKE_KEYCHAIN_CLI", "1")
            .env("CLAUDE_CONFIG_DIR", &home)
            .output()
            .unwrap();
        assert!(output.status.success());
        assert!(read(&home, true)
            .unwrap()
            .is_some_and(|o| o.bytes.as_str() == "fake-cli-keychain-canary"));
        write(&home, "fake-daemon-rotated").unwrap();
        assert!(read(&home, true)
            .unwrap()
            .is_some_and(|o| o.bytes.as_str() == "fake-daemon-rotated"));
        wipe(&home).unwrap();
        assert!(read(&home, true).unwrap().is_none());
    }

    #[test]
    #[ignore = "Only launched as a fake CLI by the isolated Keychain lifecycle test"]
    fn fake_cli_keychain_writer() {
        if std::env::var("OPENSIDE_FAKE_KEYCHAIN_CLI").as_deref() != Ok("1") {
            return;
        }
        let home = std::path::PathBuf::from(std::env::var_os("CLAUDE_CONFIG_DIR").unwrap());
        assert!(
            home.is_absolute()
                && home
                    .file_name()
                    .is_some_and(|name| name == "isolated-claude")
        );
        openside_vault::write_cli_keychain(
            &service(&home, true).unwrap(),
            &account().unwrap(),
            b"fake-cli-keychain-canary",
        )
        .unwrap();
    }
}
