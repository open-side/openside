//! Typed daemon storage names are also the documentation source of truth.
use std::path::{Path, PathBuf};

macro_rules! data_paths {
    ($( $variant:ident => ($relative:literal, $tree:literal, $purpose:literal) ),+ $(,)?) => {
        /// Daemon-owned paths relative to the configured data directory.
        #[derive(Clone, Copy, Debug, PartialEq, Eq)]
        pub enum DataPath { $( #[doc = $purpose] $variant ),+ }
        impl DataPath {
            /// Every registered data location, in documentation order.
            pub const ALL: &'static [Self] = &[$(Self::$variant),+];
            /// Literal storage name; descendants of directory entries are owned by that component.
            pub const fn relative(self) -> &'static str { match self { $(Self::$variant => $relative),+ } }
            /// Resolve against an explicitly selected root (never the user's environment).
            pub fn path(self, root: &Path) -> PathBuf { root.join(self.relative()) }
            fn tree(self) -> bool { match self { $(Self::$variant => $tree),+ } }
            fn purpose(self) -> &'static str { match self { $(Self::$variant => $purpose),+ } }
        }
    }
}

data_paths! {
    Lock => ("daemon.lock", false, "Exclusive daemon lock"),
    Events => ("events.db", false, "Sessions, queue, events and sidecar snapshots; SQLite -wal/-shm/-journal companions"),
    Devices => ("devices.db", false, "Hashed capability tokens and device metadata; SQLite companions"),
    Memory => ("memory.db", false, "Local memory, history and search indexes; SQLite companions"),
    Routines => ("routines.db", false, "Schedules and run history; SQLite companions"),
    Agents => ("agents.db", false, "Team registry, handoffs, transfer progress and metadata-only audit; SQLite companions"),
    Policy => ("policy.json", false, "Live policy; .policy.json.<nonce>.tmp during atomic replacement"),
    Port => ("opensided.port", false, "Selected loopback TCP port"),
    Audit => ("audit.jsonl", false, "Redacted tool/security audit metadata"),
    VaultAudit => ("vault-audit.jsonl", false, "Vault-use metadata, never secret values"),
    Connections => ("connections.db", false, "Sign-in metadata, masked identities, keyed identity hashes and cookie keys only; SQLite companions"),
    ConnectionsAudit => ("connections-audit.jsonl", false, "Connection-use metadata; never cookie or header values"),
    ConnectionsKey => ("connections.key", false, "Private identity HMAC key; never exposed to agents"),
    RestartHandoff => ("restart-handoff.json", false, "Private one-use controlled-exec launcher secret; boot/request-bound, five-minute TTL; never exposed to agents"),
    Vault => ("vault.db", false, "Encrypted vault when vault.path is unset; created only by vault setup"),
    BrowserProfile => ("browser-profile", true, "Daemon-owned Chromium profile and caches; never the user's profile"),
    AccountSpaces => ("browser-profiles/accounts", true, "Opt-in persistent account profiles; launched on demand, never restored at daemon startup"),
    BrowserState => ("browser-state.json", false, "Private browser endpoint and tab ownership"),
    BrowserStateTemp => ("browser-state.tmp", false, "Browser-state atomic replacement"),
    SecretFillIntents => ("secret-fill-intents", true, "<ticket>.json durable at-most-once fill markers, without secret values"),
    Artifacts => ("artifacts", true, "<session>/transcript, generated files, screenshots, attachments/<message>/ files and atomic upload staging"),
    Skills => ("skills", true, "Skill directories, SKILL.md/resources and enablement metadata"),
    MemoryModel => ("memory", true, "model/ tokenizer.json and model.safetensors; CLI model-generation/staging directories"),
    Pins => ("pinned-tabs.json", false, "Pinned URLs and titles; atomic replacement companions"),
    Sync => ("sync", true, "state.json encrypted-share metadata and atomic replacements"),
    Vapid => ("push-vapid.key", false, "Private VAPID signing key"),
    Subscriptions => ("push-subscriptions.json", false, "Device-bound push subscriptions; atomic replacement companions"),
    CrashReports => ("crash-reports", true, "panic-<time>-<pid>.json metadata-only redacted reports; no upload"),
    Onboarding => ("onboarding.json", false, "First-run guide state (pending, completed or skipped); atomic replacement companions"),
    BrowserImports => ("browser-imports", true, "Private imported source metadata and durable native delivery revisions; pending cookie keys only"),
    ExtensionImportKeys => ("extension-import-keys", true, "Private per-pairing import HMAC authenticators; never exposed to the model or web pages"),
}

/// Reject undocumented locations observed in hermetic daemon lifecycle tests.
pub fn documented_data_path(relative: &Path) -> bool {
    if relative.as_os_str().is_empty() {
        return true;
    }
    DataPath::ALL.iter().any(|entry| {
        let name = entry.relative();
        if relative == Path::new(name) || entry.tree() && relative.starts_with(name) {
            return true;
        }
        let text = relative.to_string_lossy();
        if name.ends_with(".db")
            && text
                .strip_prefix(name)
                .is_some_and(|suffix| matches!(suffix, "-wal" | "-shm" | "-journal"))
        {
            return true;
        }
        !entry.tree()
            && relative.components().count() == 1
            && text
                .strip_prefix('.')
                .and_then(|value| value.strip_prefix(name))
                .is_some_and(|suffix| suffix.starts_with('.') && suffix.ends_with(".tmp"))
    })
}

/// Non-data roots also participate in the runtime documentation audit.
#[derive(Clone, Copy, Debug)]
pub enum StorageRoot {
    /// Durable records covered by the typed DataPath registry.
    Data,
    /// User-selected configuration, including atomic replacements.
    Config,
    /// Resume cursors retained independently of the process lifetime.
    State,
    /// Private IPC sockets and disposable profiles, not durable records.
    Runtime,
    /// openside keeps no XDG cache; the managed browser's disk cache lives in its profile.
    Cache,
}

impl StorageRoot {
    /// Match files actually observed below a root, without reading source text.
    pub fn documents(self, relative: &Path) -> bool {
        if relative.as_os_str().is_empty() {
            return true;
        }
        match self {
            Self::Data => documented_data_path(relative),
            Self::Config => {
                let text = relative.to_string_lossy();
                relative.components().count() == 1
                    && (text == "config.toml"
                        || text.starts_with(".config.toml.") && text.ends_with(".tmp"))
            }
            Self::State => relative.starts_with("cli"),
            Self::Runtime => {
                let first = relative
                    .components()
                    .next()
                    .map(|value| value.as_os_str().to_string_lossy())
                    .unwrap_or_default();
                relative == Path::new("opensided.sock")
                    || first.starts_with("sidecar-")
                    || relative.starts_with("incognito-browsers")
                    || relative.starts_with("browser")
            }
            Self::Cache => false,
        }
    }

    fn row(self) -> (&'static str, &'static str, &'static str) {
        match self {
            Self::Data => ("data_dir", "see typed DataPath entries", "Durable daemon storage"),
            Self::Config => ("config_dir", "`config.toml` and `.config.toml.<nonce>.tmp`", "Configuration and atomic replacement; OPENSIDE_CONFIG can relocate the file"),
            Self::State => ("state_dir", "`cli/**`", "CLI resume cursors; the daemon itself writes no state files"),
            Self::Runtime => ("runtime_dir", "`opensided.sock`, `sidecar-*/**`, `incognito-browsers/**`, `browser/**`", "Private daemon/sidecar/gateway sockets and disposable incognito profiles; explicit socket overrides may relocate the socket"),
            Self::Cache => ("cache", "none outside `browser-profile/**`", "No openside XDG cache directory; Chromium's disk cache stays in `browser-profile/cache` (`--disk-cache-dir`) and its crash database in `browser-profile/Crash Reports` (`--breakpad-dump-location`). The managed browser keeps the user's XDG directories so native-messaging hosts work, so its NSS store (`pki/nssdb`, shared with the user's other NSS apps), GSettings runtime (`dconf`) and GPU/font library caches are the user's own"),
        }
    }
}

/// Generated Markdown is checked against the repository's data-layout specification.
pub fn markdown() -> String {
    use std::fmt::Write as _;
    let mut text = String::from("# Daemon storage layout\n\nGenerated from `openside_config::data_layout::{DataPath, StorageRoot}`; regenerate with `cargo run -q -j 8 -p openside-config --example data-docs`.\n\nRoots follow `Paths::discover` and configured overrides; see `docs/CONFIGURATION.md`. The data root is private (0700 on Unix; sensitive files are 0600). No crash reports are uploaded.\n\n| Root | Relative path | Contents |\n|---|---|---|\n");
    for entry in DataPath::ALL {
        // String formatting with primitive fields cannot fail.
        let _ = writeln!(
            text,
            "| data_dir | `{}`{} | {} |",
            entry.relative(),
            if entry.tree() { "/**" } else { "" },
            entry.purpose()
        );
    }
    for root in [
        StorageRoot::Config,
        StorageRoot::State,
        StorageRoot::Runtime,
        StorageRoot::Cache,
    ] {
        let (name, paths, purpose) = root.row();
        let _ = writeln!(text, "| {name} | {paths} | {purpose} |");
    }
    text.push_str("\nConfigured `vault.path`, `skills.dir`, `memory.embedding.model_dir`, sync directory backends and session workspaces may live outside these roots; they are explicit user-owned destinations, not hidden daemon directories. SQLite companions and named atomic replacement files are included in the rows above. Directory rows cover all files delegated to the named component.\n");
    text.push_str("\nIsolated browser-space downloads stay under `browser-profile/space-downloads/<context>` and staged upload copies under `browser-profile/upload-staging/<tab>`, both in private 0700 directories. Disposing a space deletes its downloads; closing a tab or deleting its session deletes its upload copies. A fresh Chromium launch, verified browser death and clean shutdown sweep stale downloads and orphan upload copies; reconnecting to a surviving browser keeps those of live, bound spaces and tabs.\n");
    text
}

#[cfg(test)]
mod tests {
    #![allow(clippy::unwrap_used)]
    use super::*;
    #[test]
    fn every_registered_storage_path_is_documented_without_drift() {
        let path = Path::new(env!("CARGO_MANIFEST_DIR")).join("../../docs/specs/data-layout.md");
        assert_eq!(std::fs::read_to_string(path).unwrap(), markdown());
        for entry in DataPath::ALL {
            assert!(documented_data_path(Path::new(entry.relative())));
        }
        assert!(documented_data_path(Path::new("events.db-wal")));
        assert!(documented_data_path(Path::new(
            "artifacts/session/attachments/message/file.png"
        )));
        assert!(!documented_data_path(Path::new("undocumented.db")));
        assert!(!documented_data_path(Path::new("events.db/unknown")));
    }
}
