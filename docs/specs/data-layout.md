# Daemon storage layout

Generated from `openside_config::data_layout::{DataPath, StorageRoot}`; regenerate with `cargo run -q -j 8 -p openside-config --example data-docs`.

Roots follow `Paths::discover` and configured overrides; see `docs/CONFIGURATION.md`. The data root is private (0700 on Unix; sensitive files are 0600). No crash reports are uploaded.

| Root | Relative path | Contents |
|---|---|---|
| data_dir | `daemon.lock` | Exclusive daemon lock |
| data_dir | `events.db` | Sessions, queue, events and sidecar snapshots; SQLite -wal/-shm/-journal companions |
| data_dir | `devices.db` | Hashed capability tokens and device metadata; SQLite companions |
| data_dir | `memory.db` | Local memory, history and search indexes; SQLite companions |
| data_dir | `routines.db` | Schedules and run history; SQLite companions |
| data_dir | `policy.json` | Live policy; .policy.json.<nonce>.tmp during atomic replacement |
| data_dir | `opensided.port` | Selected loopback TCP port |
| data_dir | `audit.jsonl` | Redacted tool/security audit metadata |
| data_dir | `vault-audit.jsonl` | Vault-use metadata, never secret values |
| data_dir | `vault.db` | Encrypted vault when vault.path is unset; created only by vault setup |
| data_dir | `browser-profile`/** | Daemon-owned Chromium profile and caches; never the user's profile |
| data_dir | `browser-state.json` | Private browser endpoint and tab ownership |
| data_dir | `browser-state.tmp` | Browser-state atomic replacement |
| data_dir | `secret-fill-intents`/** | <ticket>.json durable at-most-once fill markers, without secret values |
| data_dir | `artifacts`/** | <session>/transcript, generated files, screenshots, attachments/<message>/ files and atomic upload staging |
| data_dir | `skills`/** | Skill directories, SKILL.md/resources and enablement metadata |
| data_dir | `memory`/** | model/ tokenizer.json and model.safetensors; CLI model-generation/staging directories |
| data_dir | `pinned-tabs.json` | Pinned URLs and titles; atomic replacement companions |
| data_dir | `sync`/** | state.json encrypted-share metadata and atomic replacements |
| data_dir | `push-vapid.key` | Private VAPID signing key |
| data_dir | `push-subscriptions.json` | Device-bound push subscriptions; atomic replacement companions |
| data_dir | `crash-reports`/** | panic-<time>-<pid>.json metadata-only redacted reports; no upload |
| config_dir | `config.toml` and `.config.toml.<nonce>.tmp` | Configuration and atomic replacement; OPENSIDE_CONFIG can relocate the file |
| state_dir | `cli/**` | CLI resume cursors; the daemon itself writes no state files |
| runtime_dir | `opensided.sock`, `sidecar-*/**`, `incognito-browsers/**`, `browser/**` | Private daemon/sidecar/gateway sockets and disposable incognito profiles; explicit socket overrides may relocate the socket |
| cache | none outside `browser-profile/**` | No openside XDG cache directory; Chromium's disk cache stays in `browser-profile/cache` (`--disk-cache-dir`) and its crash database in `browser-profile/Crash Reports` (`--breakpad-dump-location`). The managed browser keeps the user's XDG directories so native-messaging hosts work, so its NSS store (`pki/nssdb`, shared with the user's other NSS apps), GSettings runtime (`dconf`) and GPU/font library caches are the user's own |

Configured `vault.path`, `skills.dir`, `memory.embedding.model_dir`, sync directory backends and session workspaces may live outside these roots; they are explicit user-owned destinations, not hidden daemon directories. SQLite companions and named atomic replacement files are included in the rows above. Directory rows cover all files delegated to the named component.
