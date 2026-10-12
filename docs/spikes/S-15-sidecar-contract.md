# S-15 sidecar contract inventory

> 이 문서에 나오는 `spikes/` 아래 임시 스크립트와 내부 개발 기록(`docs/GOAL.md`, `docs/PROGRESS.md`, `docs/archive/`)은 공개 저장소에 포함되어 있지 않으며, 실행 명령과 출력 결과는 작업 과정을 확인하는 기록용으로 남겨 둔다.

작성: 2026-10-11. 이 문서는 현행 Rust 워크스페이스의 `omp` 결합 지점을 전수 조사하여 확정된 사이드카 계약(`docs/specs/sidecar-protocol.md`)으로 전환하기 위한 소스 인벤토리다.

## 1. 범위와 설계 기준

- **근거 문서**: `docs/specs/sidecar-protocol.md`.
- **확정된 사이드카 계약**:
  - 데몬(`opensided`)이 세션당 Python 프로세스 하나를 띄우고 비공개 유닉스 소켓의 줄 단위 JSONL로 통신한다.
  - 메시지 큐(`MessageQueue`)는 데몬이 소유하며, 툴도 데몬이 직접 실행한다.
  - 데몬이 전달한 `ToolSpec` JSON Schema만 사이드카에 노출한다. 사이드카의 툴 호출은 reverse RPC(`tool_call` / `tool_result`)로 처리하며, 결과는 `done` | `deferred` | `denied`/`error`다.
  - 보류 티켓 해결(`ticket_resolved`)은 같은 `tool_call_id`를 최대 한 번 재개한다.
  - 사이드카는 런 종료 시점마다 `state_snapshot`을 보내고, 데몬은 이를 불투명 데이터로 저장했다가 재기동 시 `resume_state`로 돌려준다.
  - `user_message` 모드는 `queue` | `steer` | `now`; `abort`는 런 취소, `settled`는 처리할 런과 메시지가 없는 상태를 알린다.
  - `omp` 확장 UI 대화상자와 런타임 큐 동기화는 제거한다.

## 2. `openside-core::runtime` 계약 결정

근거: `crates/openside-core/src/runtime.rs:13-316`, `docs/specs/sidecar-protocol.md:28-37,50-53`.

| 항목 | 조치 | 사이드카 대응 |
| --- | --- | --- |
| `SessionSpec.resume`, `SessionSpec.model` | KEEP | `open.resume_state`, `open.model`에 대응한다. |
| `SessionSpec.cwd` | CHANGE | `open`에 명시적 `cwd` 필드는 없다. `open.config.agent`를 통한 전달은 `[INFERENCE]`이며 매핑을 확정해야 한다. |
| `SessionSpec.profile`, `SessionSpec.approval_mode`, `SessionSpec.session_dir`, `SessionSpec.thinking` | DELETE | `omp` 전용 세션 옵션이다. 승인 판단은 데몬 정책이 맡는다. |
| `ApprovalMode`, `ApprovalMode::as_str` | DELETE | 데몬 정책으로 대체한다. |
| `PromptKind`, `AskAnswer`, `PromptAnswer` | DELETE | `omp` 확장 UI 응답 모델이다. |
| `HostToolDefinition`, `HostToolCall`, `HostToolResult`, `HostToolHandler` | CHANGE | 데몬이 전달하는 동적 `ToolSpec`, `rpc_id`/`tool_call_id`가 있는 reverse RPC, 결과 상태 `done`/`deferred`/`denied`/`error`로 바꾼다. |
| `RuntimeError` | CHANGE | `Unavailable`, `Rejected`, `AuthUnavailable` 분류는 필요하지만 `omp` 의존 설명·가정을 제거하고 §3 오류 코드에 매핑한다. |
| `RuntimeEvent::AssistantDelta`, `AssistantFinal`, `TurnStarted`, `TurnEnded`, `SubagentLifecycle`, `SubagentProgress`, `SubagentEvent`, `Error` | KEEP / CHANGE | 프로토콜 `event`/`error`에 매핑한다. `thinking.delta`와 `usage` 수신도 추가한다. |
| `RuntimeEvent::ToolStarted`, `ToolUpdated`, `ToolEnded` | CHANGE | 사이드카는 툴 생명주기 이벤트를 보내지 않는다. `docs/specs/sidecar-protocol.md` §5.1대로 데몬이 직접 기록한다. |
| `RuntimeEvent::PromptOpened`, `PromptResult`, `QueueUpdated`, `PromptClosed` | DELETE | 각각 티켓 이벤트, `message_result`, 데몬 소유 큐, 티켓 만료 흐름으로 대체한다. |
| `RuntimeEvent::Settled` | KEEP | 사이드카 `settled`에 대응한다. |
| `RuntimeEvent::MessageAccepted` | CHANGE | `message_accepted` 및 `message_result`에 맞게 조정하고 `state_snapshot` 수신·저장을 추가한다. |
| `AgentSession::send` | CHANGE | `user_message`와 모드 `queue`/`steer`/`now` 전송으로 바꾼다. |
| `AgentSession::abort`, `next_event`, `shutdown` | KEEP / CHANGE | 각각 `abort`, 이벤트 수신, `shutdown`/마지막 `state_snapshot`/`bye` 절차에 대응한다. |
| `AgentSession::answer_prompt`, `remove_queued_message`, `promote_queued_message` | DELETE | `ticket_resolved`로 티켓을 재개하고 큐는 데몬에서만 조작한다. |
| `AgentRuntime`, `AgentRuntime::open_session` | KEEP / CHANGE | 트레이트 경계는 유지하고 `openside-agent-sidecar` 구현 및 개정된 세션 계약으로 바꾼다. |

## 3. `opensided` 결합 지점

- `crates/opensided/src/config.rs:4-5,20-34,54-67`: `RpcMode`, `ApprovalMode` 임포트와 `omp_program`, `omp_arg`, `approval_mode`, `rpc_mode` 플래그·파서. 제거하고 설정의 `agent.command` 및 데몬 정책 설정으로 교체한다.
- `crates/opensided/src/engine.rs:8-15,31-32,56-57,125-150,320-321,368-373,405-409,423-428,434-473,555-647`: Omp 타입 임포트, 재기동 시 미결 프롬프트 만료, `OmpSession` 보관, 런타임 큐 승격·삭제 및 `answer_prompt`, `OmpConfig` 생성, `omp` 이벤트 매핑. 데몬 소유 큐·이벤트·디스패치는 KEEP; 런타임 큐 변경 호출은 DELETE; 세션 생성과 이벤트 매핑은 사이드카 프로토콜로 CHANGE한다.
- `crates/opensided/src/host.rs:4-12,28-30,43-55,93-149,182-256,259-295`: `request_approval` 호스트 툴, 구형 프롬프트 생성·확장 UI 어댑터, 타임아웃 만료와 `answer_prompt`. `request_approval` 및 확장 UI 경로는 DELETE하고 정책 `Ask`를 티켓/`deferred` 툴 흐름으로 바꾼다.
- `crates/opensided/src/api.rs:68-69,249-326`: `GET /v1/prompts`, `POST /v1/prompts/{id}/answer`. 티켓 API로 교체한다.
- `crates/opensided/src/lib.rs:10,31-35`: 프롬프트 라우트·승인 동작 설명을 티켓/사이드카 계약으로 갱신한다.
- `crates/opensided/src/tests.rs:56-80,228-270,297-347,349-378`: 프롬프트 생성, 호스트 승인, 정책 만료·거부 테스트를 티켓·deferred·재개 테스트로 바꾼다.

## 4. `openside-proto` 및 `openside-sessions`

- `crates/openside-proto/src/api.rs:21-31,65-73`: `ApprovalMode` 및 `Settings.approval_mode`는 omp 승인 방식이다. 삭제하고 `PolicyMode`는 유지한다.
- `crates/openside-proto/src/api.rs:183-260`: `PromptKind`, `PromptStatus`, `PromptOrigin`, `PromptOption`, `ApprovalTool`, `Prompt`, `PromptList` 및 요청 타입은 구형 프롬프트 API다. 제거하고 티켓 타입으로 교체한다.
- `crates/openside-proto/src/events.rs:9-19`: `MessageOutcome::Completed` 설명의 “omp reported”를 런타임/사이드카 완료 의미로 수정한다. 열거형 의미는 유지한다.
- `crates/openside-proto/src/events.rs:137-155`: `PromptOpened`, `PromptAnswered`, `PromptExpired`를 `ticket.opened`, `ticket.resolved`, `ticket.rejected`, `ticket.expired` 이벤트 계약으로 교체한다.
- `crates/openside-proto/src/openapi.rs:143-158,225-250`: 프롬프트 라우트와 스키마를 티켓 라우트·스키마로 교체한다.
- `crates/openside-proto/src/tests.rs:6-26,43-92`: 프롬프트 스키마·이벤트 직렬화 테스트를 티켓 계약에 맞춘다.
- `crates/openside-sessions/src/state.rs:13-33,118-139`: 프롬프트 맵 저장과 생명주기 검증을 티켓·`tool_call_id`·보류 호출 상태 및 최신 `state_snapshot` 보관으로 교체한다. 데몬 소유 세션 큐는 유지한다.
- `crates/openside-sessions/src/lib.rs:23-26`: 리플레이 문서의 프롬프트 생명주기 설명을 티켓 기준으로 바꾼다.

## 5. E2E·CLI·워크스페이스

- `Cargo.toml:3-14`: 워크스페이스 멤버 `crates/openside-agent-omp`를 제거하고 `crates/openside-agent-sidecar`를 추가한다.
- `crates/opensided/Cargo.toml:25-26`: `openside-agent-omp` 의존성을 사이드카 어댑터 의존성으로 교체한다.
- `crates/openside-e2e/Cargo.toml:3,11-12,25-27`: `fake-omp` 설명·의존성·바이너리 타깃을 삭제하고 새 사이드카 E2E 구성을 넣는다.
- `crates/openside-e2e/src/lib.rs:3-5,59-75`: fake 프로세스 그룹과 `--approval-mode`, `--omp-program`, `--omp-arg` 실행 인자를 제거하고 사이드카 프로세스를 띄운다.
- `crates/openside-e2e/src/bin/fake-omp.rs:1,12-17`: `openside_agent_omp::fake` 래퍼 바이너리 전체를 삭제한다.
- `crates/openside-e2e/tests/cli.rs:1`, `crates/openside-e2e/tests/daemon.rs:13-14,158-211,397-400`: fake-omp 하니스와 `request_approval` 프롬프트·만료 검증을 사이드카/`FunctionModel` 스크립트(`OPENSIDE_AGENT_SCRIPT`) 및 티켓 검증으로 교체한다.
- `crates/openside-cli/src/lib.rs:4-6,33-35,88-90,113-116,330-363`, `crates/openside-cli/src/client.rs:102-108,258-260`, `crates/openside-cli/src/tests.rs:140-145,285-290,408-429`: `prompt list`/`prompt answer`, `/v1/prompts` 경로, 관련 테스트를 티켓 조회·해결 명령과 API로 바꾼다. **CLI 명령 이름은 미확정**이며 `ticket list`/`ticket resolve`는 `[INFERENCE]`다.

## 6. `openside-agent-omp` 크레이트 전체

크레이트·테스트·워크스페이스 항목을 호환 shim 없이 삭제하고 `openside-agent-sidecar`로 교체한다.

| 파일 | 역할 | 조치 |
| --- | --- | --- |
| `crates/openside-agent-omp/src/lib.rs:1-19,31-32,79-132,169-188` | `OmpRuntime`/`OmpSession`, RPC 자식 프로세스 수명, 호스트 툴 등록·세션 실행 | DELETE |
| `crates/openside-agent-omp/src/wire.rs:1-73` | omp RPC 직렬화 타입, 프레임·청크 디코더와 명령 모델 | DELETE |
| `crates/openside-agent-omp/src/normalize.rs:1-49,52-65` | RPC 오류 분류와 입력 프레임→`RuntimeEvent` 정규화 | DELETE |
| `crates/openside-agent-omp/src/fake.rs:1-32` | 스크립트형 JSONL fake 피어, 큐 명령 및 `prompt_result`/`session_settled` 생성 | DELETE |
| `crates/openside-agent-omp/src/bin/fake-omp.rs:1` | fake-omp 실행 바이너리 | DELETE |
| `crates/openside-agent-omp/tests/runtime.rs:102-103,423-429` | fake 프로세스 런타임 통합 테스트와 `#[ignore]` 실제 omp 스모크 | DELETE |

## 7. 이식성 스캔

실행 명령:
```sh
grep -rnE "/home/|/Users/|sharosoo|workspaces/|\.omp/|Hyprland|127\.0\.0\.1:8765|/usr/bin/chromium" crates agent/src browser tools docs -I
```

원시 명령 출력 요약은 49 matches in 17 files다. 요청된 제외 경로(`docs/archive/`, `docs/research/`, `docs/spikes/`, `docs/PROGRESS.md`, `docs/FINAL-REPORT.md`, `tools/dev-only/`)를 빼고 확인한 주요 파일·행은 다음과 같다.

- `crates/openside-browser/src/browser.rs:24,37`: `/usr/bin/chromium` 기본 실행 경로.
- `browser/BUILD.md:7,28,36-37,65,85,109,164,166`: 개인 빌드 워크스페이스, 로그, 패치 절대 경로.
- `browser/specs/focus-arbiter.md:62,66,70`: `Hyprland` 환경·검증 언급(일부는 선택적 환경 요구).
- `docs/01-parity-spec.md:50`: `Hyprland`와 omp relay 언급.
- `docs/03-architecture.md:90`: `Hyprland` 창 규칙 안내.
- `docs/04-omp-integration.md:45,51-54,67`: `~/.omp`, `OMP_*`, omp 설정·프로필과 사용자 인프라 경로.
- `docs/05-roadmap.md:31,71`: warm omp 및 `Hyprland` 언급.
- `docs/08-pydantic-ai-pivot.md:66,83`: `127.0.0.1:8765`, `~/.omp/auth-broker.token`.
- `docs/GOAL.md:7,14,16-18,23-28,38-40`: 브로커 후보·경로 및 머신 제약의 명시적 설명도 검색 패턴에 걸린다.
- `tools/goal-run/run_goal.py:43,48`: 개인 빌드 워크스페이스 경로.
- `docs/specs/acceptance.csv:67`: `Hyprland` 검증 언급.
- `agent/src`: 검색 결과 없음.

**미검증(unverified)**: 원시 검색의 일치 파일에는 제외 경로도 포함된다. 출력 일부가 생략되어 제외 경로를 뺀 매칭 건수와 파일별 수치는 미검증이다.

## 8. Rust omp 키워드 grep 집계

실행 명령:
```sh
grep -rniE "\bomp\b|oh-my-pi|rpc-ui|queue_update|prompt_result|session_settled" crates --include='*.rs' -c
```

`-c` 출력의 파일별 수치이며, 합계는 16개 비영(非零) 파일에서 106개 매칭 행이다.

| 파일 | 행 수 | 파일 | 행 수 |
| --- | ---: | --- | ---: |
| `crates/openside-agent-omp/src/bin/fake-omp.rs` | 1 | `crates/openside-core/src/queue.rs` | 1 |
| `crates/openside-agent-omp/src/fake.rs` | 13 | `crates/openside-core/src/runtime.rs` | 18 |
| `crates/openside-agent-omp/src/lib.rs` | 33 | `crates/openside-e2e/src/lib.rs` | 4 |
| `crates/openside-agent-omp/src/normalize.rs` | 8 | `crates/openside-e2e/tests/cli.rs` | 2 |
| `crates/openside-agent-omp/src/wire.rs` | 2 | `crates/openside-e2e/tests/daemon.rs` | 1 |
| `crates/openside-agent-omp/tests/runtime.rs` | 8 | `crates/openside-proto/src/api.rs` | 3 |
| `crates/opensided/src/config.rs` | 6 | `crates/openside-proto/src/events.rs` | 1 |
| `crates/opensided/src/engine.rs` | 1 | `crates/opensided/src/lib.rs` | 4 |

0건인 파일: `openside-browser` (`src/browser.rs`, `src/cdp.rs`, `src/lib.rs`, `src/snapshot.rs`, `tests/chromium.rs`, `tests/common/mod.rs`); `openside-cli` (`src/client.rs`, `src/cursor.rs`, `src/lib.rs`, `src/main.rs`, `src/sse.rs`, `src/tests.rs`); `openside-core` (`src/ids.rs`, `src/lib.rs`); `openside-e2e` (`src/bin/fake-omp.rs`, `src/bin/openside.rs`, `src/bin/opensided.rs`); `openside-policy` (`src/glob.rs`, `src/lib.rs`); `openside-proto` (`src/lib.rs`, `src/openapi.rs`, `src/tests.rs`); `openside-sessions` (`src/lib.rs`, `src/state.rs`, `src/store.rs`, `src/tests.rs`); `openside-vault` (`src/lib.rs`, `src/origin.rs`, `src/secret.rs`, `src/tests.rs`); `opensided` (`src/api.rs`, `src/browser.rs`, `src/host.rs`, `src/main.rs`, `src/persistence.rs`, `src/tests.rs`). 각 파일은 0행이다.

## 9. 새 계약 대응

확정 계약의 메시지·티켓·툴 결과·스냅샷 상세는 `docs/specs/sidecar-protocol.md`를 기준으로 한다.

- `open`/`resume_state`: 기존 세션 열기·재개, `SessionSpec.resume`·`model`의 새 대응. `cwd`는 명시 필드가 없어 매핑 확정이 필요하다.
- `user_message{mode: queue|steer|now}`와 `abort`: `AgentSession::send`/`abort`의 대응. 메시지 큐는 계속 데몬 소유이며 runtime `remove_queued_message`/`promote_queued_message`를 호출하지 않는다.
- `tool_call`/`tool_result`: `HostTool*` 역방향 호출 경계. 데몬은 동적 `ToolSpec`으로 검증하고 정책·툴 실행 후 `done`, `deferred`, `denied` 또는 오류를 반환한다. `tool.started`/`tool.ended`도 데몬이 기록한다.
- `ticket_resolved`: `answer_prompt`와 확장 UI 응답을 대체한다. 티켓이 승인되면 동일 `tool_call_id`로 재호출하고, 한 번만 반영한다.
- `message_accepted`/`message_result`: `MessageAccepted`/`PromptResult` 매핑. `message_result`는 `completed|aborted|failed|deferred`다.
- `state_snapshot`/`settled`/`error`: 런 상태 저장·복구, 유휴 완료, 기존 `RuntimeError` 분류의 대응 메시지. 인증 불가(`auth_unavailable`)는 fail-closed로 처리한다.

## 10. 영역별 요약

| area | omp binding | action | new counterpart in sidecar design |
| --- | --- | --- | --- |
| `core/runtime` | Omp 전용 `SessionSpec` 옵션, 승인·프롬프트 타입, 런타임 큐 조작, Omp 이벤트 | KEEP / CHANGE / DELETE | `open`, `user_message`, `abort`, `message_accepted`/`message_result`, `state_snapshot`, `settled`; 데몬 소유 큐와 툴 생명주기 기록 |
| `opensided engine/config/host/API` | Omp 프로세스·CLI 옵션, `request_approval`, extension UI 프롬프트, prompt route·만료, runtime 큐 동기화 | KEEP / CHANGE / DELETE | `agent.command`, JSONL sidecar, 데몬 정책 및 tool reverse RPC, ticket API·`ticket_resolved`, `resume_state` |
| `proto events/API/OpenAPI` | `ApprovalMode`, `Prompt*` API·이벤트·스키마, `"omp reported"` 주석 | KEEP / CHANGE / DELETE | `PolicyMode` 유지, ticket API·이벤트·OpenAPI, 런타임 기준 `MessageOutcome` |
| `sessions` | 프롬프트 맵 저장·생명주기 검증 | KEEP / CHANGE | 데몬 소유 큐, ticket 및 `tool_call_id` 연결, opaque `state_snapshot`/`resume_state` |
| `e2e/workspace/agent-omp` | `openside-agent-omp`, `fake-omp`, Omp CLI 실행 인자와 prompt 테스트 하니스 | DELETE / CHANGE | 크레이트 완전 제거, `openside-agent-sidecar`, `OPENSIDE_AGENT_SCRIPT`/`FunctionModel` 기반 E2E |
| `CLI` | `prompt list`/`prompt answer`, `/v1/prompts` 클라이언트 | CHANGE | ticket 조회·해결 API 및 CLI 명령(이름 미확정) |
| `portability` | `/usr/bin/chromium`, 개인 절대 경로, `Hyprland`, `~/.omp`, broker 주소 매칭 | CHANGE | 설정·환경변수·발견 기반 경로와 구형 `omp` 문서 참조 정리 |