use std::path::PathBuf;

/// Why an import stopped. Messages name the file, browser or cookie (host and name only)
/// at fault and never contain cookie values or the keyring password.
#[derive(Debug, thiserror::Error)]
pub enum ImportError {
    /// `--from` is not `<chrome|chromium|firefox>:<directory>`.
    #[error("invalid source `{0}`: expected <chrome|chromium|firefox>:<profile directory>")]
    InvalidSource(String),
    /// The openside profile name cannot be used as a directory name.
    #[error("invalid profile name `{0}`: use 1-64 characters from a-z, 0-9, `_` and `-`, starting with a letter or digit")]
    InvalidProfileName(String),
    /// A file the importer needs is not in the source profile.
    #[error("{browser} {what} not found at {path}: is this a {browser} profile directory?")]
    MissingFile {
        /// Browser family the source was declared as.
        browser: &'static str,
        /// Human name of the missing item.
        what: &'static str,
        /// Where it was expected.
        path: PathBuf,
    },
    /// The source file exists but cannot be read.
    #[error("cannot read {what} at {path}: {reason}")]
    Unreadable {
        /// Human name of the item.
        what: &'static str,
        /// File that failed.
        path: PathBuf,
        /// Operating-system error.
        reason: String,
    },
    /// A source file or its SQLite snapshot would exceed the import byte ceiling.
    #[error("{what} at {path} exceeds the import size limit ({size} bytes; limit {limit})")]
    SourceTooLarge {
        /// Human name of the item.
        what: &'static str,
        /// Source file.
        path: PathBuf,
        /// Observed file or SQLite snapshot size.
        size: u64,
        /// Maximum permitted size in bytes.
        limit: u64,
    },
    /// SQLite reported the source database as busy or locked.
    #[error("{what} database {path} is locked; close {browser} and import again")]
    Locked {
        /// Browser family the source was declared as.
        browser: &'static str,
        /// Human name of the item.
        what: &'static str,
        /// Source file.
        path: PathBuf,
    },
    /// The operating system denied access because another process holds the source.
    #[error("{what} source {path} is locked; close {browser} and retry")]
    SourceLocked {
        /// Browser family to close.
        browser: &'static str,
        /// Human name of the item.
        what: &'static str,
        /// Source file.
        path: PathBuf,
    },
    /// The file is not a usable SQLite database or is not valid JSON.
    #[error("{what} database {path} is corrupt or not a {format} file: {detail}")]
    Corrupt {
        /// Human name of the item.
        what: &'static str,
        /// Source file.
        path: PathBuf,
        /// Expected file format.
        format: &'static str,
        /// Parser or SQLite message.
        detail: String,
    },
    /// The file parses but its layout is not one this importer knows.
    #[error("unsupported {browser} {what} format in {path}: {detail}")]
    UnsupportedVersion {
        /// Browser family the source was declared as.
        browser: &'static str,
        /// Human name of the item.
        what: &'static str,
        /// Source file.
        path: PathBuf,
        /// What was missing or unexpected.
        detail: String,
    },
    /// A cookie carries an encryption scheme prefix the importer cannot decrypt.
    #[error("cookie `{name}` for {host} uses unknown encryption version `{version}`; only v10 and v11 are supported on this platform")]
    UnknownCookieVersion {
        /// Cookie host.
        host: String,
        /// Cookie name.
        name: String,
        /// The three-byte version prefix, printable ASCII only.
        version: String,
    },
    /// A v11 cookie (OS keyring key) was found without a keyring password.
    #[error("cookie `{name}` for {host} is encrypted with the OS keyring key (v11); pass --keyring-password-cmd with a command that prints the browser's `Safe Storage` password")]
    KeyringPasswordRequired {
        /// Cookie host.
        host: String,
        /// Cookie name.
        name: String,
    },
    /// The supplied keyring password does not decrypt the cookie.
    #[error("the keyring password is wrong: it does not decrypt cookie `{name}` for {host}")]
    WrongKeyringPassword {
        /// Cookie host.
        host: String,
        /// Cookie name.
        name: String,
    },
    /// A v10 cookie (fixed key) does not decrypt: the profile data is damaged.
    #[error("cookie `{name}` for {host} is corrupt: its encrypted value does not decrypt")]
    CorruptCookie {
        /// Cookie host.
        host: String,
        /// Cookie name.
        name: String,
    },
    /// `--keyring-password-cmd` could not produce a password.
    #[error("keyring password command failed: {0}")]
    KeyringCommand(String),
    /// The OS keyring holds the browser's Safe Storage key but is locked or refused access.
    #[error("the OS keyring is locked or denied access to the {browser} Safe Storage key; unlock it and try again")]
    KeyringLocked {
        /// Browser brand id.
        browser: String,
    },
    /// No Safe Storage key could be fetched from the OS keyring (no service, no entry, or an
    /// unsupported keyring such as KWallet's own API).
    #[error("the {browser} Safe Storage key is not available from the OS keyring: {reason}")]
    KeyringUnavailable {
        /// Browser brand id.
        browser: String,
        /// Why, in words that carry no secret.
        reason: &'static str,
    },
    /// A saved password uses an encryption scheme this platform cannot read (Windows `v20`
    /// app-bound encryption, DPAPI-only blobs, ...).
    #[error("a saved password for {origin} uses unsupported encryption `{version}`")]
    UnsupportedPasswordEncryption {
        /// Site the login belongs to.
        origin: String,
        /// The three-byte version prefix, printable ASCII only.
        version: String,
    },
    /// The Safe Storage key does not decrypt a saved password: a wrong keyring password.
    #[error("the keyring key does not decrypt the saved password for {origin}")]
    WrongPasswordKey {
        /// Site the login belongs to.
        origin: String,
    },
    /// A saved password blob is damaged even though the key decrypted other entries.
    #[error("the saved password for {origin} is corrupt: its encrypted value does not decrypt")]
    CorruptPassword {
        /// Site the login belongs to.
        origin: String,
    },
    /// Cookie import needs a Chromium-compatible executable to write the target profile.
    #[error(
        "importing cookies needs a Chromium-compatible browser to write the target profile: {0}"
    )]
    BrowserRequired(String),
    /// Another browser already runs on the target openside profile.
    #[error(
        "openside profile `{0}` is in use by a running browser; stop it before importing cookies"
    )]
    ProfileInUse(String),
    /// The target Chromium failed to launch, answer or shut down cleanly.
    #[error("target browser failed: {0}")]
    Browser(String),
    /// Writing the openside profile store failed.
    #[error("cannot write {path}: {reason}")]
    Write {
        /// File or directory.
        path: PathBuf,
        /// Operating-system error.
        reason: String,
    },
}

/// Result alias.
pub type Result<T, E = ImportError> = std::result::Result<T, E>;
