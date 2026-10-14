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
- `data`는 대화 기록, 해결되지 않은 보류 호출, steer 버퍼, 스크립트 커서를 담는다(`agent/src/openside_agent/runtime/state.py`). 자격 증명 어피니티는 데몬에만 남으며 사이드카가 직렬화하지 않는다. 이전 스냅샷에 `pins`가 남아 있더라도 해당 레거시 키를 무시하고 정상 디코딩한다.
- 재기동 시 `open.resume_state` 로 돌려준다. 런 도중 크래시는 그 런의 부분 히스토리를 잃는다.

## 8. 크래시·재시작 복구
| 상황 | 동작 | 근거 |
| --- | --- | --- |
| 사이드카 비정상 종료(SIGKILL, EOF) | 주입됐지만 결과 없는 메시지는 `message.completed{failed, reason:"interrupted"}`(at-most-once, 재전송 없음) + `runtime.error{interrupted}`. 큐에 남은 메시지는 보존되고 다시 dispatch 된다. 다음 전달 때 마지막 스냅샷으로 재기동 | `engine.rs` `interrupted()`, e2e `sidecar_sigkill_restores_snapshot_preserves_queue_and_completes_each_id_once` |
| `ready.pending_deferred` 중 데몬 쪽에서 이미 해결된 티켓 | 즉시 `ticket_resolved` 재전송, 사이드카가 중복을 걸러 1회만 반영 | 프로토콜 §8 |
| 데몬 재시작 | 이벤트 로그에서 세션·큐 복원, 진행 중 메시지는 `failed/interrupted`, 만료 검사. 열린 티켓은 유지되고 그 세션은 dispatch 하지 않음. 해결 시 같은 tool call 로 재개 | `engine.rs` `Daemon::open`, e2e `hs1_vault_login_secret_ticket_survives_daemon_restart_without_canary_leaks` |
| 데몬 재시작 + 브라우저 | 종료 시 열린 브라우저 티켓이 있으면 Chromium 을 남긴다. 기동 시 `browser-state.json` 의 엔드포인트로 재연결하고 탭 소유권·티켓 노드 핸들을 복원. 관리 프로필이 아니면 거부, 연결 실패면 상태 파일 삭제, 참조하는 티켓이 없으면 종료 | `tools/browser.rs` `recover()` |
| `auth_unavailable` | `runtime.error{auth_unavailable}` + `session.stopped`, no fallback to other credential sources. Background paths (restart recovery, routines, tickets) stay fail-closed while the latest `auth_unavailable` is newer than the latest `runtime.started`. An explicit send or Retry starts a fresh runtime, which asks for credentials again: a successful start clears the latch, a failed start answers `409 auth_unavailable` (web: "Check Settings → Models & accounts"). | protocol §3, `engine.rs` `auth_failed()` / `command_unlatched()` |

브라우저 유지 정책(항상 유지/항상 종료)은 사용자 결정 대기다. systemd 로 돌릴 때는 `KillMode=process` 를 권장한다.

## 9. Request credential ownership (A-8, PP-18)

데몬은 실제 프로바이더 풀, 소스 신뢰, 계정 리비전, 웜 세션 어피니티, 쿼터 예약, 범위 제한 블록, `refresh`의 유일한 소유자다. Python의 각 모델 요청 시도는 `credential_request`를 통해 새로운 비공개 리스를 발급받는다. 소스 설정, 브로커 스냅샷, CLI 홈 디렉터리, 리프레시 토큰은 진단 정보를 포함해 Python 쪽으로 일절 전달되지 않는다. 자세한 내용은 [`models-accounts.md`](specs/models-accounts.md) 및 [`sidecar-protocol.md` §6.1](specs/sidecar-protocol.md)을 참고한다.

`providers/credential_rpc.py`는 타입이 지정된 비공개 응답을 멀티플렉싱한다. `providers/request_model.py`는 논리적 요청 1회당 0부터 시작하는 시도 횟수를 최대 64회로 제한하며, 허용된 갱신/로테이션을 진행하기 전에 데몬이 처리 결과를 커밋하고 보낸 확인(ack)을 대기한다. `providers/request_auth.py`는 선택된 프로바이더 오리진으로 보낼 때만 리스된 자격 증명을 주입한다. SDK 클라이언트는 플레이스홀더 키만 보관하고 자체 자격 증명 재시도를 비활성화한다. Codex의 `openai_client=` 주입은 로컬 자격 증명 파일 로드와 OAuth 갱신을 차단한다.

결과(outcome) 종류는 `success`, `auth`, `rate_limit`, `quota`, `policy`, `transient`, `cancelled`다. 프로바이더가 알린 재시도/초기화 시각을 알 수 없으면 null 상태를 유지한다. 데몬이 `complete`, `refresh`, `rotate`, `stop` 중 하나를 결정하며, 사이드카 쪽에 별도 선택 로직은 없다. 이미 스트림을 전달하기 시작한 응답 스트림은 절대 다시 전송하지 않는다. 취소(cancellation) 요청에 한해서는 늦게 도착하는 확인을 기다리지 않고 즉시 리포트를 보낼 수 있어, 셧다운 디스패치 데드락을 방지한다.

기본 원칙은 닫힌 채로 실패한다(fail-closed). 목록에 없는 자격 증명 소스로의 다운그레이드는 금지된다. G-2 규칙에 따른 혼합 소스 사용은 사람이 명시한 실제 프로바이더 풀 내부에서만 허용되며, Agent 제약 사항도 그대로 유지된다. 프로바이더 간 변경은 명시적으로 구성된 폴백 모델이 있어야만 동작하며, `AuthUnavailable` 상태는 모델 폴백 대상에서 제외된다. 소진이나 거부가 발생하면 고정된 타입의 사유와 확인 가능한 신뢰할 만한 재시도 시각을 보존한다. 토큰, 업스트림 식별자, 원시 응답, 프로바이더 예외 본문은 오류, 리포트, 스냅샷, 공개 진단 어디에도 드러나지 않는다.

`models --json`은 자격 증명이 포함되지 않은 선별된 SDK 인덱스만 제공하며, 계정과 모델의 가용 상태 집계는 Rust가 담당한다. Python의 `accounts`와 `doctor`는 데몬이 정제해 전달한 메타데이터만 소비하며, 메타데이터가 없으면 설치/설정 전용 진단 도구가 임의로 로그인 준비 완료 상태를 판정할 수 없다. 결정론적 테스트는 모의 데몬/HTTP만 사용한다. 과거 스파이크 문서에 기록된 기존 라이브 호출 관측 결과는 새 계정 풀 경로를 검증하지 않는다.

## 10. 남은 것
- 서브에이전트(ADR-020): harness `SubAgents` 대신 사이드카 자체 어댑터(`runtime/subagents.py`)가 데몬 툴 `delegate` 결과로 서브에이전트를 돌린다. 서브에이전트의 승인·질문은 부모 세션 티켓(`origin.subagent_id`)으로 열리고, 부모 `delegate` 호출은 서브에이전트가 끝날 때까지 보류된다. 취소·steer·복구 규칙은 [`specs/sidecar-protocol.md` §10](specs/sidecar-protocol.md#10-서브에이전트).
- takeover 프레임 스트림(BLOCKED-2): `GET /v1/tickets/{id}/takeover` 는 `409 needs_desktop`.
