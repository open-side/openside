# 설치 (처음부터)

새 머신에서 openside 를 빌드하고 실행하기까지의 절차다. 지원 범위는 [PLATFORMS.md](PLATFORMS.md), 설정 키 전체는 [CONFIGURATION.md](CONFIGURATION.md) 를 본다. 이 문서의 명령은 Linux 기준이며 macOS 는 `unverified` 다.

구성 요소:

- `opensided`: Rust 데몬. 세션·정책·볼트·브라우저·티켓을 소유하고 모든 도구를 실행한다.
- `openside`: 비대화형 CLI 클라이언트(`crates/openside-cli`).
- `agent/`: omp `@oh-my-pi/pi-agent-core`/`@oh-my-pi/pi-ai` 기반의 Bun/TypeScript 사이드카(패키지 `openside-agent`). 데몬이 세션마다 하나씩 띄운다.

## 1. 필수 도구

### Rust

[rustup](https://rustup.rs) 으로 설치한다. 저장소의 `rust-toolchain.toml` 이 `stable` 채널과 `rustfmt`, `clippy` 컴포넌트를 고정하므로, 저장소 안에서 처음 `cargo` 를 실행하면 rustup 이 해당 툴체인을 맞춘다. 최소 버전은 `Cargo.toml` 의 `rust-version = "1.80"` 이다.

```bash
rustup show            # 저장소 루트에서 실행해 툴체인 확인
```

### Bun 과 사이드카

[Bun](https://bun.sh) 1.4.4 이상(`agent/package.json`의 `engines.bun`)을 설치한 뒤, 저장소 루트에서 사이드카 의존성을 설치한다. `agent/bun.lock`이 `@oh-my-pi/*` 패키지를 18.16.0으로 고정하며, 설치할 때 현재 플랫폼용 네이티브 애드온인 `@oh-my-pi/pi-natives-<os>-<arch>`(linux-x64 기준 약 245 MB)도 함께 받는다.

```bash
cd agent && bun install --frozen-lockfile
```

데몬은 사이드카를 `bun run <agent.dir>/src/main.ts`로 띄운다. 설정 `agent.bun`이나 환경변수 `OPENSIDE_BUN`으로 실행 파일 경로를 지정하지 않으면 PATH의 `bun`을 쓴다(`crates/openside-config/src/discovery.rs`).

`agent.dir`을 지정하지 않으면 실행 파일 위치를 기준으로 `../share/openside/agent`, `../../agent` 순서로 `package.json`에 `"name": "openside-agent"`가 들어 있는 디렉터리를 찾는다. 저장소에서 빌드한 바이너리(`target/<profile>/opensided`)는 두 번째 후보로 저장소의 `agent/`를 찾는다. 바이너리를 다른 곳으로 옮기면 `agent.dir`(또는 `OPENSIDE_AGENT_DIR`)이나 `agent.command`를 설정한다.

### 릴리스 배치

릴리스는 사이드카를 컴파일된 단일 바이너리로 배포하지 않는다. pi-agent-core를 불러올 때 pi-natives 네이티브 애드온을 로드하는데, `bun build --compile`은 `.node` 애드온을 바이너리에 내장할 수 없기 때문이다. 따라서 릴리스에는 소스 코드와 프로덕션 `node_modules`를 함께 담으며, 대상 머신에는 Bun 1.4.4 이상이 필요하다.

```
share/openside/
  agent/
    package.json
    bun.lock
    src/
    node_modules/                  # bun install --frozen-lockfile --production
  docs/specs/sidecar-wire.schema.json
```

`share/openside/agent` 안에서 `bun install --frozen-lockfile --production`을 실행해 `node_modules`를 만든다. `agent/src/protocol/validate.ts`가 `../../../docs/specs/sidecar-wire.schema.json` 경로에서 와이어 스키마를 불러오므로, 스키마 파일은 `agent/` 옆인 `share/openside/docs/specs/`에 두어야 한다. 설치 용량은 약 300 MB 늘어나며, 그중 대부분은 pi-natives 애드온(약 245 MB)이다.

### 바이너리 빌드

```bash
cargo build --release -p opensided -p openside-cli
# 결과: target/release/opensided, target/release/openside
```

## 2. 선택 도구

### Chromium 호환 브라우저

브라우저 도구(`browser_*`, `vault_login` 등)에 필요하다. OS 패키지(Chromium, Google Chrome) 또는 공식 빌드면 된다. 탐색 순서(`openside_config::discover_browser`, `crates/openside-config/src/discovery.rs`):

1. 설정 `browser.executable`
2. 환경변수 `OPENSIDE_BROWSER`
3. 설정 `browser.bundle_dir` 안의 `chrome`, 그다음 `chromium`
4. PATH 의 `chromium`, `chromium-browser`, `google-chrome`, `google-chrome-stable`, `chrome` 순서
5. 플랫폼 위치: macOS 는 `/Applications/Chromium.app/...`, `/Applications/Google Chrome.app/...`. Linux 는 추가 후보가 없다.

앞 단계에서 실행 가능한 파일을 찾으면 멈춘다. 찾지 못하면 `doctor` 의 `browser` 항목이 `warn` 이 되고, 시도한 후보 목록을 보여 준다. 데몬은 탐색만 하고, 첫 브라우저 도구 호출 때 데몬 소유의 격리 프로필로 한 개를 띄운다.

### bubblewrap (`bwrap`)

`bash` 도구의 샌드박스다. 있으면 `bash` 는 bwrap 안에서 workspace 만 읽기-쓰기, 시스템 디렉터리는 읽기 전용, 네트워크는 분리된 상태로 실행된다. 샌드박스 준비가 실패해도 비샌드박스로 되돌아가지 않는다.

- `sandbox.required = false`(기본): bwrap 이 없으면 `bash` 가 샌드박스 없이(정책 검사만 거쳐) 실행된다. doctor 는 `warn`.
- `sandbox.required = true`: bwrap 이 없으면 `bash` 도구를 숨긴다. doctor 는 `fail`.

Linux 배포판 패키지 이름은 보통 `bubblewrap` 이다.

## 3. 모델 자격 증명

UI의 Models & accounts에서 설정하거나 동등한 CLI 명령을 사용한다. API 키와 관리 대상 구독 OAuth 상태는 OS 키체인 기반 Vault에 암호화하여 저장하며, 기본 CLI 홈은 읽기 전용 상태로 유지된다. 자격 증명 확인과 갱신은 데몬이 전담하며, 사이드카에는 리프레시 토큰 대신 유효한 최신 접근 리스만 제공한다. 마이그레이션 시에는 기존 소스의 계정만 승계되며, 다른 소스를 추가하려면 사람이 명시적으로 프로바이더 풀을 구성해야 한다. [Models & accounts](specs/models-accounts.md)와 [Configuration](CONFIGURATION.md#credentials)을 참고한다.

소스 종류:

- `env`: API 키 환경변수. 기본 이름은 `ANTHROPIC_API_KEY`, `OPENAI_API_KEY`(`openai`, `openai-chat`, `openai-responses`), `OPENROUTER_API_KEY`, `GOOGLE_API_KEY`/`GEMINI_API_KEY`, `GITHUB_COPILOT_API_KEY`/`GITHUB_COPILOT_API_TOKEN`/`COPILOT_GITHUB_TOKEN`. 표에 없는 공급자는 `<PROVIDER>_API_KEY`(대문자, `-`→`_`). 데몬 프로세스 환경에 있어야 한다.
- `command`: `credentials.command.<provider>` 에 셸 없이 실행할 argv 를 둔다. stdout 이 키이며 기본 타임아웃은 10초다.
- `claude-code`/`codex-cli`: 기존 읽기 전용 기본 로그인을 사용하거나 `openside models accounts add <source> --label <label>`로 격리된 계정을 만든다. 화면에 안내되는 외부 명령을 실행하며 openside 자체는 별도의 OAuth 로그인 흐름을 두지 않는다.
- `vault`: 숨겨진 표준 입력을 통해 라벨이 지정된 API 키 계정을 등록한다(`openside key set <provider> --label <label>`). `builtin`은 데몬이 확인할 수 있는 프로바이더 환경변수 인증을 명시적으로 사용하며, 사이드카가 홈 디렉터리나 자격 증명 환경변수를 직접 검사하도록 허용하지는 않는다.
- `omp-broker`(선택): oh-my-pi auth broker 를 쓰는 사용자용이다. 토큰이 없으면 오류 없이 비활성화되므로 broker 가 없는 머신에서는 아무 설정도 필요 없다. `OPENSIDE_BROKER_URL`, `OPENSIDE_BROKER_TOKEN` 또는 `credentials.omp_broker.url`, `credentials.omp_broker.token_file` 로 지정한다.

예: Anthropic API 키만 쓰는 최소 설정.

```toml
[models]
default = "anthropic:<model-name>"

[credentials.order]
anthropic = ["env"]
```

## 4. 설정 파일과 기본 모델

설정 파일 위치는 `$XDG_CONFIG_HOME/openside/config.toml`(없으면 OS 설정 디렉터리) 이다. 파일이 없으면 기본값으로 동작한다. 주석이 달린 전체 예시는 `crates/openside-config/src/example.toml` 이다.

`models.default` 는 내장 기본값이 없다. 설정하지 않은 상태에서 `--model` 없이 세션을 만들면 API 가 `400 model_not_configured` 를 돌려준다(`crates/opensided/src/api.rs`). `OPENSIDE_MODEL` 환경변수로도 지정할 수 있다. 모델 식별자는 `provider:model` 형식이다.

## 5. 진단

```bash
target/release/openside doctor            # 사람이 읽는 출력
target/release/openside doctor --json     # 기계용 JSON
target/release/openside doctor --config ./config.toml
```

doctor는 데몬 없이 동작하며 디렉터리를 만들거나 자격 증명 값을 출력하지 않는다. 점검 항목: `config`, `paths.data|state|runtime`, `vault`(passphrase 명령은 실행하지 않음), `executable.bun`(`agent.bun`/`OPENSIDE_BUN` 또는 PATH에서 `bun`을 찾고 `bun --version`이 1.4.4 이상인지 확인하며, `agent.command`가 설정되어 있으면 건너뜀), `agent.discovery.*`, `agent.dependencies`(`node_modules/@oh-my-pi/pi-agent-core`와 현재 플랫폼의 `@oh-my-pi/pi-natives-<os>-<arch>`가 있는지 확인하며, 없으면 에이전트 디렉터리에서 `bun install --frozen-lockfile --production`을 실행), `agent.sidecar`/`agent.packages`(사이드카 `doctor --json`을 60초 제한으로 실행해 Bun과 패키지 버전을 보고), 프로바이더별 자격 증명, `models`, `browser`, `sandbox.bwrap`, `network.allow`. `fail` 항목이 하나라도 있으면 종료 코드는 1이다.

## 6. 실행

```bash
target/release/opensided                    # 포그라운드, SIGINT/SIGTERM 으로 종료
target/release/openside "README 를 요약해 줘"
target/release/openside --model anthropic:<model-name> --mode guard --workspace . "작업"
```

- `opensided` 플래그: `--config`, `--data-dir`, `--socket`, `--listen`.
- 기본 소켓은 `<runtime_dir>/opensided.sock`, 즉 보통 `$XDG_RUNTIME_DIR/openside/opensided.sock` 이다. 소켓 상위 디렉터리는 0700, 소켓은 0600 이어야 한다.
- CLI 도 `--socket` 이 없으면 같은 경로(`daemon.socket`/`OPENSIDE_SOCKET` 이 있으면 그 값, 없으면 `<runtime_dir>/opensided.sock`)에 연결한다. `XDG_RUNTIME_DIR` 이 없으면 데몬과 CLI 모두 `<임시 디렉터리>/openside-<uid>` 를 runtime 디렉터리로 쓴다. 이 디렉터리가 이미 있으면 소유자가 데몬을 실행하는 사용자이고 권한이 0700 이어야 하며, 다른 사용자가 먼저 만들어 둔 경우 데몬은 소유자 uid 를 알리는 오류로 시작을 거부한다.
- 선택적 TCP(`--listen`, `daemon.listen`, `OPENSIDE_LISTEN`) 는 loopback 주소만 허용하며, CLI 는 `--url` 과 `--token`(또는 `OPENSIDE_TOKEN`) 을 함께 요구한다.
- 같은 데이터 디렉터리에는 데몬 하나만 뜬다(`daemon.lock`).

### 티켓 (사람 단계)

에이전트가 확인·비밀 입력·기기 승인·정보 요청이 필요하면 데몬이 티켓을 연다. CLI 는 stderr 에 해결 명령을 안내하고, 대기 중인 티켓만 남은 세션은 상태 `waiting_human` 으로 정상 종료한다.

```bash
openside ticket list [--all]
openside ticket show <id>
openside ticket resolve <id> true              # confirm
openside ticket resolve <id> '"option-value"'  # info: JSON 이면 JSON, 아니면 문자열
openside ticket resolve <id>                   # secret_input: 비공개 프롬프트로 직접 입력
<secret-command> | openside ticket resolve <id> -   # secret_input: stdin 에서 읽음
openside ticket done <id>                      # device_approval, takeover
openside ticket reject <id> [--reason TEXT]
openside session resume <id>                   # 해결 후 이어서 보기
```

`-` 는 stdin 에서 값을 읽고 끝의 LF/CRLF 하나를 지우며, 값을 출력하지 않는다. 비밀을 argv 나 셸 히스토리에 남기지 않도록 파이프로 넘긴다. `secret_input` 은 데몬이 브라우저 입력란에 직접 넣으며, 등록된 SecretSink 가 없으면 HTTP 501 이다.

## 7. 서비스로 실행

데몬은 pending 사람 단계 티켓이 탭을 참조할 때만 종료 시 Chromium 을 남기고, 재시작 후 다시 붙는다. 그런 티켓이 없으면 종료할 때 Chromium 을 직접 닫고 회수한다(`crates/opensided/src/lib.rs`). 따라서 서비스 관리자는 메인 프로세스에만 신호를 보내고 자식 정리는 데몬에 맡겨야 한다.

### systemd user unit (Linux)

`~/.config/systemd/user/openside.service` (`openside service install` 가 같은 내용을 쓴다):

```ini
[Unit]
Description=openside daemon

[Service]
Type=simple
ExecStart=%h/.local/bin/opensided
# The daemon decides whether Chromium outlives it (pending browser tickets).
KillMode=process
Restart=on-failure
RestartSec=2
# User units get a minimal PATH; the sidecar needs bun (~/.bun/bin or ~/.local/bin).
Environment=PATH=%h/.bun/bin:%h/.local/bin:/usr/local/bin:/usr/bin:/bin

[Install]
WantedBy=default.target
```

```bash
install -m 0755 target/release/opensided target/release/openside ~/.local/bin/
systemctl --user daemon-reload
systemctl --user enable --now openside
journalctl --user -u openside -f
```

바이너리를 `~/.local/bin` 에 두면 사이드카 자동 탐색 후보는 `~/.local/share/openside/agent` 다. 저장소 checkout 을 그대로 쓰려면 설정에 `agent.dir` 을 저장소의 `agent` 디렉터리 절대 경로로 둔다. systemd user manager 는 `XDG_RUNTIME_DIR` 을 설정하므로 CLI 기본 소켓 경로와 일치한다. 이 유닛은 2026-10-12 qemu/KVM 가상머신(Arch Linux 클라우드 이미지)에서 실기 검증을 마쳤다. `sudo reboot` 및 sysrq 하드 리셋 후에도 linger(`loginctl enable-linger`) 설정 덕분에 systemd 사용자 관리자가 `openside.service`를 정상 기동했고, 중단되었던 세션은 `idle` 상태로 복구되어 새 메시지를 처리했다. 유닛에는 `Environment=PATH=%h/.bun/bin:%h/.local/bin:/usr/local/bin:/usr/bin:/bin`(없으면 systemd 사용자 관리자가 `bun`을 찾지 못함)과 `KillMode=process`가 필요하며, `openside service install`이 두 설정을 모두 기록한다. 가상머신 환경은 Arch Linux로만 시험했으며, 다른 리눅스 배포판이나 데스크톱 세션에서는 별도로 확인하지 않았다 [INFERENCE: 동일하게 정상 동작할 것으로 예상됨]. 위 가상머신 검증은 2026-10-12에 이전 Python 사이드카로 수행한 것이며, systemd 아래에서 Bun 사이드카는 다시 검증하지 않았다(`unverified`).

### launchd (macOS, `unverified`)

`openside service install --platform launchd` 가 `~/Library/LaunchAgents/dev.openside.daemon.plist` 를 쓴다(`openside service unit --platform launchd` 는 내용만 출력). 쓰는 내용:

```xml
<?xml version="1.0" encoding="UTF-8"?>
<!DOCTYPE plist PUBLIC "-//Apple//DTD PLIST 1.0//EN" "http://www.apple.com/DTDs/PropertyList-1.0.dtd">
<plist version="1.0">
<dict>
	<key>Label</key>
	<string>dev.openside.daemon</string>
	<key>ProgramArguments</key>
	<array>
		<string>/usr/local/bin/opensided</string>
	</array>
	<key>RunAtLoad</key>
	<true/>
	<key>KeepAlive</key>
	<dict>
		<key>SuccessfulExit</key>
		<false/>
	</dict>
	<!-- Stop only the daemon; it decides whether Chromium outlives it (pending browser tickets). -->
	<key>AbandonProcessGroup</key>
	<true/>
	<!-- Agents get a minimal PATH; the sidecar needs bun from Homebrew or /usr/local/bin. -->
	<key>EnvironmentVariables</key>
	<dict>
		<key>PATH</key>
		<string>/usr/local/bin:/opt/homebrew/bin:/usr/bin:/bin</string>
	</dict>
</dict>
</plist>
```

`ProgramArguments` 의 경로는 `openside` 옆의 `opensided`(없으면 `PATH`)이고 `--daemon` 으로 바꿀 수 있다. 소켓 경로는 plist 에 고정하지 않는다. launchd agent 와 로그인 셸이 같은 `$TMPDIR` 를 쓴다고 보면 데몬과 CLI 가 둘 다 `$TMPDIR/openside-<uid>/opensided.sock` 을 기본값으로 쓰므로 `--socket` 이 필요 없다 [INFERENCE: macOS 에서 확인하지 않음]. 다르게 쓰고 싶으면 `daemon.socket` 을 설정한다.

`AbandonProcessGroup` 은 systemd 의 `KillMode=process` 와 같은 의도다. 데몬은 Chromium 과 사이드카를 각자 별도 프로세스 그룹(`process_group(0)`)으로 띄우므로 이 키가 실제로 필요한지는 확인하지 않았고(`unverified`), 서비스 관리자는 메인 프로세스에만 신호를 보내야 한다는 위 원칙에 맞춰 렌더러가 항상 넣는다.

```bash
launchctl bootstrap gui/$(id -u) ~/Library/LaunchAgents/dev.openside.daemon.plist
openside doctor
```

Windows 에서는 `openside service` 가 systemd unit 대신 미지원 오류(`docs/PLATFORMS.md` 안내)를 낸다. `--platform` 을 직접 지정하면 다른 기기용 파일을 만들 수는 있다.

## 8. 개발 검증

전체 검증은 `tools/gate.sh`로 수행하며, 아래 명령어는 그중 핵심 단계만 모은 것이다.

```bash
cargo fmt --check
cargo clippy --all-targets -- -D warnings
cargo test
cd agent && bun install --frozen-lockfile && bun x biome check . && bun x tsc --noEmit -p . && bun test
```

- 브라우저 테스트는 브라우저를 찾지 못하면 `skip` 을 출력하고 건너뛴다.
- `openside-e2e`는 `bun`(PATH 또는 `OPENSIDE_BUN`)이나 `agent/node_modules`가 없으면 건너뛰므로, 먼저 `agent/`에서 `bun install --frozen-lockfile`을 실행한다.
- `tools/gate.sh`는 전체 게이트 검사를 한 번에 실행한다. 위에 적힌 명령 외에 웹 및 확장 프로그램 검사, 언어 검사, 라이선스 및 `NOTICE` 확인, 브라우저 UX 근거 점검, 이식성 검사를 함께 진행한다. 실행하려면 PATH 환경 변수에 `cargo-deny`가 있거나 `CARGO_DENY` 환경 변수를 지정해야 한다. 자세한 내용은 `docs/specs/quality-operations.md`를 참고한다.
- `tools/e2e-scenarios.sh`는 `docs/specs/e2e-scenarios.md`에 정의된 엔드투엔드 시나리오 스위트(실제 데몬, 스크립트 기반 모델, 로컬 테스트 사이트)를 실행하고 PASS/FAIL 표를 출력한다.
- `bench/run.sh`는 로컬 벤치마크 스위트를 명령 하나로 재현한다(`bench/README.md` 참고).
- `tools/soak.sh`는 압축된 소크(soak) 테스트를 실행한다(기본 15분). 24시간 실행 시: `OPENSIDE_SOAK_SECONDS=86400 OPENSIDE_SOAK_SESSIONS=100 tools/soak.sh`.
