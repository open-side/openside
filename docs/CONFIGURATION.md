# 설정

openside 설정은 TOML 파일 하나다. 아래 표는 2026-10-11 에 `cargo run -q -p openside-config --example config-docs` 가 출력한 예시 TOML(`crates/openside-config/src/example.toml`)과 JSON Schema(`openside_config::config_schema`)로 만들었다. 키를 바꾸면 이 명령을 다시 실행해 표를 맞춘다.

설치 절차는 [INSTALL.md](INSTALL.md) 를 본다.

## 파일 위치와 경로

`crates/openside-config/src/paths.rs` 의 `Paths::discover` 가 정한다. 탐색은 디렉터리를 만들지 않는다.

| 경로 | 결정 순서 |
|---|---|
| 설정 파일 | `OPENSIDE_CONFIG` → `$XDG_CONFIG_HOME/openside/config.toml` → OS 설정 디렉터리(`dirs::config_dir`)`/openside/config.toml` → 임시 디렉터리 |
| data | `$XDG_DATA_HOME/openside` → `dirs::data_dir()/openside` → 임시 디렉터리`/openside`. `daemon.data_dir`/`OPENSIDE_DATA_DIR`/`opensided --data-dir` 가 덮어쓴다 |
| state | `$XDG_STATE_HOME/openside` → `dirs::state_dir()/openside` → data 디렉터리 |
| runtime | `$XDG_RUNTIME_DIR/openside` → `dirs::runtime_dir()/openside` → Unix 는 임시 디렉터리`/openside-<uid>`, 그 밖은 임시 디렉터리`/openside` |

- 데몬 소켓 기본값은 `<runtime>/opensided.sock` 이다(`crates/opensided/src/config.rs`). 상위 디렉터리는 0700 이어야 하며 없으면 0700 으로 만든다.
- data 디렉터리는 0700 으로 맞춰지고 `daemon.lock` 으로 데몬 하나만 허용한다.
- CLI resume 커서는 `$XDG_STATE_HOME/openside/cli`(또는 `$HOME/.local/state/openside/cli`) 아래 0600 파일이다(`crates/openside-cli/src/lib.rs`).
- macOS 에서 `dirs::state_dir`, `dirs::runtime_dir` 는 값이 없으므로 state 는 data 디렉터리, runtime 은 임시 디렉터리 아래로 간다(`dirs` crate 동작, 실행 확인은 `unverified`).
- openside가 data, config, runtime, cache 아래에 생성하는 모든 경로는 `docs/specs/data-layout.md`에 적혀 있다. 이 표는 타입 레지스트리(`crates/openside-config/src/data_layout.rs`의 `DataPath`, `StorageRoot`)에서 생성하며, 실제 데몬, 사이드카, Chromium을 구동하는 E2E 테스트는 문서에 없는 경로가 생기면 실패한다.
- 관리형 Chromium은 사용자의 `XDG_CONFIG_HOME`, `XDG_DATA_HOME`, `XDG_CACHE_HOME`, `XDG_RUNTIME_DIR`을 변경 없이 그대로 상속받는다. 비밀번호 관리자의 네이티브 메시징 호스트가 브라우저 환경에서 실행되어 데스크톱 앱을 탐색하고(Bitwarden `desktop_proxy`는 `XDG_CACHE_HOME`, KeePassXC는 `XDG_RUNTIME_DIR` 사용), 데스크톱 글꼴 및 IME 설정도 함께 적용하기 위함이다. Chromium 디스크 캐시는 `--disk-cache-dir=<profile>/cache`로 프로필 내부에 고정하며, 게이트웨이 디렉터리는 `--openside-gateway-runtime-dir=<runtime>/browser`로 별도 전달한다. Chromium 크래시 데이터베이스는 `BREAKPAD_DUMP_LOCATION` 환경 변수 대신 `--breakpad-dump-location` 스위치로 `<profile>/Crash Reports`에 고정한다. 환경 변수를 쓰면 native-messaging 호스트가 이 설정을 그대로 물려받기 때문이다.
- 이에 따라 NSS 인증서 저장소는 사용자의 것(`~/.pki/nssdb`가 있으면 해당 경로, 없으면 `$XDG_DATA_HOME/pki/nssdb`)을 사용하고, 웹 앱 바로가기는 `$XDG_DATA_HOME/applications`에 저장되며, GPU 및 글꼴 라이브러리 캐시는 사용자의 캐시 디렉터리를 쓴다. NSS 저장소는 사용자의 다른 NSS 애플리케이션과 함께 사용하며, 여기에 가져온 인증서와 클라이언트 키는 양쪽 모두에서 접근할 수 있다. GSettings는 런타임 상태를 `$XDG_RUNTIME_DIR/dconf`에 보관한다. 자체 프로필 디렉터리로 실행한 브라우저는 사용자 수준 매니페스트를 지정된 위치에서만 읽으므로, 네이티브 메시징 호스트 매니페스트는 `<data>/browser-profile/NativeMessagingHosts/` 아래에 배치해야 한다.
- 데몬이 패닉에 빠지면 data 디렉터리 내 `crash-reports/`에 메타데이터만 담긴 보고서를 기록한다. 원인 파일명과 줄 번호만 포함되며 패닉 메시지, 백트레이스, 환경 변수, 스레드 이름은 남기지 않는다(디렉터리 권한 0700, 파일 권한 0600). 외부로 전송하지 않으며 `openside doctor`로 목록을 확인할 수 있다. Chromium 미니덤프, OS 코어 덤프, SIGKILL로 강제 종료된 프로세스는 패닉 보고서가 아니므로 수집하지 않는다.

## 우선순위와 검증 규칙

`crates/openside-config/src/lib.rs` `load_with`, `validate`:

1. 파일: `opensided --config PATH`/`openside doctor --config PATH` 또는 `OPENSIDE_CONFIG` 로 명시한 파일은 반드시 있어야 한다(`explicit configuration file does not exist`). 기본 위치 파일이 없으면 모든 기본값을 쓴다.
2. 환경변수가 파일 값을 덮어쓴다(아래 표의 `env` 열). 빈 문자열 환경변수는 설정되지 않은 것으로 본다.
3. 예외: `OPENSIDE_BROWSER` 는 설정이 아니라 브라우저 탐색 단계에만 쓰이며, `browser.executable` 이 있으면 그 값이 먼저다.
4. `opensided` 플래그 `--data-dir`, `--socket`, `--listen` 은 파일과 환경변수보다 우선한다.
5. 알 수 없는 섹션이나 키는 오류다. `[credentials]` 안쪽만 예외로, Rust 는 내용을 해석하지 않는다.
6. 오류 메시지는 파일과 키 경로만 알려 주고 입력값은 보여 주지 않는다.

값 검증: argv 키(`agent.command`, `vault.passphrase_cmd`)는 비어 있지 않은 실행 파일과 NUL 없는 인자여야 한다. 경로 키는 빈 문자열일 수 없다. `agent.startup_timeout_ms` 는 0 보다 커야 한다. `agent.compaction_keep_messages` 는 `agent.compaction_max_messages` 보다 작아야 한다. `models.default`, `models.fallback` 항목은 빈 문자열일 수 없다. 티켓 TTL 은 1 이상이다.

설정 화면(웹 UI·API)은 `GET`/`PUT /v1/settings/config`(admin)로 이 문서의 모든 키를 JSON 으로 읽고 쓴다. `PUT` 은 파일과 같은 파서로 검증한 뒤(알 수 없는 키·타입·범위 오류는 400 `invalid_config`) 설정 파일을 0600 으로 원자적으로 바꿔 쓴다. 새 값은 데몬을 다시 시작해야 반영되며, 응답의 `restart_required` 가 이를 알려 준다(실행 중에 바뀌는 것은 `/v1/policy` 의 라이브 정책뿐이다). `[credentials]` 는 파일에서만 고친다: 응답에 넣지 않고, `PUT` 에 넣으면 400 `file_only_section` 이며, 쓰기 때 파일에 있던 값을 그대로 둔다.

TCP 통신 환경(원격 관리자 토큰 포함)에서 `PUT /v1/settings/config`로 바꿀 수 있는 설정은 `tickets.*`, `models.default`, `models.fallback`, `memory.default_retention`, `memory.history`뿐이다. 그 밖의 키를 수정하려고 하면 403 `local_socket_required`를 반환하고 설정 파일은 건드리지 않으며, 거부 사실을 `refused_remote`로 기록한다. 실행 파일, 경로, 리스너, 허용 목록, 정책 등을 가리키는 설정은 로컬 소켓(CLI)을 통해서만 바꿀 수 있어, 원격 토큰이 유출되더라도 재시작 후 임의 코드가 실행되는 위험을 막는다.

## 키

`—` 는 기본값 없음(unset)이다.

### `[daemon]`

| 키 | 타입 | 기본값 | env | 설명 |
|---|---|---|---|---|
| `data_dir` | path | — (data 경로) | `OPENSIDE_DATA_DIR` | 세션·capability·audit 기록 디렉터리 |
| `socket` | path | — (`<runtime>/opensided.sock`) | `OPENSIDE_SOCKET` | Unix 소켓 경로. 소켓은 0600 |
| `listen` | `IP:port` | — | `OPENSIDE_LISTEN` | 선택적 TCP 리스너. loopback 만 허용하며 bearer capability 가 필요하다. 포트 0 은 빈 포트를 고르고 실제 포트를 `<data>/opensided.port` 에 쓴다 |

### `[vault]`

| 키 | 타입 | 기본값 | env | 설명 |
|---|---|---|---|---|
| `path` | path | `<data_dir>/vault.db` | `OPENSIDE_VAULT_PATH` | 암호화된 볼트 파일 경로. 파일이 생성되기 전까지 볼트 툴(`vault_login`, `vault_otp`, `vault_passkey`)은 노출되지 않음 |
| `passphrase_cmd` | argv | — | — | 데몬 시작 시 실행되어 출력값으로 passphrase 볼트의 잠금을 해제하는 선택적 명령. 셸 없이 실행되며, 출력값은 잠금 해제에만 사용된 후 메모리에서 안전하게 소멸(zeroized)되고 로그에 기록되지 않음. doctor 실행 시에는 호출되지 않음. 미설정 시 `openside vault unlock` 또는 웹 Vault 페이지에서 잠금 해제 |
| `unlock` | `auto` \| `passphrase` \| `secret-service` \| `native` | `auto` | — | 잠금 해제 방식. `auto`는 기존 파일의 방식을 따르고, 신규 생성 시 접근 가능한 OS 키링(Linux Secret Service, macOS Keychain, Windows Credential Manager)이 있으면 키링을, 없으면 passphrase를 사용함. `secret-service` 및 `native`는 키링 강제(버전 2 파일), `passphrase`는 passphrase 강제(버전 1 파일) |
| `default_policy` | `always` \| `while_unlocked` \| `never` | `while_unlocked` | — | 항목별 재정의가 없는 항목의 접근 정책. 재정의는 `openside vault policy` 로 바꾼다 |

`openside vault init`을 실행하거나(키링 환경에서는 첫 항목 추가로도 가능) 볼트를 생성하고, 선택된 방식은 `openside doctor`로 확인한다. 포맷, 정책 및 감사 관련 상세 규격은 [`docs/specs/vault.md`](specs/vault.md)를 참고한다.

### `[tickets]`

사람 단계 티켓 만료 시간(초, 최소 1). 환경변수 덮어쓰기는 없다.

| 키 | 타입 | 기본값 | 설명 |
|---|---|---|---|
| `confirm_ttl_secs` | u32 | `600` | confirm 만료는 거부된 승인으로 처리 |
| `secret_input_ttl_secs` | u32 | `300` | secret_input 만료 |
| `device_approval_ttl_secs` | u32 | `1800` | device_approval 만료 |
| `info_ttl_secs` | u32 | `86400` | 비밀이 아닌 정보 요청 만료 |
| `takeover_ttl_secs` | u32 | `1800` | takeover 만료 |

### `[agent]`

| 키 | 타입 | 기본값 | env | 설명 |
|---|---|---|---|---|
| `command` | argv | — | `OPENSIDE_AGENT_COMMAND`(JSON 문자열 배열) | 사이드카 실행 argv. 지정하면 자동 탐색을 쓰지 않는다 |
| `dir` | path | — | `OPENSIDE_AGENT_DIR` | 사이드카 Python 프로젝트 디렉터리 |
| `startup_timeout_ms` | u64 | `30000` | — | 사이드카 ready 응답 대기 시간 |
| `tool_retries` | u32 | `3` | — | 툴 거부·실패 뒤 모델 재시도 횟수(사이드카 `open.config.agent`) |
| `compaction_max_messages` | u32 | `100` | — | 대화가 이 길이를 넘으면 요약 압축 |
| `compaction_keep_messages` | u32 | `20` | — | 압축 때 그대로 두는 최근 메시지 수, `compaction_max_messages` 보다 작아야 함 |
| `profile` | string | `"full"` | — | 도구·시스템 프롬프트 프로필: `full`, `browser-agent`, `ultrabrowse`([profiles.md](specs/profiles.md)). 사이드카 `open.config.agent.profile` 로도 전달 |

`command` 가 없을 때(`crates/openside-config/src/discovery.rs`): `dir` 미지정이면 실행 파일 기준 `../share/openside/agent`, `../../agent` 중 `pyproject.toml` 이 있는 곳을 쓴다. uv 가 PATH 에 있고 `dir` 이 정해지면 `uv run --project <dir> python -m openside_agent`, 아니면 `python3 -m openside_agent` 다.

### `[models]`

| 키 | 타입 | 기본값 | env | 설명 |
|---|---|---|---|---|
| `default` | string | — | `OPENSIDE_MODEL` | 기본 모델(`provider:model`). 내장 기본값은 없으며, 세션에 모델을 주지 않으면 `model_not_configured` |
| `fallback` | string[] | `[]` | — | 순서 있는 대체 모델 목록 |

### `[credentials]`

Rust 에는 불투명한 테이블이다. 그대로 사이드카에 넘기고 Debug 출력에도 넣지 않는다. 비밀 값이 아니라 소스 순서와 소스별 설정만 둔다. 아래 키는 `agent/src/openside_agent/providers/config.py`, `sources.py`, `broker.py`, `anthropic.py` 와 `agent/README.md` 에서 읽었다.

| 키 | 타입 | 기본값 | env | 설명 |
|---|---|---|---|---|
| `order.<provider>` | string[] | `anthropic`: `["omp-broker","env","command"]`, `openai-codex`: `["omp-broker","codex-cli"]`, 그 밖: `["env","command"]` | — | 자격 증명 소스 시도 순서. 소스 이름: `omp-broker`, `env`, `command`, `codex-cli`, `builtin`. 모델 fallback 순서와는 별개 |
| `env.<provider>` | string 또는 string[] | 공급자별 기본 변수(아래) | — | `env` 소스가 읽을 환경변수 이름 |
| `command.<provider>` | argv | — | — | `command` 소스. 셸 없이 실행하고 stdout 을 키로 쓴다. stderr 는 버리고 결과는 로그에 남기지 않는다 |
| `command.timeout_seconds` | number | `10` | — | `command` 실행 제한 시간 |
| `omp_broker.url` | string | 선택적 omp 기본 후보 | `OPENSIDE_BROKER_URL`(우선) | auth broker URL |
| `omp_broker.token_file` | path | omp 기본 토큰 파일 후보 (선택적) | `OPENSIDE_BROKER_TOKEN` (토큰 값, 최우선) | 브로커 토큰 파일 경로. 토큰 우선순위는 `OPENSIDE_BROKER_TOKEN` > `omp_broker.token_env` > `omp_broker.token_file` > omp 기본 토큰 파일 후보 순이며, 앞선 환경 변수 값이 비어 있으면 다음 순서로 넘어간다. 유효한 토큰이 없으면 해당 자격 증명 소스는 조용히 비활성화된다 |
| `omp_broker.token_env` | string | — | `OPENSIDE_BROKER_TOKEN` 우선 | 브로커 토큰이 들어 있는 환경 변수의 이름. ASCII 문자 환경 변수 이름만 지원. 설정 파일 안에 토큰 값을 직접 넣지 않는다 |
| `omp_broker.snapshot_ttl_seconds` | number | `15` | — | 스냅샷 메모리 캐시 TTL |
| `omp_broker.timeout_seconds` | number | `10` | — | broker 요청 타임아웃 |
| `omp_broker.sse` | bool | `false` | — | 스냅샷 SSE 스트림 사용 |
| `anthropic.claude_code_version` | semver string | `2.1.280` | `OPENSIDE_CLAUDE_CODE_VERSION`(우선) | Anthropic 요청 형태에 쓰는 버전. semver 가 아니면 기본값 |

omp broker 기본 후보(omp 사용자용, 필수 아님): omp 가 쓰는 기본 loopback 주소와 홈 디렉터리 아래 broker 토큰 파일이다. 정확한 값은 `agent/src/openside_agent/providers/broker.py` 의 상수에 있다.

`env` 기본 변수 이름: `anthropic` → `ANTHROPIC_API_KEY`; `openai`, `openai-chat`, `openai-responses` → `OPENAI_API_KEY`; `openrouter` → `OPENROUTER_API_KEY`; `google` → `GOOGLE_API_KEY`, `GEMINI_API_KEY`; `github-copilot` → `GITHUB_COPILOT_API_KEY`, `GITHUB_COPILOT_API_TOKEN`, `COPILOT_GITHUB_TOKEN`; 그 밖은 `<PROVIDER>_API_KEY`(대문자, `-` → `_`).

`codex-cli` 는 `CODEX_HOME/auth.json` 또는 `~/.codex/auth.json` 을, `builtin` 은 Pydantic AI 기본 동작을 쓰며 별도 키가 없다. broker 장애 시 다운그레이드하지 않는 정책은 `agent/README.md` 를 본다.

```toml
[credentials.order]
anthropic = ["env", "command"]

[credentials.env]
anthropic = "ANTHROPIC_API_KEY"

[credentials.command]
anthropic = ["pass", "show", "services/anthropic"]
timeout_seconds = 10
```

#### 계정 조회 (`openside accounts`)

`openside accounts`는 각 자격 증명 소스별 가용 상태, 마스킹된 식별용 계정 라벨, 설정된 프로바이더 모델 목록, 브로커 사용량 비율, 차단 만료 시각을 보여준다. `openside accounts --json`은 `GET /v1/accounts` (`read` 권한)와 동일한 데이터를 JSON으로 출력하며, 웹 UI의 Accounts 페이지에도 같은 정보가 표시된다.

자격 증명 원문 값, 업스트림 계정 ID, 이메일 주소는 일절 반환하지 않는다. 출력되는 모델 목록은 현재 설정된 모델 목록이며, 브로커 전체 모델 카탈로그가 아니다.

제한 사항: 데몬은 5초 동안 유효한 단일 플라이트(single-flight) 인메모리 캐시를 유지한다. 수집 작업은 설정된 `agent.command`에 `accounts --json`을 붙여 실행하며, 타임아웃은 10초, 표준 출력(stdout) 상한은 1 MiB이다. 최대 32개 소스, 소스당 최대 128개 계정, 최대 64개 설정 모델, 계정당 최대 16개 사용량 윈도우까지 처리한다.

`command` 소스는 실행 파일이 존재하는지만 확인하며, `accounts`나 `doctor` 실행 시 명령어를 실제로 실행하지 않는다. 수집에 실패하면 API는 HTTP 503 `account_sources_unavailable`을 반환한다.

#### 자격 증명 파일 보호

- `FullAccess`를 포함한 모든 정책 모드에서 보호 대상: 어떤 소스에 속하든 설정된 모든 `token_file` 경로, 선택적 브로커 기본 토큰 파일 후보 경로, Codex CLI의 `CODEX_HOME/auth.json`(또는 홈 디렉터리 기준 기본 경로; 빈 `CODEX_HOME`은 미지정으로 처리). 데몬은 경로 메타데이터만 사용하며 해당 파일의 내용을 절대 읽지 않는다.
- 파일 도구(read, write, edit)와 아티팩트 수집기는 읽기나 쓰기를 수행하기 전에 실제 연 파일 디스크립터의 디바이스/아이노드(inode) 식별자를 대조한다. 따라서 직접 경로뿐 아니라 심볼릭 링크, `..` 경로 별칭, 하드링크, 바인드 마운트 별칭을 통한 접근을 모두 거부한다. `read` 디렉터리 목록에서도 보호 대상 항목은 제외된다. 확인된 식별자는 메타데이터 전용 CLOEXEC 디스크립터로 유지하므로 이름 바꾸기(rename)나 삭제(unlink)로 검사를 우회할 수 없다. 식별자는 최대 256개까지 추적하며, 추적에 실패하면 즉시 접근을 차단(fail-closed)하고 데몬을 재시작해야 한다.
- Linux 환경에서는 연 디스크립터에 `fstatfs`를 호출하여 procfs, sysfs 등 프로세스/커널 가상 파일시스템을 판별하고, 다른 이름으로 바인드 마운트된 경우를 포함해 파일 도구 및 아티팩트 수집기의 접근을 거부한다. 이를 통해 `/proc/self/environ`이나 `/proc/<daemon pid>/environ`을 거쳐 상속된 환경 변수 자격 증명을 읽는 경로를 차단한다. Linux 이외의 플랫폼에서는 호스트 파일 차단 검사가 fail-closed 방식으로 즉시 실패한다.
- bash가 bubblewrap 안에서 실행될 때: 보호 파일에 추가 하드링크가 있거나 바인드 별칭으로 노출된 경우(제한된 크기의 `mountinfo`와 해당 파일의 마운트 ID로 확인) 샌드박스 시작을 거부한다. 그 외에 노출된 자격 증명 경로는 빈 읽기 전용 파일로 마스킹한다. 샌드박스를 구성할 수 없으면 명령 실행 자체를 거부하며, 샌드박스 없이 재시도하지 않는다. 셸 환경 변수는 완전히 비우므로 자격 증명 소스의 환경 변수 값은 샌드박스로 상속되지 않는다.
- 자격 증명 키 명령어(`command` 소스): 표준 출력(stdout)은 최대 16 KiB, 전체 타임아웃은 최대 60초(설정된 제한 시간이 더 짧을 수 있음)로 제한되며, 표준 오류(stderr)는 버린다. 전용 Unix 프로세스 그룹에서 실행되며, 출력 초과·타임아웃·취소·종료 시 1초 이내에 프로세스 그룹을 강제 종료하고 리핑(reap)한다. 프로세스 그룹 격리를 지원하지 않는 플랫폼에서는 fail-closed로 동작한다.
- 브로커 응답: 개별 GET 응답, SSE 라인 및 이벤트, 보관 스냅샷은 각각 최대 4 MiB로 제한되며, 캐시 자격 증명은 최대 512개까지 유지한다. 허용 크기를 넘어서거나 유효하지 않은 스트림은 즉시 닫고 캐시를 폐기하며 폴링을 통해 복구한다. 에러 메시지에는 원시 응답 본문을 절대 포함하지 않는다.
- 잔여 위험 요소 (명시적 고지): (1) 샌드박스가 꺼진 `FullAccess` bash는 임의의 호스트 셸 권한을 가지므로 자격 증명 파일을 의도적으로 읽을 수 있다. openside는 이러한 사용자의 명시적 선택까지 막지는 않으므로, 이 경계가 중요하다면 `sandbox.required = true`로 설정해야 한다. 명령어 문자열 필터링은 수행하지 않는다. (2) 샌드박스가 이미 실행 중인 동안 모델이 아닌 호스트 프로세스가 경쟁적으로 새 링크나 바인드 별칭을 만드는 상황은 위협 모델 범위 밖이다. 기존 별칭은 샌드박스 시작 시점에 검사한다. (3) 사용자가 메시지나 첨부파일에 직접 붙여넣은 자격 증명은 사용자 입력이며, 소스 자동 읽기 대상이 아니다.

#### broker wire 계약 점검

`docs/specs/omp-broker-wire.schema.json`은 openside가 사용하는 스냅샷, 사용량, 범위 한정 차단 필드를 엄격히 고정한다 (출처 커밋은 `$comment`에 기록됨). 업스트림의 추가 필드는 허용된다. openside가 사용하는 필드가 삭제되거나 이름이 바뀌거나 타입이 변경되면 밀폐 테스트(hermetic test)가 실패한다. omp 업그레이드 뒤에는 선택형 점검을 실행할 수 있다. `OPENSIDE_LIVE_BROKER_URL`을 직접 지정하고 `OPENSIDE_LIVE_BROKER_TOKEN_ENV`에 기존 토큰 환경 변수의 이름을 지정한 뒤 `cd agent && uv run pytest -q -m live_broker`를 실행한다. 이 점검은 `GET /v1/snapshot`과 `GET /v1/usage`만 호출하며 변경 요청이나 프로바이더 요청은 일절 보내지 않는다. 응답 값은 출력하지 않고 마스킹된 스키마 불일치 에러만 보고한다. 기본 URL이나 토큰 파일은 자동으로 탐색하지 않는다. 또한 `live` 마커가 붙어 있어 `-m 'not live'`로 실행하면 검사 대상에서 제외된다.

### `[browser]`

| 키 | 타입 | 기본값 | env | 설명 |
|---|---|---|---|---|
| `executable` | path | — | — | Chromium 호환 실행 파일. `OPENSIDE_BROWSER` 보다 우선 |
| `bundle_dir` | path | — | — | `chrome`, 그다음 `chromium` 을 찾을 디렉터리 |
| `gateway` | `auto`·`off` | `auto` | — | `auto` 면 띄운 브라우저가 gateway(패치한 Chromium, Linux 전용)를 열었을 때 그것을 쓰고, 없으면 CDP만 쓴다. `off` 면 항상 CDP만 쓴다 |
| `extensions` | path[] | `[]` | — | 신뢰하는 압축 해제 확장 디렉터리 목록. 데몬이 직접 띄우는 순정 Chromium 에 이 확장을 불러와, Agent Tabs 가 에이전트 소유 탭을 id 로 묶을 수 있게 한다. 상대 경로는 설정 파일 디렉터리 기준이고, 실제로 있는 디렉터리여야 한다. Chromium 확장 목록은 쉼표로 구분하므로 쉼표나 NUL 이 든 경로는 거부한다. 여기 적은 확장만 켠다(`--load-extension`, `--disable-extensions-except`, 셸 없이 프로세스 인수로 전달). 시크릿 세션의 브라우저에는 불러오지 않는다. Google Chrome 정식 버전(137 이상)은 `--load-extension` 옵션을 무시하므로 Chromium이나 Chrome for Testing을 사용해야 한다 |

탐색 순서: `executable` → `OPENSIDE_BROWSER` → `bundle_dir` → PATH(`chromium`, `chromium-browser`, `google-chrome`, `google-chrome-stable`, `chrome`) → 플랫폼 위치(macOS 앱 번들 두 곳, Windows 는 `LOCALAPPDATA`/`PROGRAMFILES`/`PROGRAMFILES(X86)` 아래, Linux 없음).

### `[sandbox]`

| 키 | 타입 | 기본값 | env | 설명 |
|---|---|---|---|---|
| `required` | bool | `false` | — | `true` 면 bwrap 이 없을 때 `bash` 도구를 숨긴다 |
| `bwrap` | path | — (PATH 에서 `bwrap` 탐색) | — | bubblewrap 실행 파일 |

### `[policy]`

| 키 | 타입 | 기본값 | env | 설명 |
|---|---|---|---|---|
| `mode` | `read-only` \| `guard` \| `full` | `guard` | — | 세션 기본 권한 모드. `read-only` 는 변경·실행·민감 도구를 숨기고, `guard` 는 승인된 루트 밖 작업을 묻고, `full` 도 민감 작업은 승인을 요구한다. CLI `--mode` 로 세션별 지정 |
| `allow_origins` | string[] | `[]` | — | 비어 있지 않으면 브라우저·Vault·민감 작업을 이 origin glob(`scheme://host[:port]`)에서만 허용한다. 제한만 하고 권한을 넓히지 않는다 |
| `deny_origins` | string[] | `[]` | — | 이 origin glob 에서는 브라우저·Vault·민감 작업을 모든 모드에서 거부한다(예: `https://*.evil.example`) |

`PUT /v1/policy`(`openside policy set`)로 저장한 `<data_dir>/policy.json` 이 있으면 `[policy]` 대신 그 파일을 쓴다. 자세한 내용은 [specs/policy.md](specs/policy.md).

### `[network]`

| 키 | 타입 | 기본값 | env | 설명 |
|---|---|---|---|---|
| `allow` | string[] | `[]` | — | loopback 과 설정한 모델·채널 엔드포인트 외에 허용할 외부 목적지. `openside_policy::Egress` 문법(`host`, `*.domain`, `host:port`, `scheme://host[:port]`)을 따른다. 사이드카의 HTTP 전송, 채널 어댑터, `memory model fetch` 가 이 목록 밖으로 나가는 요청(리다이렉트 포함)을 거부한다. 문법이 틀리면 데몬이 시작하지 않는다. bash 샌드박스는 이 목록과 무관하게 네트워크가 없다 |

### `[ui]`

| 키 | 타입 | 기본값 | env | 설명 |
|---|---|---|---|---|
| `dir` | path | — | `OPENSIDE_UI_DIR` | 데몬이 `/` 에서 서빙할 웹 UI 빌드 디렉터리(`pnpm -C web build` 결과인 `web/dist`). `index.html` 이 없으면 UI 를 끈다. 없는 경로는 `index.html` 로 돌려주고(SPA), `/v1/*` 는 가리지 않는다. 응답에 CSP(인라인 스크립트 금지, `connect-src 'self'`)를 붙인다 |

### `[memory]`

| 키 | 타입 | 기본값 | env | 설명 |
|---|---|---|---|---|
| `default_retention` | `permanent` \| `days_90` \| `days_30` | `permanent` | — | 보존 기간을 지정하지 않은 메모의 기본값. 브라우징 기록은 이 값과 관계없이 항상 `days_30` |
| `history` | bool | `true` | — | `false` 면 `browser_open_tab`·`browser_navigate`·`browser_snapshot` 으로 본 페이지를 기억에 저장하지 않는다 |
| `embedding.model_dir` | path | `<data_dir>/memory/model` | — | 로컬 임베딩 모델(`tokenizer.json`, `model.safetensors`) 디렉터리. 파일이 없으면 검색은 lexical-only 로 동작한다. 데몬은 모델을 내려받지 않는다. `openside memory model fetch <owner/name>` 으로 받는다 |

### `[skills]`

스킬(`SKILL.md` 디렉터리) 라이브러리와 다른 에이전트 연동 설정이다. 형식과 동작은 [`docs/specs/skills.md`](specs/skills.md) 에 있다.

| 키 | 타입 | 기본값 | env | 설명 |
|---|---|---|---|---|
| `dir` | path | `<data_dir>/skills` | — | 사용자 스킬 라이브러리. 데몬을 `--data-dir` 로 따로 띄우면 CLI 와 기본 경로가 달라지므로 이 키로 맞춘다 |
| `project` | bool | `true` | — | `false` 면 세션 작업공간의 `.openside/skills/`(읽기 전용)를 쓰지 않는다 |
| `agents.<이름>` | path | — | `OPENSIDE_SKILLS_<이름>_DIR` | `openside skills install <이름>` 이 openside 브라우저 스킬을 넣을 그 에이전트의 스킬 디렉터리. 설정 항목이 환경변수보다 먼저고, `claude`·`codex`·`omp` 는 둘 다 없으면 각자의 관례 경로(`$CLAUDE_CONFIG_DIR`/`$CODEX_HOME`/`$OMP_CODING_AGENT_DIR`(다음 `$PI_CODING_AGENT_DIR`) 아래 `skills`, 없으면 `$HOME` 아래 각 에이전트의 설정 디렉터리)를 쓴다 |

### `[channels]`

채팅 어댑터(`openside-channels`)용이다. 어댑터는 데몬 공개 API 의 클라이언트이며 자세한 내용은 [`docs/specs/channels.md`](specs/channels.md) 에 있다. 비밀 값은 키에 직접 쓰지 않고 환경변수 **이름**(`*_env`)이나 argv(`*_cmd`, 셸 없이 실행하고 stdout 이 값)로만 지정한다.

| 키 | 타입 | 기본값 | env | 설명 |
|---|---|---|---|---|
| `api_url` | string | `http://<daemon.listen>` | `OPENSIDE_API_URL` | 어댑터가 연결할 데몬 주소. https 이거나 loopback http 여야 한다 |
| `token_file` | path | — | — | 데몬 bearer 토큰 파일(`read`·`steer`·`approve` 범위). 없으면 `token_env` 변수를 읽는다 |
| `token_env` | string | `OPENSIDE_TOKEN` | — | 데몬 토큰이 들어 있는 환경변수 이름 |
| `stt_cmd` | string[] | — | — | 로컬 음성 전사 명령. 오디오 파일 경로를 마지막 인자로 붙여 실행하고 stdout 을 전사문으로 쓴다. 없으면 음성 메시지를 정중히 거절한다 |
| `stt_timeout_secs` | int | `120` | — | 전사 제한 시간. 0 이면 거부 |
| `max_audio_bytes` | int | `26214400` | — | 받아 줄 오디오 최대 크기. 0 이면 거부 |
| `edit_interval_ms` | int | `1000` | — | 스트리밍 답글을 고쳐 쓰는 최소 간격. 200 미만이면 거부 |
| `state_dir` | path | `<data_dir>/channels` | — | 스레드-세션 매핑 저장 위치(디렉터리 0700, 파일 0600) |

`[channels.telegram]`, `[channels.slack]`, `[channels.discord]` 는 모두 `enabled = false` 가 기본이다. 켜려면 허용 목록(`allowed_*`) 중 하나 이상을 채워야 하며, 비어 있으면 설정을 읽을 때 거부한다. ID 는 문자열이나 정수로 쓴다.

| 표 | 키 | 기본값 | 설명 |
|---|---|---|---|
| `[channels.telegram]` | `api_base` | `https://api.telegram.org` | Bot API 주소 |
| | `token_env`, `token_cmd` | — | 봇 토큰 출처 |
| | `allowed_chats`, `allowed_users` | `[]` | 비어 있지 않은 목록은 모두 만족해야 한다 |
| | `poll_timeout_secs` | `30` | `getUpdates` 롱폴링 시간. 0 이면 거부 |
| `[channels.slack]` | `api_base` | `https://slack.com/api` | Web API 주소 |
| | `listen` | `127.0.0.1:8787` | `/slack/events`, `/slack/interactivity` 수신 주소 |
| | `bot_token_env`, `bot_token_cmd` | — | 봇 토큰 출처 |
| | `signing_secret_env`, `signing_secret_cmd` | — | 요청 서명 검증용 signing secret 출처 |
| | `allowed_workspaces`, `allowed_channels`, `allowed_users` | `[]` | 허용 목록 |
| `[channels.discord]` | `api_base` | `https://discord.com/api/v10` | REST 주소 |
| | `listen` | `127.0.0.1:8788` | `/discord/interactions` 수신 주소 |
| | `public_key` | — | 애플리케이션 공개키(hex). Ed25519 서명 검증에 쓴다 |
| | `bot_token_env`, `bot_token_cmd` | — | 봇 토큰 출처 |
| | `allowed_guilds`, `allowed_channels`, `allowed_users` | `[]` | 허용 목록 |

### `[sync]`

설정(라이브 정책), 핀 탭, Vault 항목, 태그로 고른 메모리를 기기 사이에 종단간 암호화로 동기화한다. openside 서버 없이 사용자의 WebDAV 공유나 공유 디렉터리를 쓴다. 백엔드는 둘 중 하나만 쓰고, 둘 다 비우면 동기화를 쓰지 않는다. 동작과 충돌 해결 규칙은 [`docs/specs/sync.md`](specs/sync.md) 에 있다.

| 키 | 타입 | 기본값 | env | 설명 |
|---|---|---|---|---|
| `webdav_url` | string | — | — | WebDAV 컬렉션 주소. 계정마다 그 아래에 컬렉션을 하나 만든다. https 여야 하고, http 는 loopback 주소에만 허용한다. 사용자 정보·쿼리가 들어간 주소는 거부한다. 저장소에는 암호문만 남는다 |
| `webdav_username` | string | — | — | WebDAV HTTP Basic 사용자 이름. `webdav_password_cmd` 와 함께 설정한다 |
| `webdav_password_cmd` | string[] | — | — | WebDAV 비밀번호를 stdout 으로 내는 명령(셸 없이 실행). 출력은 기록하지 않는다 |
| `dir` | path | — | — | WebDAV 대신 쓰는 공유 디렉터리(Syncthing·네트워크 공유 등). `webdav_url` 과 함께 쓰면 거부한다 |
| `passphrase_cmd` | string[] | — | — | 동기화 패스프레이즈를 stdout 으로 내는 명령(셸 없이 실행). 모든 기기에 같은 패스프레이즈를 설정한다. 유도한 키는 데몬 메모리에만 두며, doctor 는 이 명령을 실행하지 않는다 |
| `settings` | bool | `false` | — | 라이브 정책 문서(`/v1/policy`)도 동기화할지 여부. 켜면 페어링한 기기가 이 기기의 정책을 바꿀 수 있다(약하게 만드는 변경 포함) |
| `memory_tags` | string[] | `["sync"]` | — | 이 태그 중 하나가 붙은 메모리만 동기화한다. 모든 기기에서 같게 둔다. 비우면 메모리를 동기화하지 않는다 |

### `[remote]`

휴대폰 같은 다른 기기에서 쓸 때의 설정이다. 데몬은 loopback 에서만 듣고, TLS 는 앞단(Tailscale Serve, 리버스 프록시)이 맡는다. 구성 방법은 [`docs/REMOTE.md`](REMOTE.md) 에 있다.

| 키 | 타입 | 기본값 | env | 설명 |
|---|---|---|---|---|
| `public_url` | string | — | — | 다른 기기가 접속하는 주소(예: `https://desk.tail1234.ts.net`). `openside pair` 의 QR 이 `<public_url>/pair#<코드>` 를 가리킨다. http(s) 주소만 받고, 사용자 정보·쿼리·프래그먼트가 있으면 거부한다. doctor 는 loopback 이 아닌 http 주소를 실패로 표시한다 |
| `push_contact` | string | `public_url`(https 일 때), 없으면 `mailto:openside@localhost` | — | Web Push VAPID 토큰의 `sub` 연락처. 푸시 서비스 운영자가 문제가 생겼을 때 연락하는 주소다. `mailto:` 또는 `https://`로 시작해야 한다. 그렇지 않으면 설정 검증에 실패한다 |

### `[otp]`

SMS·메일 전달 앱이 `POST /v1/otp`(`otp_ingest` 범위 토큰)로 보낸 문자에서 일회용 코드를 뽑아, 그 사이트의 대기 중인 `secret_input` 티켓을 자동으로 채운다. 코드는 메모리에만 두고 디스크·로그·감사 기록에 남기지 않는다.

| 키 | 타입 | 기본값 | env | 설명 |
|---|---|---|---|---|
| `senders` | string[] | `[]` | — | 받아 줄 발신자(전화번호, 메일 주소, 발신 ID). 정확히 일치해야 하며, 목록 밖 발신자는 403 `sender_not_allowed` 로 거부하고 감사 기록에 남긴다. 비어 있으면 모두 거부한다 |
| `sites.<origin>` | string(정규식) | — | — | 사이트 origin(`https://login.example.com`, 소문자·경로 없음)별 코드 추출 정규식. 첫 캡처 그룹(없으면 전체 일치)이 코드다. 컴파일되지 않으면 데몬이 시작하지 않는다 |
| `ttl_secs` | int | `300` | — | 맞는 티켓이 아직 없을 때 코드를 기다려 두는 시간. 지나면 버린다. 0 이면 거부 |

### `[service]`

| 키 | 타입 | 기본값 | env | 설명 |
|---|---|---|---|---|
| `inhibit_sleep` | bool | `false` | — | 실행 중인 세션이 하나라도 있으면 시스템 절전을 막는다. 마지막 세션이 끝나면 바로 푼다 |
| `inhibit_cmd` | string[] | Linux `systemd-inhibit --what=sleep:idle … sleep infinity`, macOS `caffeinate -i` | — | 절전을 막는 동안 띄워 둘 명령(셸 없이 실행). 이 프로세스를 끝내면 잠금이 풀린다 |

서비스 등록은 `openside service install` 이 한다([`docs/REMOTE.md`](REMOTE.md#5-상시-실행)).

## 그 밖의 환경변수

설정 키에 대응하지 않는 변수다.

| 변수 | 읽는 쪽 | 용도 |
|---|---|---|
| `OPENSIDE_TOKEN` | CLI | `--url`(loopback HTTP) 연결의 bearer 토큰 |
| `OPENSIDE_BROWSER` | 브라우저 탐색 | 위 `[browser]` 참고 |
| `OPENSIDE_SIDECAR_SOCKET`, `OPENSIDE_SIDECAR_NONCE` | 사이드카 | 데몬이 주입한다. 직접 설정하지 않는다 |
| `OPENSIDE_DOCTOR_CONFIG` | 사이드카 `doctor` | doctor 가 `credentials`, `models` JSON 을 넘길 때 쓴다 |
| `OPENSIDE_HF_ENDPOINT` | CLI `memory model fetch` | 모델 저장소 주소(기본 `https://huggingface.co`). 테스트 서버용 loopback 외에는 https 만 허용 |
| `OPENSIDE_AGENT_SCRIPT` | 사이드카 | 테스트용 스크립트 모델 경로(`openside-e2e`) |
| `OPENSIDE_LIVE_MODEL`, `OPENSIDE_LIVE_CREDENTIALS` | `agent/tests` | live 테스트 opt-in |
