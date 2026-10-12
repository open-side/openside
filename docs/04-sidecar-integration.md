# 04. 에이전트 사이드카 통합 설계

작성: 2026-10-11. 이전 omp RPC 통합 설계를 대체한다.
와이어 계약의 정본은 [specs/sidecar-protocol.md](specs/sidecar-protocol.md)이고, 이 문서는 구조·실행 경로·복구·자격증명 규칙을 요약한다. 둘이 다르면 프로토콜 문서가 맞다.
구현: Rust `crates/openside-agent-sidecar`(프로세스·소켓·프레이밍·툴 게이트), `crates/opensided`(엔진·툴·티켓), Python `agent/src/openside_agent`(Pydantic AI 런타임·자격증명 소스).

## 1. 역할 분담
| 영역 | Python 사이드카(`openside_agent`) | Rust 데몬(`opensided`) |
| --- | --- | --- |
| 모델 호출 | Pydantic AI `Model`, `FallbackModel` 체인, 제공자별 요청 셰이핑 | 모델 ID 설정만 전달(`models.default`, `models.fallback`). 코드에 모델 ID 없음 |
| 자격증명 | 소스 플러그인(`omp-broker`, `env`, `command`, `codex-cli`, `builtin`)에서 직접 읽음 | `[credentials]` 절을 해석하지 않고 그대로 전달. 값은 읽지도 저장하지도 않음 |
| 에이전트 루프 | 런, steer 주입, 보류/재개, abort, 대화 상태 직렬화 | 영속 큐, 세션 이벤트 로그(진실원천), 상태 스냅샷 보관 |
| 툴 | 데몬이 준 `ToolSpec` 으로 만든 얇은 프록시만 노출 | 모든 툴의 실행·정책·승인 티켓·redaction·감사 |
| harness capability | `SummarizingCompaction`, `PromptInjectionDefender`, `RepairToolArguments`, `WarnOnCacheBusts`, `SpendLimits(expose_tools=False)` (모두 툴 0개 노출) | — |

## 2. 프로세스 모델
- 활성 세션마다 warm 사이드카 1개. 데몬이 설정 `agent.command`(argv 배열, 환경변수 `OPENSIDE_AGENT_COMMAND` JSON 으로 덮어씀)를 실행하고 프로세스 그룹으로 관리한다. 미설정이면 `openside-config` 가 `uv run --project <agent.dir> python -m openside_agent` 를 만들고, uv 나 `agent.dir` 이 없으면 `python3 -m openside_agent` 후보를 쓴다(`discovery.rs` `agent_command`).
- 소켓: 런타임 디렉터리 아래 세션별 비공개 디렉터리(0700)에 `rpc.sock` 을 만들고 0600 으로 바꾼 뒤 리슨한다. 자식 환경에 `OPENSIDE_SIDECAR_SOCKET`, `OPENSIDE_SIDECAR_NONCE`(무작위 32 hex, 자격증명 아님)를 넣는다. stdin 닫음, stdout 버림, stderr 는 줄당 2 KiB·최근 64줄 진단 버퍼.
- 핸드셰이크: 사이드카 `hello{protocol:1, nonce, pid, versions}` → 데몬 `open{session, model, system_prompt_parts, tools, resume_state?, config{credentials, agent}}` → 사이드카 `ready{tools, pending_deferred, startup_ms}`. 첫 접속 1개만 받고 nonce 가 다르면 종료. 접속 후 소켓 파일은 지운다.
- 툴 게이트: `ready.tools` 집합이 `open.tools` 이름 집합과 정확히 같지 않으면 `tool_gate`(fatal)로 세션 시작을 거부한다.
- 기동 시간: `agent.startup_timeout_ms`(기본 30000) 안에 spawn·hello·ready 가 끝나지 않으면 프로세스를 죽이고 `Unavailable`. 측정값 `startup_ms` 는 `runtime.started` 이벤트에 기록된다. 실제 환경의 기동 시간 수치는 측정 기록 없음(`unverified`).
- 종료: 데몬 `shutdown` → 사이드카가 진행 중 런 취소 → 마지막 `state_snapshot` → `bye`. 5초 후 프로세스 그룹 SIGKILL.

## 3. 프로토콜 요약
- 프레이밍: UTF-8 JSON 한 줄(`\n`), 최대 4 MiB(`MAX_LINE_BYTES = 4194304`), 봉투 `{"v":1,"type":…}`.
- 손상 입력(너무 긴 줄, JSON 아님, 모르는 `type`, 필수 필드 누락)은 그 줄만 버리고 `error{code:"protocol", fatal:false}`. 프로세스는 죽지 않는다.
- `error.code`: `protocol`, `bad_request`, `stale_ticket`, `model_error`, `auth_unavailable`(fatal), `tool_gate`(fatal), `internal`.
- 관측 이벤트(`event.kind`): `turn.*`, `assistant.delta|final`, `thinking.delta`, `usage`. 툴 생명주기(`tool.started/ended`)는 데몬만 기록한다(D6).
- 데몬은 어떤 비밀값(모델 자격 증명, Vault 값, `secret_input`, 기능 토큰)도 프로토콜 메시지에 담지 않는다. 단, 에이전트가 실행한 페이지 스크립트의 반환값은 도구 결과로 전달되므로 내부 비밀값은 redaction만으로 보호된다(ADR-022).
- 테스트용 스크립트 모델: `OPENSIDE_AGENT_SCRIPT=<JSON 경로>` 이면 자격증명 소스를 열지 않고 `FunctionModel` 로 응답을 재생한다(프로토콜 §9, `crates/openside-e2e`).

## 4. 툴 실행 경로
툴은 데몬에만 있다(`request_human_step`, `ask_user`, `read`, `write`, `edit`, `bash`, `browser_open_tab|tabs|snapshot|navigate|act|screenshot|close_tab`, `vault_login`, `vault_otp`). 사이드카 프록시는 데몬 `schemars` 스키마를 그대로 `Tool.from_schema` 로 감싼다.

```
model tool call
  → 사이드카 tool_call{rpc_id, tool_call_id, name, args, approval_ticket?}   (reverse RPC, 동시 다수 가능)
  → 데몬: 가시성·인자 검증 → 정책 판정(Allow/Ask/Deny, Deny 우선)
        Deny  → denied (승인으로도 못 뒤집음)
        Ask   → confirm 티켓 생성 → deferred{kind:"approval"}
        Allow / 승인된 티켓 → 실행
  → redaction(Vault 패턴) → 모델 출력 32 KiB 상한 → tool.started/ended 이벤트 → 0600 메타데이터 감사 JSONL
  → tool_result{rpc_id, outcome}
```
- `outcome`: `done{for_model}`, `deferred{ticket_id, kind:"approval"|"external"}`, `denied{reason, hint}`, `error{message}`. denied/error 는 사이드카에서 `ModelRetry`(기본 재시도 3회, `agent.tool_retries`).
- 승인 후 재실행: 사이드카가 같은 `tool_call_id` 로 `approval_ticket` 을 붙여 다시 호출한다. 데몬은 그 티켓이 같은 세션·같은 `tool_call_id`·같은 툴 이름·`confirm`·resolved(approved) 일 때만 Ask 를 건너뛴다. 그 외에는 `denied`(위조 승인 차단, `crates/opensided/src/host.rs`).
- Read only 모드는 read 등급이 아닌 툴을 스펙 목록에서 뺀다. `sandbox.required = true` 인데 bwrap 이 없으면 `bash` 를 숨긴다. Vault 가 잠겨 있으면 Vault 툴을 숨긴다. 노출 목록이 바뀌어도 §2 의 툴 게이트는 데몬 목록과 사이드카 노출 목록이 같은지만 본다.

## 5. 보류(티켓)와 재개
- 사람 단계는 항상 보류다(D2). `request_human_step`, `ask_user` → `deferred{kind:"external"}` → 사이드카 `CallDeferred`. 정책 Ask → `deferred{kind:"approval"}` → `ApprovalRequired`. 런은 `deferred{calls}` → `message_result{deferred}` 로 끝난다.
- 티켓은 데몬이 영속으로 소유한다(`/v1/tickets`, `ticket.*` 이벤트). 프롬프트 리소스는 없다(D1). 종류별 TTL 은 `[tickets]`, 만료는 1초마다와 데몬 기동 시 검사.
- 열린 티켓이 있으면 세션은 `waiting_human` 이고 큐 메시지를 사이드카로 보내지 않는다(D4). stop 은 열린 티켓을 `rejected(stopped)` 로 닫는다.
- 해결: 데몬 `ticket_resolved{ticket_id, tool_call_id, resolution}`. 사이드카는 멈춘 런의 보류 호출이 모두 해결될 때까지 모았다가 같은 `tool_call_id` 로 `DeferredToolResults` 재개 런을 시작한다. 이미 반영된 호출은 `stale_ticket` 후 무시하므로 재개는 최대 1회.
- `secret_input` 값은 데몬의 SecretSink(브라우저, 정확한 origin + 문서별 노드 핸들)로만 가고 zeroize 된다. 모델이 받는 결과는 `{"status":"filled"}`. 설계 배경은 [specs/human-step.md](specs/human-step.md).

## 6. steer·abort
- steer(S-8, [spikes/S-8-steer.md](spikes/S-8-steer.md)): Pydantic AI 네이티브 `enqueue` 는 쓰지 않는다(`DeferredToolRequests` 종료를 덮어씀). `steer`/`now` 메시지는 사이드카 버퍼에 두고 `before_model_request` 훅에서 다음 모델 요청 직전에 주입한다. 주입 시 `message_accepted`. 런이 끝날 때 남은 것은 새 런으로, 보류로 끝났으면 재개 런에서 주입한다. `queue` 메시지는 현재 런이 끝난 뒤 처리한다.
- 큐 소유는 데몬(`MessageQueue`)이다. 편집·삭제·승격(`/v1/sessions/{id}/queue/{mid}`)은 사이드카로 보내기 전 단계에서만 일어난다.
- abort: `abort` → `run.cancel()`. 유휴면 무시. 세션 stop 은 abort 와 열린 티켓 거부를 함께 한다.

## 7. 상태 스냅샷
- 사이드카는 런 종료(completed/aborted/failed/deferred)마다 `state_snapshot{format:"openside-agent-state/1", data}` 를 보낸다(D3). 데몬은 내용을 해석하지 않고 세션별 최신 1개만 `agent_state` 에 보관한다.
- `data` 에는 대화 히스토리, 미해결 보류 호출, steer 버퍼, 계정 핀(§9), 스크립트 커서가 들어간다(`agent/src/openside_agent/runtime/state.py`). 토큰·계정 식별자는 없다.
- 재기동 시 `open.resume_state` 로 돌려준다. 런 도중 크래시는 그 런의 부분 히스토리를 잃는다.

## 8. 크래시·재시작 복구
| 상황 | 동작 | 근거 |
| --- | --- | --- |
| 사이드카 비정상 종료(SIGKILL, EOF) | 주입됐지만 결과 없는 메시지는 `message.completed{failed, reason:"interrupted"}`(at-most-once, 재전송 없음) + `runtime.error{interrupted}`. 큐에 남은 메시지는 보존되고 다시 dispatch 된다. 다음 전달 때 마지막 스냅샷으로 재기동 | `engine.rs` `interrupted()`, e2e `sidecar_sigkill_restores_snapshot_preserves_queue_and_completes_each_id_once` |
| `ready.pending_deferred` 중 데몬 쪽에서 이미 해결된 티켓 | 즉시 `ticket_resolved` 재전송, 사이드카가 중복을 걸러 1회만 반영 | 프로토콜 §8 |
| 데몬 재시작 | 이벤트 로그에서 세션·큐 복원, 진행 중 메시지는 `failed/interrupted`, 만료 검사. 열린 티켓은 유지되고 그 세션은 dispatch 하지 않음. 해결 시 같은 tool call 로 재개 | `engine.rs` `Daemon::open`, e2e `hs1_vault_login_secret_ticket_survives_daemon_restart_without_canary_leaks` |
| 데몬 재시작 + 브라우저 | 종료 시 열린 브라우저 티켓이 있으면 Chromium 을 남긴다. 기동 시 `browser-state.json` 의 엔드포인트로 재연결하고 탭 소유권·티켓 노드 핸들을 복원. 관리 프로필이 아니면 거부, 연결 실패면 상태 파일 삭제, 참조하는 티켓이 없으면 종료 | `tools/browser.rs` `recover()` |
| `auth_unavailable` | `runtime.error{auth_unavailable}` + `session.stopped`. 세션은 영구 중지(fail-closed), 다른 자격증명으로 폴백하지 않음 | 프로토콜 §3, `engine.rs` `fail()` |

브라우저 유지 정책(항상 유지/항상 종료)은 사용자 결정 대기다. systemd 로 돌릴 때는 `KillMode=process` 를 권장한다.

## 9. 자격증명과 계정 규칙 (A-8, PP-18)
### 9.1 소스와 순서
- 모델 ID 는 설정값 `provider:model` 이다(D5, `models.default` 미설정이면 세션 생성이 `model_not_configured`).
- `build_model` 이 `credentials.order.<provider>` 순서로 소스를 시도한다. 기본 순서(`agent/src/openside_agent/providers/config.py`): `anthropic` = `omp-broker` → `env` → `command`, `openai-codex` = `omp-broker` → `codex-cli`, 그 외 = `env` → `command`. `builtin` 은 순서에 명시했을 때만 쓴다.
- 소스별 설정 키와 기본값은 [CONFIGURATION.md](CONFIGURATION.md)와 [agent/README.md](../agent/README.md)를 본다. 이 문서는 규칙만 다룬다.

### 9.2 fail-closed (P-1)
1. openside 는 제공자 로그인 UI·자격증명 저장소를 만들지 않는다. 계정 추가·OAuth 갱신은 소스 쪽(예: omp auth broker)이 한다.
2. 토큰·스냅샷·자격증명 행은 로그/API/UI/에러 메시지/stderr 에 절대 출력하지 않는다. 오류 문장은 정제된 이유만 담는다(`AuthUnavailable.reasons`).
3. broker 소스가 선택됐는데(토큰이 있음) broker 에 닿지 못하거나 해당 제공자 계정이 전부 차단이면 `AuthUnavailable` 이다. `env`/`command` 로 강등하지 않는다. broker 스냅샷에 그 제공자가 아예 없을 때만 빌드 시점에 다음 소스로 넘어간다.
4. `AuthUnavailable` 은 `FallbackModel` 폴백 조건이 아니다. 실행 중 broker 가 끊겨도 런 도중 로컬 소스로 바뀌지 않는다.
5. broker 토큰이 없으면 broker 소스는 조용히 비활성(다음 소스)이다. 이는 "broker 를 설정하지 않음"이지 강등이 아니다.

### 9.3 broker 소스의 계정 선택과 핀
구현: `agent/src/openside_agent/providers/selector.py`, `pins.py`, `models.py`.
1. 후보: 스냅샷의 OAuth 자격증명 중 제공자가 일치하고, broker `blocks` 가 만료되지 않았고, 이 세션이 로컬로 차단하지 않은 것. 후보가 없으면 `AuthUnavailable("no unblocked OAuth account")`.
2. 세션 핀 유지: `CredentialPins` 는 세션·제공자별로 `credential_id` 와 마지막 요청 시각을 기록하고, 핀이 후보에 남아 있으면 그 계정을 계속 쓴다(prompt cache 보호). Anthropic 핀은 마지막 요청 후 1시간(3600초)이 지나면 해제한다.
3. 핀이 없거나 무효면 새로 고른다: `GET /v1/usage` 로 계정별 잔여 비율의 최솟값이 가장 큰 계정 → 만료가 늦은 계정 → 작은 id. 사용량 응답은 참고용이며 실패해도 스냅샷이 기준이다.
4. 429 또는 `rate_limit` 응답: `retry-after`(1~86400초, 기본 60) 동안 그 계정을 로컬 차단하고 핀을 해제한 뒤 `POST /v1/credential/:id/block` 을 보낸다. 스냅샷을 강제로 다시 읽어 다른 계정으로 같은 요청을 재시도하고, 이미 시도한 계정만 남으면 `AuthUnavailable("all attempted accounts rate limited")`.
5. 토큰 갱신은 broker 만 한다. 만료 30초 이내면 스냅샷을 강제로 다시 읽고, 그래도 만료 임박이면 fail-closed. 401 이면 스냅샷을 다시 읽어 다른 자격증명으로 1회만 재시도하고, 같은 자격증명이거나 만료 임박이면 fail-closed. Codex 는 `openai_client=` 주입으로 provider 의 자체 OAuth refresh 경로를 끈다(S-16, [spikes/S-16-provider-refresh.md](spikes/S-16-provider-refresh.md)).
6. 핀은 스냅샷(`openside-agent-state/1`)에 같이 저장돼 사이드카 재기동 후에도 같은 계정을 쓴다. 직렬화 형식은 `{session:{provider:{credential_id, last_request_at}}}` 뿐이며 토큰·계정 ID·이메일은 넣지 않는다(`CredentialPins.from_json` 이 다른 키를 거부).
7. 계정 풀·우선순위·reserve 같은 정책은 broker 소유다. openside 는 계정을 추가·삭제·비활성화하지 않고, `block` 외의 쓰기를 하지 않는다. 풀은 라우팅 정책이지 인가 경계가 아니다. 진짜 격리가 필요하면 broker 를 따로 둔다.
8. broker 접속 정보는 머신 로컬 설정으로 둔다: URL 은 `OPENSIDE_BROKER_URL` 또는 `credentials.omp_broker.url`, 토큰은 `OPENSIDE_BROKER_TOKEN`(값) 또는 `credentials.omp_broker.token_file`(경로). 동기화되는 설정 파일에 토큰 값을 쓰지 않는다. 같은 키는 [CONFIGURATION.md](CONFIGURATION.md) 참고.

### 9.4 진단과 검증 상태
- `openside doctor [--json]` 는 사이드카 `doctor --json` 으로 소스별 `provider`, `source`, `available`, `reason`, `next_step` 을 보고한다. broker 는 읽기 전용 GET 만 하고 모델 요청을 보내지 않는다. `command` 는 실행하지 않고 실행 가능 여부만 본다(`unverified`).
- 결정론 테스트는 가짜 broker 와 `MockTransport` 만 쓴다(`agent/tests/providers`). 실제 broker 로는 Codex·Claude 한 줄 응답과 `ask_user` 보류→해결→재개 왕복을 확인했다([spikes/S-14-broker-provider.md](spikes/S-14-broker-provider.md)). 실제 broker 에서의 429 계정 이동, 다계정 핀 동작은 `unverified`.
- Anthropic 구독 셰이핑(`providers/anthropic.py`)은 제공자 정책이 바뀌면 깨질 수 있다(R-13).

## 10. 남은 것
- 서브에이전트(ADR-020): harness `SubAgents` 대신 사이드카 자체 어댑터(`runtime/subagents.py`)가 데몬 툴 `delegate` 결과로 서브에이전트를 돌린다. 서브에이전트의 승인·질문은 부모 세션 티켓(`origin.subagent_id`)으로 열리고, 부모 `delegate` 호출은 서브에이전트가 끝날 때까지 보류된다. 취소·steer·복구 규칙은 [`specs/sidecar-protocol.md` §10](specs/sidecar-protocol.md#10-서브에이전트).
- takeover 프레임 스트림(BLOCKED-2): `GET /v1/tickets/{id}/takeover` 는 `409 needs_desktop`.
