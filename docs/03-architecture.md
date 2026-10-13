# 03. 아키텍처

## 1. 한눈에
```
┌───────────── clients (모두 같은 공개 API 의 클라이언트) ─────────────┐
│ openside CLI (구현) │ MCP · Web/PWA · 채널봇 · 확장 UI (계획)        │
└────────┬─────────────────────────────────────────────────────────────┘
         │ unix socket(관리자) / loopback HTTP + SSE(능력 토큰), OpenAPI 3.1
┌────────▼──────────────────────── opensided (Rust 데몬) ─────────────────────────┐
│ api ── sessions(event-sourced, SQLite) ── queue ── tickets(사람 단계·승인)       │
│ policy(Read only/Guard/Full) ── tools(파일·bash·browser·vault·human) ── audit    │
│ vault(redaction) ── browser controller(CDP) ── doctor(openside-config)           │
└────────┬───────────────────────────────────────┬─────────────────────────────────┘
         │ openside-agent-sidecar                │ CDP (WebSocket)
         │ 0700 dir / 0600 unix socket + nonce   │
         │ JSONL v1, reverse RPC tool_call       │
┌────────▼─────────────────────────┐   ┌─────────▼──────────────────────┐
│ Python sidecar (세션당 1 프로세스) │   │ Chromium (데몬 소유 격리 프로필) │
│ openside_agent: Pydantic AI 런타임│   │ Stage A: 발견된 Chromium/Chrome │
│ + 선택한 pydantic-ai-harness 기능  │   │ Stage C: 패치 Chromium(browser/)│
│ 툴 = 데몬 툴 프록시(자체 툴 없음)  │   └────────────────────────────────┘
└────────┬─────────────────────────┘
         │ daemon-owned source pools; fresh private credential leases per request
┌────────▼──────────────────────────────────────────────────────────┐
│ omp-broker │ vault │ env │ command │ claude-code │ codex-cli │ builtin │
└───────────────────────────────────────────────────────────────────┘
```
사이드카 연동의 세부(핸드셰이크, 툴 경로, 티켓 재개, 복구, 계정 규칙)는 [04-sidecar-integration.md](04-sidecar-integration.md), 와이어 계약은 [specs/sidecar-protocol.md](specs/sidecar-protocol.md).

## 2. 코드 구성
### Rust 크레이트 (`crates/`)
| 크레이트 | 책임 | 요구사항 |
| --- | --- | --- |
| `openside-core` | ID 타입(`SessionId`, `TicketId` 등), 큐 상태 기계(steer/queue/edit/promote/remove), `AgentRuntime` trait 과 사이드카 계약 타입(`ToolSpec`, `ToolOutcome`, `TicketResolution`, `AgentState`, `RunOutcome`) | PP-10, PP-24 |
| `openside-policy` | 권한 모드(Read only/Guard/Full), Allow/Ask/Deny 규칙, Deny 우선, 레이어 평가 | D-1, D-2, D-4 |
| `openside-proto` | 공개 API v1 타입, 이벤트 봉투(`ticket.*`, `runtime.*`, `thinking.delta`, `usage` 등), 라우트 표 18개, OpenAPI 3.1 생성 | PP-8, PP-17 |
| `openside-sessions` | SQLite 이벤트 소싱 저장소(`events.db`), 재생·투영, 티켓 투영, `agent_state` 테이블 | PP-6, PP-10, PP-24 |
| `openside-config` | XDG/플랫폼 경로, `config.toml` 스키마와 환경변수 오버라이드, `agent.command` 해석, 브라우저 발견, `doctor`. 설정 문서는 `cargo run -q -p openside-config --example config-docs` 로 생성 | D-5, PP-13 |
| `openside-agent-sidecar` | `AgentRuntime` 의 사이드카 구현: 0700 디렉터리·0600 소켓·nonce, 프로세스 그룹, stderr 꼬리, 4 MiB 프레이밍, 툴 게이트, reverse RPC 상관 | ADR-002 |
| `opensided` (bin) | 데몬: API·SSE, 엔진(큐·dispatch·복구), 티켓·만료, 데몬 툴(파일·bash(bwrap)·브라우저·Vault·사람 단계), redaction, 출력 상한, 감사 JSONL | 전반 |
| `openside-cli` (bin `openside`) | 비대화형 API 클라이언트: 태스크 실행, `session list|resume|stop`, `ticket list|show|resolve|done|reject`, `routine list|show|add|run|runs|pause|resume|rm|suggest`, `doctor`, `--json`, 재개 커서 | E-1, PP-17 |
| `openside-vault` | 암호화 시크릿 저장, origin 검증, redaction, 감사 로그, 플랫폼 잠금 trait | C-1~C-6 |
| `openside-browser` | CDP 컨트롤러: 임시/격리 프로필, background 탭, 탭 장부, a11y 스냅샷, `act`/`insert_text`/`navigate`, detached 실행·재연결·탭 입양 | B-2~B-7 |
| `openside-sched` | 루틴 스케줄러: cron(POSIX 요일, IANA 시간대)·간격 스케줄, `routines.db`(루틴·실행 기록), 겹침 건너뛰기, 대상 세션 삭제 시 pause, 지금 실행, 재시작 복구(`missed`), 반복 요청 초안 제안. 데몬 부수효과는 `RoutineHost` trait 뒤에 둔다. 명세는 `docs/specs/routines.md` | A-10 |
| `openside-e2e` | 실제 프로세스 E2E: 실제 Python 사이드카 + 스크립트 모델(`OPENSIDE_AGENT_SCRIPT`)로 SIGKILL·재시작·사람 단계·정책 시나리오 | 품질 |

`crates/openside-agent-omp` 와 fake omp 바이너리는 삭제됐다(ADR-002).

### Python 사이드카 (`agent/`, uv 프로젝트, 패키지 `openside_agent`)
| 모듈 | 책임 |
| --- | --- |
| `__main__.py` | 진입점: 사이드카 모드, `doctor --json` |
| `protocol.py` | v1 메시지 타입, 4 MiB JSONL, 손상 입력 복구, `openside-agent-state/1` |
| `runtime/` | 세션 런타임: 런, steer 버퍼(S-8), 보류/재개, `stale_ticket`, abort, 상태 스냅샷(`state.py`) |
| `tools/` | 데몬 `ToolSpec` 을 `Tool.from_schema` 프록시로 노출, outcome → `ApprovalRequired`/`CallDeferred`/`ModelRetry` 매핑 |
| `capabilities.py` | 채택한 harness 기능(`SummarizingCompaction`, `PromptInjectionDefender`, `RepairToolArguments`, `WarnOnCacheBusts`, `SpendLimits(expose_tools=False)`) |
| `providers/` | 자격증명 소스(`sources.py`), broker 클라이언트(`broker.py`), 계정 선택·핀(`selector.py`, `pins.py`), 모델 조립(`models.py`), Anthropic 셰이핑(`anthropic.py`), `build_model`·`doctor_report` |
| `scripted.py` | 테스트용 스크립트 모델(프로토콜 §9) |

### 계획(아직 없음)
`openside-memory`(A-11), `openside-channels`(A-13), `openside-push`(F-2), MCP 서버(E-2), `openside-bench`(E-5). 비 Rust: `web/`(PWA), `extension/`(MV3). `browser/` 에는 Stage C 패치 Chromium 오버레이·빌드 스크립트가 있다.

## 3. 프로세스 모델
- 데몬 1개 + 활성 세션마다 warm 사이드카 1개. 프롬프트마다 프로세스를 새로 띄우지 않는다(PP-9). 사이드카는 다음 전달 때 지연 기동한다.
- 세션의 진실원천은 데몬의 이벤트 로그(append-only, SQLite)다. 사이드카 대화 상태는 런 종료 시 받은 불투명 스냅샷으로만 보관하고, 데몬은 해석하지 않는다.
- 메시지 주입은 런타임 I/O 전에 커밋한다(at-most-once). 사이드카·데몬 크래시 시 진행 중 메시지는 `failed/interrupted`, 큐 메시지는 보존된다.
- 사람 단계는 영속 티켓으로 보류되고 데몬 재시작 후에도 같은 tool call 로 재개한다. 열린 브라우저 티켓이 있으면 Chromium 을 데몬 재시작 너머로 유지하고 재연결한다.
- 데이터 디렉터리당 데몬 1개(`daemon.lock`).

## 4. 공개 API 표면
`openside_proto::ROUTES` 가 전체 표면이다(18개, OpenAPI 3.1 로 생성):
- `GET /v1/health`
- `POST|GET /v1/sessions`(생성 요청 `incognito: true` 면 브라우징 기록을 남기지 않는 비공개 세션, 생성 후 변경 불가; `docs/specs/memory.md` §7; `browser_space: "isolated"` 로 생성하면 공유 브라우저 안에서 자체 쿠키와 스토리지를 갖는 전용 브라우저 공간을 할당한다(`docs/09` §3 참고)), `GET /v1/sessions/{id}`, `POST /v1/sessions/{id}/messages`(`mode`: queue|steer|now), `PATCH|DELETE /v1/sessions/{id}/queue/{mid}`, `POST /v1/sessions/{id}/queue/{mid}/promote`, `POST /v1/sessions/{id}/queue/{mid}/move`, `POST /v1/sessions/{id}/stop`
- `GET /v1/sessions/{id}/events` (SSE, `Last-Event-ID` 재개)
- `GET /v1/tickets`, `GET /v1/tickets/{id}`, `POST /v1/tickets/{id}/resolve|done|reject`, `GET /v1/tickets/{id}/takeover`(WebSocket: 탭 JPEG 프레임 + 입력 전달, `docs/specs/human-step.md` §3)
- `POST /v1/devices/pair`, `DELETE /v1/devices/{id}`
- 능력 스코프: `read`, `steer`, `approve`, `human_step`, `admin`. unix socket 클라이언트는 관리자, TCP 는 loopback 전용 + bearer.

계획: WebSocket 멀티플렉스, 서브에이전트 API, 브라우저 탭/프레임 API, 설정·스킬·루틴·메모리 API, 계정/사용량 읽기 전용 API.

## 5. 이벤트 모델 (요지)
세션 이벤트는 불변 레코드이고 `session_id`, `seq`(세션 내 단조 증가), `ts`, `run_id` 를 가진다. 소비자는 `seq` 로 중복 제거·재개한다.
주요 종류: 메시지 큐/완료(`message.completed{outcome, reason}`), `turn.started|ended`, `assistant.delta|final`, `thinking.delta`, `usage`, `tool.started|ended`(데몬만 기록), `ticket.*`, `runtime.started|error`, `session.settled|stopped`. 정본은 `crates/openside-proto/src/events.rs`.
상태 기계(`openside-core`)는 순수 함수로 두고 속성 테스트로 불변식을 검증한다(PP-24).

## 6. 브라우저 스택: 3단계
브라우저 계층은 리스크가 가장 크므로 단계별로 나누어 구현한다(ADR-004).

| 단계 | 구성 | 달성 가능한 항목 | 한계 |
| --- | --- | --- | --- |
| **A. 스톡 Chromium + CDP** (구현) | 데몬이 발견한 Chromium/Chrome 을 데몬 소유 격리 프로필로 띄우고 CDP 로 제어. 툴은 데몬 `browser_*`(ADR-005). 확장 UI 는 계획 | B-2~B-4, B-7, B-5 근사 | 포커스 완전 차단·봇 탐지·브라우저 크롬 UI 변경 불가 |
| **B. (선택) 번들 배포** | Stage A 를 하나의 설치물로 패키징 | 설치 UX | 브라우저는 여전히 스톡 |
| **C. 패치 Chromium** (브라우저 트랙) | `browser/` 오버레이·패치, 핀 빌드(BR-0 완료) | B-5 완전, B-6, 세로 탭/분할/브라우저 크롬 UI | 빌드 비용·유지보수 큼(R-1) |

CEF 기반 자체 셸은 채택하지 않는다: 확장 생태계(비밀번호 관리자, PP-19)가 약해지기 때문(ADR-004).

### 브라우저 규율
- 에이전트 작업은 별도 창의 background 탭에서 한다. `Target.activateTarget`/`bringToFront` 는 쓰지 않는다.
- 숨겨진 탭 스크린샷은 포커스 에뮬레이션을 잠깐 켜고 항상 해제한다. 스크린샷은 0600 앱 파일이며 모델에는 크기만 간다. 비밀이 입력된 문서는 다른 문서로 이동할 때까지 스크린샷을 거부한다.
- unloaded/discarded 탭에는 attach 하지 않는다. attach 는 타임아웃 race.
- 탭 장부: 세션 소유권. 에이전트는 자기 세션이 연 탭만 다룬다.
- ref 는 탭 수명 동안 단조 증가하는 `eN`. 내비게이션 후 옛 ref 는 `StaleReference`.
- 결제·삭제류 대상(`browser_act` 의 접근성 이름/역할 휴리스틱)은 `Sensitive` 로 Full 모드에서도 항상 Ask.
- 타일링 WM 의 창 규칙(새 창 포커스 억제) 점검은 `openside doctor` 계획 항목이다.
- 관측 순서: url/title → a11y snapshot → extract → screenshot(시각 증명만).

## 7. 보안 모델
- 신뢰 경계: ① 모델 출력(비신뢰) ② 웹 페이지 내용(비신뢰, 프롬프트 인젝션 가능) ③ 클라이언트 토큰(스코프) ④ 로컬 사용자 ⑤ Python 사이드카(툴 실행권 없음, 비밀 수신 없음).
- 툴은 데몬만 실행한다. 사이드카는 `open.tools` 로 받은 프록시만 노출하고, 노출 목록이 데몬 목록과 다르면 세션을 거부한다(툴 게이트, ADR-015).
- Vault 입력값과 `secret_input`은 Rust 프로세스 메모리에서만 다루며, CDP 입력 직후 0으로 덮어써 완전히 파기(zeroize)한다. 사이드카나 이벤트 로그로 전달하지 않고 도구 결과에서도 마스킹하지만, 에이전트가 실행한 페이지 스크립트는 페이지 데이터를 직접 읽을 수 있다(ADR-022).
- 데몬이 모델 자격 증명 소스, 명시적 프로바이더 풀, 갱신, 계정 사용량 및 차단 상태를 관리한다. 사이드카는 외부 요청을 시도할 때마다 유효한 비공개 접근 리스만 받는다. 관리 대상 API 키와 OAuth 상태는 Vault에 암호화하여 정식 보관하며 기본 사용자 CLI 홈은 읽기 전용 상태를 유지한다. 소스가 없거나 모두 소진되면 목록에 없는 소스로 다운그레이드하지 않고 fail-closed 처리한다.
- redaction: 모든 공개 이벤트와 툴 결과에 Vault 패턴 redaction, 툴 인자는 툴별 `redact_args`(입력 텍스트는 길이만), 감사 JSONL 은 메타데이터만.
- 승인: 정책 `Ask` → `confirm` 티켓 → 런 보류. 승인된 티켓은 같은 세션·tool_call_id·툴 이름에만 유효. `Deny` 는 승인으로 뒤집히지 않는다. 만료는 거부.
- `bash` 는 bubblewrap(workspace 읽기·쓰기, 시스템 디렉터리 읽기 전용, 네트워크 unshare). 설정 실패 시 샌드박스 없이 실행하지 않는다.
- 데몬 바인딩은 unix socket(0700 부모, 0600) + 선택적 loopback TCP. 원격 노출은 사용자가 명시적으로.

## 8. 데이터 디렉터리
기본은 XDG 데이터 디렉터리 아래 `openside/`(0700, `daemon.data_dir`/`OPENSIDE_DATA_DIR` 로 변경): `events.db`(세션 이벤트·티켓·`agent_state`), `devices.db`, `audit.jsonl`, `vault-audit.jsonl`, `browser-profile/`, `browser-state.json`, `daemon.lock`, `opensided.port`(loopback TCP 포트), `policy.json`(`PUT /v1/policy` 로 저장한 정책, 0600), `routines.db`, `memory.db`, `memory/`, `skills/`, `channels/`, `artifacts/`(긴 툴 출력), `secret-fill-intents/`. SQLite 의 `-wal`/`-shm` 파일은 각 DB 에 딸린다. 소켓은 런타임 디렉터리(`opensided.sock`, 사이드카별 비공개 디렉터리). 설정은 XDG 설정 디렉터리의 `openside/config.toml`. 키·환경변수 전체는 [CONFIGURATION.md](CONFIGURATION.md).

`agents.db` stores the team registry, handoffs, transfer progress, and metadata-only audit, with SQLite `-wal`/`-shm` companions. Artifact directories are created on demand: observation screenshots remain model-only; `browser_screenshot output=true` creates a delivered PNG under `artifacts/`.

Connection metadata lives in `connections.db`; `connections-audit.jsonl` records metadata-only usage, and `connections.key` holds the private identity HMAC key. The complete storage manifest, including on-demand paths and atomic replacements, is [specs/data-layout.md](specs/data-layout.md).
