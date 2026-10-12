<picture>
  <source media="(prefers-color-scheme: dark)" srcset="docs/assets/logo-dark.svg">
  <img src="docs/assets/logo.svg" alt="openside" width="200">
</picture>

# openside

[한국어](README.ko.md)

openside is a local-first, agent-native browser. A Rust daemon (`opensided`) owns sessions, policy, a secrets vault and a Chromium instance, and runs every tool itself. A Python sidecar (`agent/`, Pydantic AI) runs the model loop and proxies tool calls back to the daemon. You talk to it through the `openside` CLI, a web/PWA UI, an MV3 extension for stock Chromium, MCP, or chat bots. A patched Chromium build is optional.

openside is an independent project and is not affiliated with Aside or at-inc.

## Status

Pre-alpha. APIs, configuration and on-disk formats may change without notice.

- Linux: supported.
- macOS: compiles; running it has not been tested.
- Windows: compiles; the runtime is not supported.

See [docs/PLATFORMS.md](docs/PLATFORMS.md).

## Build and run

Requirements: Rust stable via [rustup](https://rustup.rs) (the repository pins the channel), a C compiler, [uv](https://docs.astral.sh/uv/) (it installs Python 3.13), Node.js 22+ with pnpm 9, and a Chromium-based browser. [bubblewrap](https://github.com/containers/bubblewrap) is optional; if present, the shell tool runs inside it.

```bash
cargo build --release -p opensided -p openside-cli
uv sync --project agent
pnpm -C web install && pnpm -C web build                # optional web UI
pnpm -C extension install && pnpm -C extension build    # optional extension (load extension/dist unpacked)
```

Set a model and a credential source in `$XDG_CONFIG_HOME/openside/config.toml`:

```toml
[models]
default = "anthropic:<model-name>"

[credentials.order]
anthropic = ["env"]   # reads ANTHROPIC_API_KEY
```

Then:

```bash
target/release/openside doctor                                         # check Python, uv, browser, sandbox, credentials
OPENSIDE_UI_DIR=web/dist target/release/opensided --listen 127.0.0.1:7777   # keep running
# in another terminal:
target/release/openside "summarise this README"
target/release/openside pair    # pairs the web UI at http://127.0.0.1:7777
```

openside uses a stock Chromium or Chrome by default. The patched browser in [browser/](browser/BUILD.md) is optional and needs a separate Chromium source checkout.

More: [docs/INSTALL.md](docs/INSTALL.md), [docs/CONFIGURATION.md](docs/CONFIGURATION.md), [docs/REMOTE.md](docs/REMOTE.md).

## Where you can use it

- Remote access: reach the daemon from a phone or another computer over Tailscale Serve, a Cloudflare Tunnel or your own relay; set it up with `openside remote setup` ([docs/REMOTE.md](docs/REMOTE.md)).
- MCP and delegation: `openside mcp` serves the browser and agent tools to other agents, and `openside run` hands a goal to the openside browser agent ([docs/specs/mcp.md](docs/specs/mcp.md)).
- First run: the web UI opens `/welcome` to detect installed agents, import browser profiles and set up the vault ([docs/specs/onboarding.md](docs/specs/onboarding.md)).
- Phone app: the web UI installs as a PWA, with a share target, shortcuts, a pending-ticket badge and an offline outbox.

## Security

The agent can browse, fill forms and run shell commands for you. Policy modes and per-action approvals limit this, but read [docs/specs/threat-model.md](docs/specs/threat-model.md) before you give it a workspace, stored secrets, or remote access. The daemon listens only on loopback; exposing it to other devices needs TLS in front of it ([docs/REMOTE.md](docs/REMOTE.md)). To report a vulnerability, see [SECURITY.md](SECURITY.md).

CLI/MCP connections over the relay are end-to-end encrypted with Noise and pin the daemon
key from the pairing link. A PWA loaded from a relay protects traffic against passive
observers, but trusts the relay operator to serve its code: modified scripts can capture
pairing codes, tokens and plaintext. Prefer a self-hosted relay, or load and connect the
PWA directly to your daemon/Tailscale origin instead of using the relay. Login values
are omitted from agent tool results, not hidden from page scripts the agent runs.
Confirm approvals, choice answers, authenticator enrollment and saving a login require
a paired person device; agent clients and the local socket may deny or cancel, not approve.
Issue agent pairing links on the daemon host with `openside pair --agent`, then redeem
them with `openside connect`. An ordinary phone/browser link keeps its person-device
approval authority even when a CLI redeems it; do not give that token to an agent.

## Documentation

- [docs/03-architecture.md](docs/03-architecture.md): components and data flow
- [docs/specs/](docs/specs): protocol, API and feature specifications
- [docs/reference/](docs/reference): generated CLI and HTTP API reference
- [CONTRIBUTING.md](CONTRIBUTING.md): checks to run before sending changes

Most design documents are written in Korean.

## License

Licensed under either of [Apache License, Version 2.0](LICENSE-APACHE) or [MIT license](LICENSE-MIT), at your option. Unless you explicitly state otherwise, any contribution you intentionally submit for inclusion in this work, as defined in the Apache-2.0 license, is dual licensed as above, without any additional terms or conditions.

Third-party notices are in [NOTICE](NOTICE). The web UI design system is adapted from [aside-relay](https://github.com/mym0404/aside-relay) and [aside-remote](https://github.com/esc5221/aside-remote) (MIT). The bundled fonts Wanted Sans and D2Coding are under the SIL Open Font License 1.1.
