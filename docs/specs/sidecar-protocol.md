# 사이드카 프로토콜 v1 (opensided ↔ Python `openside_agent`)

작성: 2026-10-11. 상태: **확정(v1)**. 근거: docs/08 §A, docs/09, docs/specs/human-step.md, 스파이크 S-8/S-9/S-15/S-16.
구현: Rust `crates/openside-agent-sidecar`, Python `agent/src/openside_agent/protocol.py`. 이 문서와 구현이 다르면 이 문서를 고치고 양쪽을 맞춘다.
메시지 모양의 기계 판독 계약은 [`sidecar-wire.schema.json`](sidecar-wire.schema.json)(JSON Schema 2020-12)이다. Python 모델에서 `uv run --project agent python -m openside_agent.wire_schema > docs/specs/sidecar-wire.schema.json` 으로 생성한다. `agent/tests/test_wire_schema.py` 는 커밋된 파일이 모델과 같은지, Rust `crates/openside-agent-sidecar/src/wire_contract_tests.rs` 는 데몬이 주고받는 모든 메시지 종류(변형을 빠짐없이 match)가 이 스키마를 통과하는지 확인한다.

## 1. 원칙
- 세션당 Python 프로세스 1개. 데몬이 띄우고, 데몬이 죽이면 같이 죽는다(프로세스 그룹).
- **툴은 데몬이 실행한다.** 사이드카에 노출되는 툴은 `open.tools` 로 받은 명세뿐이고, 호출은 `tool_call` reverse RPC 다. 사이드카가 자체 파일/셸/브라우저 툴을 노출하지 않는다.
- **비밀은 이 프로토콜의 어떤 메시지에도 실리지 않는다**: 모델 자격증명(사이드카가 자격증명 소스에서 직접 읽음), Vault 값, `secret_input` 값, 데몬 능력 토큰. 사이드카 `error.message`·stderr 도 정제된 문장만 담는다.
- 사이드카는 대화 상태를 직렬화해 데몬에 넘기고(`state_snapshot`), 데몬은 그 내용을 해석하지 않고 보관했다가 재기동 시 `open.resume_state` 로 돌려준다.
- 큐는 데몬이 소유한다(`MessageQueue`). 사이드카는 받은 메시지를 즉시 처리할 뿐 별도 영속 큐를 갖지 않는다.

## 2. 전송
1. 데몬이 비공개 디렉터리(0700, 런타임 디렉터리 아래)에 유닉스 소켓을 만들고 0600 으로 바꾼 뒤 리슨한다.
2. 설정 `agent.command`(argv 배열) 를 실행한다. 환경: 데몬 환경 상속 + `OPENSIDE_SIDECAR_SOCKET=<경로>`, `OPENSIDE_SIDECAR_NONCE=<무작위 32 hex>`(일회용 접속 확인 값, 자격증명 아님). stdin 은 닫고, stdout 은 버리고, stderr 는 데몬이 줄 단위로 읽어 길이 제한(줄당 2 KiB, 최근 64줄) 진단 버퍼에 둔다.
3. 사이드카가 접속해 `hello` 를 보낸다. 데몬은 첫 접속 1개만 받고, `nonce` 가 다르거나 `agent.startup_timeout_ms`(기본 30000) 안에 `hello` 가 없으면 프로세스를 죽이고 `Unavailable`. 접속 후 소켓 파일은 지운다.
   `hello.protocol` 이 1 이 아니거나 `hello.versions.openside_agent` 가 지원 범위 **`>=0.1.0,<0.2.0`** 밖이면 역시 프로세스를 죽이고 `Unavailable`(메시지에 지원 범위를 적는다. 상대가 보낸 버전 문자열은 32자 이하의 버전 모양일 때만 그대로 쓴다). `0.1.0+local` 같은 접미사는 허용한다. 범위는 Rust `openside_agent_sidecar::supported_agent_versions()` 와 스키마의 `x-openside-agent-versions` 가 같은 값이며, 양쪽 테스트가 일치를 확인한다. 범위를 바꾸려면 둘 다 바꾸고 `agent/src/openside_agent/__init__.py` 의 `__version__` 이 그 안에 있어야 한다.
4. 프레이밍: UTF-8 JSON 객체 하나 + `\n`. 한 줄 최대 **4 MiB**(`MAX_LINE_BYTES = 4194304`).
5. 봉투: 모든 메시지는 `{"v":1,"type":"<이름>", ...}`. `v != 1` 은 거부.
6. 자원 상한(데몬 쪽): 동시에 실행 중인 `tool_call` 은 연결당 최대 32개. 넘는 호출은 실행하지 않고 즉시 `tool_result{outcome:{status:"error", message:"too many concurrent tool calls"}}`. 중복 `rpc_id` 검사는 실행 중인 것 + 최근 완료 1024개 범위. 사이드카 → 데몬 이벤트 큐는 256개로 제한하고, 데몬이 소비하지 못하면 소켓 읽기를 멈춰 사이드카 쪽이 막힌다(메모리 증가 대신 역압).
- `user_message.text` 및 `subagent_steer.text`의 최대 크기는 UTF-8 기준 1 MiB(1048576 바이트)이다. 첨부 파일은 최대 4개까지 첨부할 수 있고 디코딩 후 전체 합산 크기는 2 MiB 이하(≤4개/총 ≤2 MiB)로 기존과 동일하다. 텍스트가 이 한도를 초과하면 데몬은 큐 적재나 저장소 기록에 앞서 `submit`(메시지 전송), `edit`(대기열 메시지 수정), `child-steer` 요청에 대해 413 `message_too_large`를 반환하며, 사이드카는 치명적이지 않은 `bad_request` 오류로 거부한다.

## 3. 오류 모델
- 받는 쪽에서 줄이 너무 길거나, JSON 이 아니거나, `type` 을 모르거나, 필수 필드가 없으면: 그 줄만 버리고 상대에게 `error{code:"protocol", fatal:false}` 를 보낸다. **프로세스는 죽지 않는다**(퍼징 테스트 대상).
- `error.code`: `protocol`, `bad_request`(상태상 처리 불가 — 예: 실행 중이 아닌데 abort 는 무해하므로 무시하고 오류 아님), `stale_ticket`(이미 해결됐거나 모르는 보류 호출), `model_error`(제공자 오류, 정제됨), `auth_unavailable`(자격증명 소스 전부 불가, **fatal**), `tool_gate`(노출 툴 불일치, fatal), `internal`.
- `fatal:true` 를 받거나 보낸 쪽은 연결을 닫는다. 데몬은 `auth_unavailable` 를 기존 fail-closed 경로(`runtime.error{code:"auth_unavailable"}` + `session.stopped`, 다른 자격증명 폴백 없음)로 처리한다.

## 4. 데몬 → 사이드카
| type | 필드 | 의미 |
| --- | --- | --- |
| `open` | `session`, `model{id, fallback[]}`, `system_prompt_parts[]`, `tools[ToolSpec]`, `resume_state?`, `config{credentials, agent}` | 세션 열기. `agent` = `{tool_retries, compaction_max_messages, compaction_keep_messages, profile, orchestration, front_tools?, front_model?}`(설정 `[agent]`, §11). `profile` 이 `"ultrabrowse"` 면 사이드카가 높은 추론 설정, 계획 전 브라우저 호출 거절, 최종 답변 인용 검사(재시도 후 `[citation needed]` 표시)를 더하고, 최종 답변 후보 텍스트는 `assistant.delta` 로 보내지 않는다([`ultrabrowse.md`](ultrabrowse.md) §4). 도구·프롬프트 축소는 데몬이 이미 해서 보낸다([`profiles.md`](profiles.md)). 연결당 정확히 1회, `hello` 직후 |
| ↳ 스킬 | — | `system_prompt_parts` 에 `<available_skills>` 목록이 있고 `tools` 에 `skill` 이 있으면, 사용자 메시지의 `/이름`(목록에 있는 이름)마다 사이드카가 모델 요청 전에 `tool_call` 로 `skill` 을 호출해 결과를 대화 기록에 넣는다(`tool_call_id` 는 `skill-mention-…`). 거절·오류면 건너뛴다. [`skills.md`](skills.md) |
| `user_message` | `message_id`, `text`, `mode: "queue"\|"steer"\|"now"` | 유휴면 새 런 시작. 실행 중이면 `steer`/`now` 는 다음 모델 요청 직전에 주입, `queue` 는 현재 런의 작업이 끝난 뒤 처리 |
| `abort` | — | 현재 런 취소. 유휴면 무시 |
| `tool_result` | `rpc_id`, `outcome` | `tool_call` 응답(§6) |
| `ticket_resolved` | `ticket_id`, `tool_call_id`, `resolution` | 보류 호출 해결(§7) |
| `subagent_cancel` | `subagent_id` | 서브에이전트 취소(§10). 모르거나 이미 끝난 서브에이전트면 오류 없이 무시 |
| `discard_deferred` | — | 복원한 보류 이어하기(continuation)를 실행하지 않고 버린다. 보류 호출·`delegate` 대기·미종료 서브에이전트·`deferred_messages` 를 지우고, 히스토리는 답이 없는 tool-call batch 앞까지 자른 뒤 `state_snapshot` 을 보낸다(§8). 실행 중이면 무시 |
| `subagent_steer` | `subagent_id`, `text`, `control_id` | 자식 에이전트가 다음 모델 요청을 보내기 전에 지시 사항을 사용자 메시지(user message)로 주입한다. 전달 확인(ack)은 `control_id`로 대응시키며, 대상을 알 수 없거나 이미 종료된 자식 에이전트는 `delivered:false`로 확인을 반환한다. |
| `shutdown` | — | 진행 중 런 취소 → 마지막 `state_snapshot` → `bye` → 종료(0). 데몬은 5초 후 프로세스 그룹 SIGKILL |

`ToolSpec = {name, description, parameters(JSON Schema object), approval: "read"|"write"|"exec"|"sensitive", deferrable: bool}`. 스키마는 데몬의 `schemars` 출력이 유일한 원본이다(사이드카에서 이중 정의 금지).
`model.id`/`fallback[]` 는 설정값 문자열(`provider:model`)이며 코드에 특정 ID 를 두지 않는다. `config.credentials` 는 설정 파일의 `[credentials]` 절(비밀 아님: 소스 순서, broker URL, 토큰 **파일 경로**, 환경변수 이름, `api_key_cmd` argv)이다.
`resume_state = {format:"openside-agent-state/1", data:"<불투명 문자열>"}`.

## 5. 사이드카 → 데몬
| type | 필드 | 의미 |
| --- | --- | --- |
| `hello` | `protocol:1`, `nonce`, `pid`, `versions{python, pydantic_ai, pydantic_ai_harness, openside_agent}` | 접속 직후 1회 |
| `ready` | `tools[]`(실제 노출 이름), `pending_deferred[{tool_call_id, ticket_id, kind}]`, `startup_ms` | `open` 처리 완료. 데몬은 `tools` 집합이 `open.tools` 이름 집합과 **정확히 같지 않으면** 세션 시작을 거부(`tool_gate`) |
| `event` | `event{kind, ...}` | 관측 이벤트(§5.1). 데몬이 세션 이벤트 로그로 정규화 |
| `message_accepted` | `message_id` | 메시지가 대화 히스토리에 들어감(모델 요청에 포함) |
| `message_result` | `message_id`, `outcome: completed\|aborted\|failed\|deferred`, `reason?` | 메시지를 포함한 런의 종료. 런에 들어간 모든 메시지마다 1회. `deferred` 로 끝난 메시지는 재개 런이 끝날 때 최종 결과(`completed`/`aborted`/`failed`)를 한 번 더 받는다. 재개 런이 다시 보류되면 `deferred` 를 반복해서 보내지 않는다 |
| `tool_call` | `rpc_id`, `tool_call_id`, `name`, `args`, `subagent?`, `approval_ticket?` | reverse RPC. 동시에 여러 개 가능. 데몬은 같은 `rpc_id` 로 `tool_result` 를 1회 보낸다 |
| `subagent_steer_result` | `control_id`, `delivered` | 동일한 `control_id`를 담아 `subagent_steer`에 응답한다. 대상 에이전트가 지시 사항을 수락했을 때만 `delivered:true`로 설정하고, 대상을 알 수 없거나 이미 종료된 경우에는 `false`로 설정한다. 데몬은 이 값이 `true`일 때만 202를 반환하며, 그렇지 않으면 409 `subagent_finished`를 반환한다. |
| `deferred` | `calls[{tool_call_id, ticket_id, kind}]` | 런이 보류 호출 때문에 멈춤(`DeferredToolRequests`). 이후 `message_result{deferred}` |
| `state_snapshot` | `state{format, data}` | 직렬화된 대화 상태. **일관된 지점(런 종료: completed/aborted/failed/deferred)마다** 전송. 데몬은 최신 것만 보관 |
| `settled` | — | Front 에이전트가 유휴 상태(idle settled)가 되면 대기열에 쌓인 사용자 메시지를 바로 다음 작업으로 넘겨 처리한다. 데몬은 백그라운드 작업이 실행되는 동안 `session.settled` 이벤트 전달을 보류한다(§11) |
| `context_compacted` | `subagent?` | 대화 요약으로 이전 메시지가 모델 컨텍스트에서 빠졌음. 데몬은 이를 이후의 `tool_call` 보다 먼저 처리해, 모델이 더는 볼 수 없는 도구 결과 기준(브라우저 스냅샷 id)을 폐기한다. `subagent` 가 없으면 부모 에이전트 |
| `error` | `code`, `message`, `fatal`, `message_id?` | §3 |
| `bye` | — | `shutdown` 완료 |

`state_snapshot`은 실행 종료 시점에 남기는 체크포인트일 뿐이며, 최종 해결 결과를 그대로 재생해도 된다는 근거가 아니다. 사이드카는 스냅샷을 복원할 때 저장되어 있던 미결 호출 해결 결과를 모두 폐기하고 데몬의 `ticket_resolved`를 대기한다.

### 5.1 `event.kind`
`turn.started{}`, `turn.ended{}`(모델 요청 1회 단위), `assistant.delta{text}`, `assistant.final{text}`, `thinking.delta{text}`, `subagent.lifecycle{subagent_id, status, output?}`(`running`/`waiting_human`/`completed`/`failed`/`cancelled`), `subagent.progress{subagent_id, progress}`(`progress.kind`: `model_request`, `model_response`, `steer_queued`, `steer_injected`), `usage{model, input_tokens, output_tokens}`.
툴 생명주기(`tool.started/ended`)는 **데몬이 직접** 기록한다(툴을 실행하는 쪽이 데몬이고, 인자 redaction 이 데몬에서 일어나므로). 사이드카는 보내지 않는다.

## 6. 툴 호출(reverse RPC)
`tool_result.outcome`:
- `{"status":"done","for_model": <JSON 값>}` → 툴 반환값.
- `{"status":"deferred","ticket_id":"…","kind":"approval"}` → 사이드카는 `ApprovalRequired(metadata={ticket_id})`. 정책 `Ask` 의 경로.
- `{"status":"deferred","ticket_id":"…","kind":"external"}` → `CallDeferred(metadata={ticket_id})`. `request_human_step`, `ask_user` 의 경로.
- `{"status":"denied","reason":"…","hint":"…"|null}` / `{"status":"error","message":"…"}` → `ModelRetry`(이유와 대안 문장). 재시도 한도는 사이드카 설정(기본 3).
사이드카는 동일한 `tool_call_id`에 `approval_ticket`을 실어 재호출할 수 있다. 데몬은 세션, 호출 ID, 도구, 호출 주체(부모 또는 같은 `subagent`)가 일치하는지, 그리고 승인된 `Confirm`으로 해결되었는지 검증한다. 데몬은 `Ask` 확인 티켓을 열 때 화면 표시용으로 가공된 인자가 아니라 원본 인자 전체와 권한(capability) 및 대상을 포함한 정책 동작(policy actions) 전체를 재귀적으로 키 정렬한 canonical JSON의 SHA-256 해시를 내부에 영구 보관한다. 재호출이 성립하려면 이 바인딩이 일치해야 하며, 도구 실행 전에 해당 승인을 원자적으로 영구 보관 장치에 `consumed`로 표기해야 한다. 인자나 정책 동작의 권한 및 대상이 불일치하면 `approval_args_mismatch`로 거부하고, 이미 사용된 승인은 `approval_already_used`로 거부한다. 명시적 `Deny`는 여전히 우선권을 가진다. 선점 표기 후 크래시가 발생했다면 해당 승인은 재시도할 수 없다.

## 7. 티켓 해결과 재개
`ticket_resolved.resolution`:
- `approval` 종류: `{"status":"approved"}` → `ToolApproved()`, `{"status":"denied","reason":"…"}` → `ToolDenied(reason)`(만료·거부 포함).
- `external` 종류: `{"status":"result","for_model": <JSON>}` → 그 값이 보류됐던 같은 tool call 의 결과. 만료·거부도 `for_model:{"status":"expired"|"rejected", ...}` 로 전달한다(비밀 값 없음, `secret_input` 은 `{"status":"filled"}`).
사이드카는 멈춘 런의 보류 호출이 **모두** 해결될 때까지 모아 두었다가 `DeferredToolResults` 로 새 런을 시작한다(이 런은 보류됐던 원래 메시지의 `message_result` 를 보낸다, 종료 시 `state_snapshot`·`settled`). 모르는/이미 반영된 `tool_call_id` 는 `error{stale_ticket}` 후 무시 → **재개는 최대 1회**.
보류 중인 동안 데몬은 큐의 새 메시지를 사이드카로 보내지 않는다(세션 상태 `waiting_human`). 세션 중지(stop)는 열린 티켓을 `rejected(reason:"stopped")` 로 닫는다.

데몬은 각 최종 `ticket_resolved`를 전송하기 전에 `delivered` 상태를 먼저 커밋한다. 복원된 스냅샷에 미결 상태로 남아 있더라도 이미 전달된 적이 있는 해결 결과는 원래 내용 그대로 재전송하지 않는다. 승인 티켓에는 `{status:"denied",reason:"interrupted_after_resume"}`를 반환하고, 외부 티켓에는 `{status:"result",for_model:{status:"interrupted",message:"The previous resumption was interrupted; verify state before retrying."}}`를 반환하며 데몬은 `runtime.error{code:"interrupted"}`를 발행한다. 중단된 부수 효과는 실행 여부가 불확실하므로 재시도 전에 상태를 확인해야 하며, 이 정책이 부수 효과의 정확히 1회 실행을 보장하지는 않는다. 모르는 호출이나 이미 반영된 호출에 대한 기존 라이브 `stale_ticket` 중복 제거 규칙은 그대로 유지한다.

## 8. 재기동·크래시 복구
- 사이드카 비정상 종료(EOF/종료 코드) 감지 → 진행 중 메시지(주입됐지만 `message_result` 없음)는 데몬이 `message.completed{failed, reason:"interrupted"}` 로 닫는다(at-most-once, 재전송 없음). 큐에 남은 메시지는 보존된다.
- 세션을 다시 열 때는 최신 스냅샷을 전달한다. `ready.pending_deferred`에 포함된 최종 티켓 중 전달되지 않은 티켓에는 원래의 최종 해결 결과를 전달하고, 이미 전달된 티켓에는 §7의 중단 대체 응답을 전달한다. 데몬 기동 시 중지되었거나 인증에 실패한 세션을 제외하고 대기열 메시지가 없더라도 최종 미전달 해결 결과의 발행을 예약한다. 반면 모든 해결이 이미 전달된 세션(재개 런 도중 크래시)은 기동 시 자동으로 열지 않는다. 사이드카 스냅샷은 데몬에게 불투명해 남은 보류 호출을 알 수 없기 때문이며, 중단 대체 응답과 `runtime.error{code:"interrupted"}` 는 그 세션의 런타임이 다음에 열릴 때(새 메시지 등) 전달·기록된다. 사이드카만 죽은 경우에는 크래시 시점에도 `runtime.error{code:"interrupted"}` 가 기록된다. 신규 와이어 필드는 추가하지 않는다.
- 스냅샷은 런 종료 지점에서만 보내므로, 런 도중 크래시는 그 런의 부분 히스토리를 잃는다(그 메시지는 위와 같이 failed 로 드러남).
- 스냅샷은 전체 `state_snapshot` JSON UTF-8 엔벨로프 크기를 4 MiB 이하로 유지해야 한다. 한도를 초과하면 대화 기록과 대기열의 steer에 포함된 미디어를 먼저 정리한다. 대화 기록은 이미 완료된 과거 턴만 순차적으로 잘라낼 수 있으며, 부모·자식 에이전트 모두 아직 응답을 기다리는 도구 호출(pending tool call)부터 가장 최근 메시지까지의 대화 구간은 그대로 보존한다. 이렇게 정리한 뒤에도 4 MiB를 넘으면 잘못된 스냅샷을 전송하지 않고 정상 연결을 유지한 채 `'Snapshot exceeds 4 MiB; pending continuation cannot be checkpointed'` 메시지와 함께 치명적이지 않은 `internal` 오류를 반환한다.
- 재개 런의 `assistant.final` 은 기록됐는데 그 뒤 스냅샷이 기록되기 전에 크래시하면, 복원된 사이드카는 이미 끝난 보류 호출을 여전히 `ready.pending_deferred` 로 보고한다. 데몬은 보고된 티켓이 모두 닫혔고 마지막 닫힘 이후에 `assistant.final` 이 기록돼 있으면 그 이어하기를 다시 돌리지 않고 `discard_deferred` 를 보낸다. 그래서 최종 출력은 두 번 나오지 않는다. 대신 그 재개 런의 대화 내용은 다음 대화 히스토리에서 빠진다.

## 9. 테스트용 스크립트 모델
`OPENSIDE_AGENT_SCRIPT=<JSON 파일 경로>` 가 있으면 사이드카는 설정 모델 대신 `FunctionModel` 기반 스크립트 모델을 쓴다(자격증명 소스를 열지 않음). 형식:
```json
{"responses": [
  {"parts": [{"type": "text", "text": "hello"}]},
  {"parts": [{"type": "tool_call", "name": "request_human_step", "args": {"kind": "info", "summary": "…"}, "id": "call-1"}]}
], "delay_ms": 0}
```
모델 요청마다 다음 응답을 하나 소비한다(스트리밍 시 텍스트를 단어 단위 delta 로 쪼갬). 응답이 바닥나면 텍스트 `"[script exhausted]"`. `{"type":"wait_steer"}` 파트가 있으면 다음 steer 메시지가 주입될 때까지(최대 `delay_ms`, 파트에 `timeout_ms` 가 있으면 그 값) 그 응답을 지연한다. `"subagents": {"<부모 delegate tool_call_id>": [응답…]}` 로 서브에이전트별 응답 목록을 따로 줄 수 있으며, 각 서브에이전트는 자기 소비 위치를 가진다. 스크립트 상태(소비 위치)는 `state_snapshot` 에 포함돼 재기동 후 이어진다.
- `echo`: 가장 최근에 입력된 사용자 프롬프트를 그대로 출력한다.
- `{{result:call.field.path}}`: 최상위 문자열 인자 안에서 도구 호출 반환 객체의 필드 경로 또는 숫자 배열 인덱스를 따라가 값을 치환한다.
- `script.subagents`: `task_start` 및 `delegate` 호출 ID를 하위 에이전트 응답 목록과 매핑하며, 하위 작업마다 독립적인 커서와 steer 이벤트를 관리한다.

## 10. 서브에이전트
- 부모 모델이 데몬 툴 `delegate{task, model?, tools?, context?: "fresh"|"fork"}` 를 호출하면 데몬이 허용 여부를 판단한다. 모델은 세션 모델, `models.default`, `models.fallback` 중 하나여야 하며(아니면 `model_not_allowed` 로 거부), 툴은 세션에 노출된 툴 중 `delegate` 를 뺀 범위 안에서만 줄 수 있다. 허용되면 데몬이 `subagent_id` 를 정하고 `subagent.started` 를 기록한 뒤 `{subagent_id, model, tools, context}` 를 돌려준다.
- 사이드카는 이 결과를 받아 서브에이전트 런을 직접 돌린다. `fork` 는 부모 히스토리에서 아직 답이 없는 tool-call batch(그 `delegate` 호출이 든 batch 포함)를 잘라 낸 사본으로 시작한다.
- 서브에이전트의 reverse RPC 에는 `subagent` 가 붙는다. 그래서 서브에이전트의 승인·질문 티켓은 부모 세션 티켓 목록에 `origin.subagent_id` 와 함께 열린다. 승인 재호출은 그 티켓을 연 주체와 같은 `subagent` 일 때만 성립한다(부모·다른 서브에이전트는 쓸 수 없다).
- 서브에이전트가 보류되면 부모의 `delegate` 호출도 사이드카 안에서 보류된다(데몬 티켓 없음). 부모 런은 `deferred` 로 끝나고, 서브에이전트의 보류 호출이 `deferred`·`ready.pending_deferred` 에 실린다. 그 티켓이 해결되면 해당 서브에이전트만 재개된다. 서브에이전트가 끝나면 부모가 `{status:"completed", subagent_id, output}` / `{status:"failed", subagent_id, message}` / `{status:"cancelled", subagent_id}` 를 `delegate` 결과로 받아 재개된다.
- 서브에이전트 상태(대화, 보류 호출, steer 버퍼, 스크립트 위치)는 `state_snapshot` 에 들어간다. 스냅샷은 부모 또는 서브에이전트의 런 종료 지점에서만 찍힌다.
- 취소: 데몬은 자식 에이전트별로 실행 중인 호스트 도구를 취소하고 정리가 완료될 때까지 대기한 뒤, `subagent.ended{cancelled}`를 기록하고 `ticket_gate` 아래에서 대기 중인 자식 티켓을 거부하며 `subagent_cancel`을 전송한다. 셸 프로세스 그룹은 종료 및 수거(kill/reap)되며, `browser_repl`은 스크립트를 취소하고 렌더러와 입력 정리가 끝날 때까지 대기한 다음 브라우저 게이트(browser gate) 또는 소유권을 해제한다. 이때 부모 및 형제 에이전트는 영향을 받지 않으며, 버려진 역방향 RPC `tool_result`는 안전하게 무시된다.
- 일반 네이티브 내비게이션 취소 시 먼저 `Page.stopLoading` 확인(ack)을 요청하며, 중단이 확인되지 않으면 이미 발행된 내비게이션이 안정화(settle)될 때까지 대기한 후 최종 종료 상태를 발행한다. 새 `Open`(`browser_open_tab`) 취소는 `createTarget`을 끝까지 드레인(drain)해 대상 ID(target ID)를 파악한 뒤 새로 생성된 해당 탭만 닫으며, 닫기가 확인되지 않으면 발행된 로드가 안정화될 때까지 대기한다. 이때 이 `Open` 정리 과정에서 기존 탭이 닫히는 일은 없다. 그 외 브라우저나 파일 상태를 변경하는 도구 호출 또한 로컬 퓨처를 단순히 버리지 않고 이미 발행된 명령과 파일시스템 I/O를 끝까지 드레인한 뒤에야 최종 종료 상태 기록 및 소유권 해제를 진행한다.
- 복구: 데몬 재기동 시 `running` 이던 서브에이전트는 `failed(reason:"interrupted")` 가 된다. 이미 끝난 서브에이전트에 열린 티켓이 남아 있으면(종료 기록과 티켓 거부 사이에 크래시) 기동 때 거부해 마무리한다. 미종료 서브에이전트가 있는 세션은 기동 단계에서 바로 사이드카를 열어 아래 대조를 한다(다른 동작을 기다리지 않음). 사이드카를 다시 연 뒤 `ready.pending_deferred` 에 보류 호출이 없는 미종료 서브에이전트(마지막 스냅샷 이후에 티켓이 열린 경우 등)는 깨울 방법이 없으므로 `failed(reason:"not_recoverable")` 로 끝내고 그 티켓을 닫는다. 이미 끝난 서브에이전트의 보류 호출은 해결을 전달하기 전에 `subagent_cancel` 로 정리한다. 사이드카도 복원 시 보류 호출이 없는 미종료 서브에이전트를 실패 처리한다.
- 브라우저 상태는 데몬이 재시작되어도 유지되도록 Task의 탭 소유권을 디스크에 영속 저장한다. 사이드카 조정(reconciliation)이나 새 브라우저 도구 요청을 허용하기 전에, 복구 대기 중인 Task의 사전 예약을 먼저 복원한다. 종료되었거나 복구할 수 없는 Task는 해당 예약을 해제한다.
- 보장 범위: 티켓 해결 전달은 최대 1회이고, 중단되면 §7 의 중단 대체 응답을 쓴다. 이미 기록된 최종 출력이 있는 이어하기는 §8 에 따라 버리므로 `assistant.final` 이 두 번 나오지 않는다. 부수 효과의 정확히 1회 실행은 보장하지 않는다. `subagent` 값은 세션 사이드카가 붙이는 값이다. 모델의 툴 인자로는 바꿀 수 없지만, 사이드카 자체는 세션 단위 주체로 신뢰한다.
- API: `GET /v1/sessions/{id}/subagents`(read), `POST /v1/sessions/{id}/subagents/{sid}/cancel`(steer, 끝났으면 409 `subagent_finished`), `POST /v1/sessions/{id}/subagents/{sid}/steer {text}`(steer, 202). SSE 는 `subagent.started|lifecycle|progress|ended` 를 보내며 `?subagents=false` 로 뺄 수 있다.

## 11. Front 에이전트와 백그라운드 Task

### 오케스트레이션 구성
- `[agent]`의 `orchestration="front"`가 기본 설정이다. 기존의 단일 순차 실행 방식이 필요하면 `"single"`을 지정한다.
- `[models]`의 `front` 항목으로 빠른 프론트 모델을 지정할 수 있다. 이 설정이 없으면 프론트 모드를 유지한 채 현재 세션 모델을 그대로 사용한다.
- 사용자가 보내는 모든 메시지는 Front 에이전트가 처리한다.

### 도구 권한 및 실행 제약
- Front 에이전트에게 주어지는 도구는 다음과 같다:
  - Task 제어 도구: `task_start(goal, optional tools)`, `task_steer(task_id, message)`, `task_cancel(task_id)`, `task_status()`
  - 브라우저 및 보조 도구: `browser_tabs`, `browser_snapshot`, `browser_screenshot`, `memory_search`, `skill`
- Front 에이전트가 브라우저나 시스템을 직접 변경하려 시도하면 데몬이 `front_agent_cannot_act` 오류를 내며 거부한다.
- `task_*` 도구는 하위 작업(child task)에 부여하지 않으며, `delegate` 도구는 프론트 모드에서 감춘다.
- `task_start`는 세션 모델을 기반으로 백그라운드 ADR020 하위 에이전트를 띄우고, 즉시 `task_id`와 running 상태를 반환한다. 작업이 시작될 때 `subagent.started`(background: true) 이벤트를, 작업이 끝나면 `subagent.ended` 이벤트로 최종 출력을 기록한다.

### 대기열 처리 및 제어(Steer) 동작
- 데몬은 백그라운드 작업이 실행되는 동안 `session.settled` 이벤트 전달을 보류한다. 다만 Front 에이전트가 유휴 상태(idle settled)가 되면 대기열에 쌓인 사용자 메시지를 바로 다음 작업으로 넘겨 처리한다. 즉, 하위 작업 티켓이 Front 대기열을 가로막지 않는다.
- `task_steer`는 이미 돌고 있는 블로킹 도구를 강제로 끊지 않고, 다음 모델 요청 직전에 전달된다.
- 호스트의 `task_steer` 응답이 `pending` 상태인 것은 요청이 초기 진입 단계에서 수용(admission)되었을 뿐 아직 대상에 전달이 완료되지 않았음을 의미한다. 사이드카는 지시 사항을 정상적으로 수락했을 때만 `steered`를 반환하며, 작업 완료 처리가 우선하면 `task_finished` 거부를 반환한다.
- 하위 작업이 끝나면 영속 메시지 ID로 중복을 방지한 `task-report-<task_id>` steer 메시지를 대기열에 넣는다. 작업 종료 후 리포트 추가 사이에 크래시가 발생하더라도 기동 시 복구 작업을 거친다.
- Task 완료 인도의 정확히 한 번(exactly-once) 보장 대상은 영속화된 'Task-finished' 메시지 행(`task-report-<task_id>`)과 이벤트 로그에 보존되는 작업 출력(`subagent.ended.output`)이다. Front가 생성하는 텍스트 요약은 최선 노력(best-effort)으로 동작한다. 리포트 주입(`MessageInjected`) 후 `assistant.final` 전에 사이드카가 종료되더라도 해당 리포트는 실패 또는 중단(`failed`/`interrupted`)으로 안전하게 마킹되어 재실행되지 않으며, 영속화된 행과 작업 출력은 그대로 유지된다.

### 브라우저 탭 격리 및 상태 표현
- 한 세션 안의 Front 에이전트와 모든 Task는 동일한 브라우저 작업공간을 함께 사용한다. 작업 탭 소유권 원장(owner ledger)을 두어 다른 활성 Task가 점유 중인 탭을 조작하려 하면 `tab_owned_by_task` 오류로 거부한다(읽기 접근은 허용). Front 에이전트는 읽기 전용으로만 탭을 조회한다.
- 웜 리스(warm lease)는 `browser_repl`을 포함해 실제 Task가 탭을 처음 점유하는 시점에 브라우저 게이트(browser gate) 아래에서 원자적으로 회수된다. `Open`은 아직 점유되지 않은 웜 탭만 가져오며, 점유된 탭은 TTL 정리 대상에 포함되어 닫히는 일이 없다. 비동기 `browser.state` 조사는 Task별 세대(generation)와 소유권 및 종료 상태 재확인을 통해 격리되므로, 이전 조사가 이미 완료되었거나 더 새로운 작업 상태를 되돌릴 수 없다.
- `browser.state` 스키마는 `task_id?`, `tab_id`, `url`, `host`, `title`, `phase`, `action?`, `page_kind`, `facts`, `thumbnail?` 필드로 이뤄진다.
- 요소 라벨은 모델 추론값이 아니라 접근성(AX) 트리에서 직접 가져오며, 입력 필드 값은 일절 넣지 않고 레지스트리 마스킹 규칙을 적용한다.
- 썸네일은 불투명 JPEG ID로 다루며, `browse` 권한 인증을 거쳐 `GET /v1/sessions/{id}/browser/thumbnails/{thumbnail_id}`로 가져온다.
  - 최대 360x225 해상도, 품질 35, 이미지당 최대 128KiB, 메모리 보관 한도 64장, 만료 시간 10분, `no-store` 캐시 정책을 둔다.
  - 썸네일은 세션, 탭, 문서에 종속된다. 비공개이거나 비밀 정보가 섞인 화면, 사용자 직접 제어(takeover), 보관, 중지, 삭제, 페이지 이동, 만료된 이미지에 대해서는 썸네일을 비워 두거나 404를 반환한다.

### 사전 예열(Prewarm) 및 정리
- 프론트 세션을 만들 때 즉시 준비 작업을 시작하며, 컴포저 포커스 시 `POST /v1/sessions/{id}/prewarm`을 호출하여 예열한다. 새 컴포저 포커스 시에는 브라우저 전용 엔드포인트인 `POST /v1/browser/prewarm`을 호출한다.
- 런타임과 브라우저 준비는 동시에 진행하며, 빈 탭 하나를 미리 열어 두었다가 다음 탭 열기 요청 시 그대로 넘겨준다. 헤드리스 환경에서는 데스크톱 창이나 포커스를 띄우지 않는다.
- 사용하지 않는 탭은 세션을 중지, 보관, 삭제할 때 정리하거나 10분 TTL이 지나면 닫으며, 기동 직후 최종 상태를 다시 확인한다.
- 예열 API는 202 Accepted를 반환하는 멱등성을 지닌 최선 노력(best-effort) 방식으로 작동한다. 예열 중 발생한 실패는 실제 메시지를 처리할 때 드러난다.
- 테스트 시에는 `OPENSIDE_TEST_DISABLE_PREWARM`과 `OPENSIDE_AGENT_SCRIPT` 환경변수를 설정해 자동 사전 준비 및 예열을 우회할 수 있으며, 이 경로에서는 실제 공급자(live provider) 호출을 측정하지 않는다.
