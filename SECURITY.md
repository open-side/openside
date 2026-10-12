# Security policy

## Reporting a vulnerability

Please report vulnerabilities privately through GitHub: on [github.com/open-side/openside](https://github.com/open-side/openside), open **Security → Advisories → Report a vulnerability**. Do not open a public issue for a suspected vulnerability.

Include the affected component, the version or commit, steps to reproduce, and the impact you expect. openside is pre-alpha and maintained on a best-effort basis; there is no guaranteed response time.

## Scope

In scope: the daemon (`opensided`), the `openside` CLI, the Python sidecar (`agent/`), the web UI (`web/`), the MV3 extension (`extension/`) and the Chromium overlay and patches in `browser/`. Typical issues: policy or approval bypass, secret or vault value disclosure, sandbox escape from the shell tool, token or scope confusion in the HTTP API, pairing flaws, and cross-origin access to the daemon.

Out of scope: vulnerabilities in upstream Chromium, model providers, or third-party dependencies (report those upstream), and behaviour a user explicitly allowed through policy (for example, the `full` mode).

The threat model is in [docs/specs/threat-model.md](docs/specs/threat-model.md).
