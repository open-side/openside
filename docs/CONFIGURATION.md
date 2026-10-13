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
5. 알 수 없는 최상위 섹션이나 키는 오류로 처리한다. `credentials`는 플러그인 메타데이터 호환성을 유지하며, 데몬의 자격 증명 소유자만 해석하고 Debug 형식으로 절대 출력하지 않는다.
6. 오류 메시지는 파일과 키 경로만 알려 주고 입력값은 보여 주지 않는다.

값 검증: argv 키(`agent.command`, `vault.passphrase_cmd`)는 비어 있지 않은 실행 파일과 NUL 없는 인자여야 한다. 경로 키는 빈 문자열일 수 없다. `agent.startup_timeout_ms` 는 0 보다 커야 한다. `agent.compaction_keep_messages` 는 `agent.compaction_max_messages` 보다 작아야 한다. `models.default`, `models.fallback` 항목은 빈 문자열일 수 없다. 티켓 TTL 은 1 이상이다.

`GET`/`PUT /v1/settings/config`(admin)는 파일과 동일한 유효성 검사 및 원자적 0600 저장으로 JSON 설정을 읽고 쓴다. `credentials`는 파일 전용으로 유지되어 응답에서 제외되고, PUT 시 거부되며, 디스크의 기존 내용이 보존된다. `restart_required`는 저장된 설정과 실행 중인 설정을 비교하며, 여기에는 실시간 원격, models, channels, sync, 외부 에이전트 폴더 오버레이가 포함된다. 전용 엔드포인트는 해당 오버레이를 즉시 반영하며, 설정 파일 직접 편집은 재시작 시 적용된다.

로컬 권한(local authority)은 비공개 Unix 소켓이나 소켓에서 발급한 person/admin 토큰 및 로컬 시크릿을 갖춘 검증된 루프백 런처를 뜻한다. 로컬 권한은 파일 전용이 아닌 모든 키를 편집할 수 있다. 원격 person/admin 토큰은 `tickets.*`, `models.default/fallback/front`, `memory.default_retention/history`, `agent.startup_timeout_ms/tool_retries/compaction_max_messages/compaction_keep_messages/orchestration`, `service.inhibit_sleep`만 편집할 수 있으며, 허용되지 않은 변경은 `local_access_required`를 반환하고 감사에 기록된다. TCP 에이전트 클라이언트는 설정을 쓸 수 없다(`forbidden`). `POST /v1/daemon/restart`는 로컬 권한이 필요하며, 정상 종료(graceful shutdown) 후 동일한 PID로 re-exec한다. 만료되는 비공개 일회용 요청 바인딩 핸드오프는 이 제어된 exec 동안에만 런처 시크릿을 보존하며, 일반적인 부팅 시에는 보존하지 않는다.

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

데몬은 소스 설정과 사람이 구성한 프로바이더 풀을 해석한다. 여기에는 소스 순서, 환경 변수 이름, command argv, 브로커 경로, 계정 참조만 저장하며, API 키나 OAuth 값은 절대 넣지 않는다. 암호화된 Vault 항목과 명시적 풀은 전용 Models & accounts 엔드포인트에서 관리한다. 사이드카에는 요청당 액세스 리스와 검증된 요청 셰이핑 메타데이터만 전달된다. 출처: `crates/opensided/src/model_accounts.rs`, `model_broker.rs`, `managed_subscriptions.rs`, `docs/specs/models-accounts.md`.

| 키 | 타입 | 기본값 | env | 설명 |
|---|---|---|---|---|
| `order.<provider>` | string[] | `anthropic`: `["omp-broker","claude-code","vault","env","command"]`, `openai-codex`: `["omp-broker","codex-cli"]`, other: `["vault","env","command"]` | — | 자격 증명 우선순위이며 모델 fallback과는 독립적이다. 명시적으로 활성화하지 않으면 Claude Code는 건너뛴다. 명시적 순서 설정이 기본값을 덮어쓴다. |
| `pools.<provider>` | `{source,account_id}[]` | 선택한 소스에서만 마이그레이션됨 | — | 순서가 지정된 명시적 신뢰 경계이며, 비어 있으면 라우팅을 비활성화한다. 다른 소스에서 호환되는 계정을 추가하는 작업은 사람만 수행할 수 있다. |
| `subscriptions.claude-code` | bool | `false` | — | 기존 Claude Code 구독 로그인의 재사용을 허용한다. 로그인은 외부 명령(`claude auth login`)으로 유지된다. |
| `subscriptions.codex-cli` | bool | `true` | — | 기존 Codex CLI 구독 로그인의 재사용을 허용한다. 로그인은 외부 명령(`codex login`)으로 유지된다. |
| `env.<provider>` | string 또는 string[] | 공급자별 기본 변수(아래) | — | `env` 소스가 읽을 환경변수 이름 |
| `command.<provider>` | argv | — | — | `command` 소스. 셸 없이 실행하고 stdout 을 키로 쓴다. stderr 는 버리고 결과는 로그에 남기지 않는다 |
| `command.timeout_seconds` | number | `10` | — | `command` 실행 제한 시간 |
| `omp_broker.url` | string | 선택적 omp 기본 후보 | `OPENSIDE_BROKER_URL`(우선) | 인증 브로커 URL |
| `omp_broker.token_file` | path | omp 기본 토큰 파일 후보 (선택적) | `OPENSIDE_BROKER_TOKEN` (토큰 값, 최우선) | 브로커 토큰 파일 경로. 토큰 우선순위는 `OPENSIDE_BROKER_TOKEN` > `omp_broker.token_env` > `omp_broker.token_file` > omp 기본 토큰 파일 후보 순이며, 앞선 환경 변수 값이 비어 있으면 다음 순서로 넘어간다. 유효한 토큰이 없으면 해당 자격 증명 소스는 조용히 비활성화된다 |
| `omp_broker.token_env` | string | — | `OPENSIDE_BROKER_TOKEN` 우선 | 브로커 토큰이 들어 있는 환경 변수의 이름. ASCII 문자 환경 변수 이름만 지원. 설정 파일 안에 토큰 값을 직접 넣지 않는다 |
| `omp_broker.snapshot_ttl_seconds` | number | `15` | — | 스냅샷 메모리 캐시 TTL |
| `omp_broker.timeout_seconds` | number | `10` | — | broker 요청 타임아웃 |
| `omp_broker.sse` | bool | `false` | — | 스냅샷 SSE 스트림 사용 |
| `anthropic.claude_code_version` | semver string | `2.1.280` | `OPENSIDE_CLAUDE_CODE_VERSION`(우선) | Anthropic 요청 형태에 쓰는 버전. semver 가 아니면 기본값 |

선택적 브로커 후보는 `http://127.0.0.1:8765`와 `~/.omp/auth-broker.token`으로 유지되며, 필수가 아니다. 자격 증명 URL 및 토큰 확인은 Python broker.py 상수가 아니라 데몬이 전적으로 담당한다. 브로커 시크릿 스냅샷은 명시적 프로바이더 풀 신뢰 경계 안에서 데몬만이 가져온다.

`env` 기본 변수 이름: `anthropic` → `ANTHROPIC_API_KEY`; `openai`, `openai-chat`, `openai-responses` → `OPENAI_API_KEY`; `openrouter` → `OPENROUTER_API_KEY`; `google` → `GOOGLE_API_KEY`, `GEMINI_API_KEY`; `github-copilot` → `GITHUB_COPILOT_API_KEY`, `GITHUB_COPILOT_API_TOKEN`, `COPILOT_GITHUB_TOKEN`; 그 밖은 `<PROVIDER>_API_KEY`(대문자, `-` → `_`).

Models & accounts는 라벨이 지정된 N개의 프로바이더 키와 격리된 외부 CLI 홈을 관리하며, 계정별 활성화와 순서가 지정된 명시적 프로바이더 풀을 제공한다. 관리형 API 및 OAuth 상태의 정본은 Vault에 보관한다. 데몬 전용 갱신은 리비전과 mtime을 기준으로 비공개 관리형 CLI 파일을 생성하거나 반영하며, 계정 삭제 시 이를 완전히 지운다. 기본 사용자 홈은 읽기 전용 상태를 유지한다. 사이드카는 외부 요청 시도마다 새로운 액세스 자격 증명만 받으며, 리프레시 토큰은 절대 전달되지 않는다. 브로커 장애가 발생해도 목록에 없는 소스로 넘어가지 않으며, 여러 소스가 혼합된 풀은 사람의 승인이 필요하다. `docs/specs/models-accounts.md`를 참고한다.

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

자격 증명 값, 업스트림 계정 ID, 이메일은 절대 반환되지 않는다. 계정 진단 정보에는 브로커가 보고한 모델 목록이 제공될 경우 이를 포함한다. `openside models`와 GET `/v1/settings/models`는 설정된 모델, 소스 계정, 대화 및 도구 사용이 가능한 선별된 프로바이더 목록을 준비 상태 사유와 함께 병합하여 제공한다.

계정 진단 정보는 데몬이 집계하며, 세대(generation) 무효화와 single-flight 갱신을 통해 최대 5초 동안 캐시한다. 진단 시 자격 증명 명령을 실행하지 않는다. Python `models --json` 자식 프로세스는 최대 1 MiB로 제한된 stdout으로 자격 증명이 없는 SDK 인덱스만 출력하며, 자격 증명이 포함된 설정이나 환경 변수는 일절 상속받지 않는다.

`command` 소스는 실행 파일이 존재하는지만 확인하며, `accounts`나 `doctor` 실행 시 명령어를 실제로 실행하지 않는다. 수집에 실패하면 API는 HTTP 503 `account_sources_unavailable`을 반환한다.

#### 자격 증명 파일 보호

- 모든 정책 모드는 설정된 token_file 경로, 브로커 기본 경로, 읽기 전용 Claude/Codex 자격 증명 파일, 관리형 구독 홈을 보호하며, 하드링크나 연결 해제(unlinked)된 별칭을 가리키는 보존 파일 디스크립터까지 포함한다. 보호 메커니즘은 메타데이터만 추적한다. 신뢰할 수 있는 분리된 데몬 자격 증명 소유자만 이들 파일을 읽을 수 있으며, 모델 도구, 아티팩트 수집기, 샌드박스 내부의 bash는 절대 읽을 수 없다.
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
| `embedding.model_dir` | path | `<data_dir>/memory/model` | — | 로컬 정적 임베딩 모델 폴더(`tokenizer.json`, `model.safetensors`). 파일이 없으면 어휘 전용 검색만 수행한다. Memory 페이지(`POST /v1/memory/model/download`)나 `openside memory model fetch`로 명시적으로 다운로드한다. 암묵적 다운로드는 수행하지 않으며, 데몬이 진행 상황, 취소, 체크섬, 재시작 필요 여부를 보고한다. |

### `[skills]`

스킬(`SKILL.md` 디렉터리) 라이브러리와 다른 에이전트 연동 설정이다. 형식과 동작은 [`docs/specs/skills.md`](specs/skills.md) 에 있다.

| 키 | 타입 | 기본값 | env | 설명 |
|---|---|---|---|---|
| `dir` | path | `<data_dir>/skills` | — | 사용자 스킬 라이브러리. 데몬을 `--data-dir` 로 따로 띄우면 CLI 와 기본 경로가 달라지므로 이 키로 맞춘다 |
| `project` | bool | `true` | — | `false` 면 세션 작업공간의 `.openside/skills/`(읽기 전용)를 쓰지 않는다 |
| `agents.<이름>` | path | — | `OPENSIDE_SKILLS_<이름>_DIR` | `openside skills install <이름>` 이 openside 브라우저 스킬을 넣을 그 에이전트의 스킬 디렉터리. 설정 항목이 환경변수보다 먼저고, `claude`·`codex`·`omp` 는 둘 다 없으면 각자의 관례 경로(`$CLAUDE_CONFIG_DIR`/`$CODEX_HOME`/`$OMP_CODING_AGENT_DIR`(다음 `$PI_CODING_AGENT_DIR`) 아래 `skills`, 없으면 `$HOME` 아래 각 에이전트의 설정 디렉터리)를 쓴다 |

### `[channels]`

관리형 Telegram, Slack, Discord 계정은 Settings → Channels 또는 `/v1/settings/channels`를 통해서만 설정한다. 각 서비스는 라벨이 지정된 여러 ID를 지원하며, 독립적인 활성화, teammate 할당, 상태 조회가 가능하다. 시크릿 값은 타입화된 `channel_secret` Vault 항목으로 암호화되어 저장되며, 메타데이터로 반환되거나 config/argv/logs에 저장되거나 동기화되거나 브라우저 자동 완성으로 노출되지 않는다. 기존 env/command 테이블은 독립 실행형(standalone) 러너에서 서비스당 1개의 레거시 계정으로 계속 허용된다.

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
| `accounts` | object[] | `[]` | — | `id`, `kind`, `label`, `enabled`, `agent_id`, `secret_id`, `options`를 가진 독립 계정 목록. id는 kind별로 1~40자의 소문자·숫자·밑줄·하이픈만 허용되며, label은 제어 문자가 없는 1~128바이트 문자열이다. `secret_id`는 Vault의 `channel:{kind}:{id}`를 참조하며 자격 증명 값을 직접 포함하지 않는다. |

`[[channels.accounts]]`는 내부 및 파일의 정본 표현이다. 보안 폼을 통해 라벨, 활성화 여부, teammate, 허용 목록, 플랫폼 필드를 수정하며, 마스킹된 자격 증명을 교체하면 해당 계정의 시크릿만 로테이션된다. 원격 GUI 작성 시 표준 서비스 API 목적지는 고정되어 있으며, 커스텀 목적지는 로컬 소켓을 거쳐야 한다. 관리형 Slack/Discord 리스너는 기본적으로 임시 루프백 포트를 사용하며 실제 콜백 주소를 메타데이터로 반환한다. 계정 변경 사항은 데몬 TCP 감독 하에 실시간으로 적용되며, 그렇지 않으면 `restart_required`가 true가 된다. 에이전트가 지정되지 않은 새 채널 스레드는 chief에게 배정되고, 기존 스레드는 담당 에이전트를 유지한다. `openside channels list|add|rm|test` 명령을 사용하며, add는 숨김 처리된 표준 입력(stdin)으로만 값을 읽는다(Slack은 봇 토큰과 signing secret을 별도의 줄로 입력받는다).

관리형 Slack/Discord 계정에서 webhook_url은 플랫폼에 등록해야 하는 정확한 공개 `/v1/hooks/{kind}/{account_id}` URL이다. 현재 주소가 루프백이면 GUI의 원격 접속 설정을 사용해야 한다. receiver_address는 호환 리스너 진단 정보일 뿐이며 관리형 콜백 주소가 아니다. effective_agent_id는 실제 라우팅 대상을 나타내는 읽기 전용 값이며, agent_id는 수정 가능한 재정의(override) 필드로 유지된다. API나 UI를 통한 명시적 Vault 잠금은 관리형 봇을 중지시키지만, OS 키링이나 화면 유휴(idle) 상태는 봇을 중지시키지 않는다.

레거시 `[channels.telegram]`, `[channels.slack]`, `[channels.discord]` 테이블은 각각 `legacy-telegram`, `legacy-slack`, `legacy-discord`로 정규화된다. 기본값은 비활성화 상태이며, 활성화된 계정에는 최소 하나 이상의 관련 허용 목록이 여전히 필요하다. TOML에서는 숫자 ID도 계속 허용된다. 레거시 소스 참조는 파일 전용으로 유지되며 채널 메타데이터에서는 제외된다. 보안 계정 폼을 통해 자격 증명을 교체하면 기존 스레드 매핑을 변경하지 않고도 동일한 ID를 Vault 관리로 마이그레이션할 수 있다. 다음 표는 호환용 입력 형식만 설명하며, 새 계정은 `accounts[].options`를 사용한다.

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
| `OPENSIDE_HF_ENDPOINT` | CLI `memory model fetch` | CLI 모델 허브 엔드포인트(기본값 `https://huggingface.co`). 데몬의 요청은 로컬 권한이 필요한 명시적 엔드포인트 재정의를 사용한다. 루프백 테스트 서버를 제외하고 HTTPS여야 한다. |
| `OPENSIDE_AGENT_SCRIPT` | 사이드카 | 테스트용 스크립트 모델 경로(`openside-e2e`) |
| `OPENSIDE_LIVE_MODEL`, `OPENSIDE_LIVE_CREDENTIALS` | `agent/tests` | live 테스트 opt-in |
