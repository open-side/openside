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
| `open` | `session`, `model{id, fallback[]}`, `system_prompt_parts[]`, `tools[ToolSpec]`, `resume_state?`, `config{credentials, agent}` | 세션 열기. `agent` = `{tool_retries, compaction_max_messages, compaction_keep_messages, profile}`(설정 `[agent]`). `profile` 이 `"ultrabrowse"` 면 사이드카가 높은 추론 설정, 계획 전 브라우저 호출 거절, 최종 답변 인용 검사(재시도 후 `[citation needed]` 표시)를 더하고, 최종 답변 후보 텍스트는 `assistant.delta` 로 보내지 않는다([`ultrabrowse.md`](ultrabrowse.md) §4). 도구·프롬프트 축소는 데몬이 이미 해서 보낸다([`profiles.md`](profiles.md)). 연결당 정확히 1회, `hello` 직후 |
| ↳ 스킬 | — | `system_prompt_parts` 에 `<available_skills>` 목록이 있고 `tools` 에 `skill` 이 있으면, 사용자 메시지의 `/이름`(목록에 있는 이름)마다 사이드카가 모델 요청 전에 `tool_call` 로 `skill` 을 호출해 결과를 대화 기록에 넣는다(`tool_call_id` 는 `skill-mention-…`). 거절·오류면 건너뛴다. [`skills.md`](skills.md) |
| `user_message` | `message_id`, `text`, `mode: "queue"\|"steer"\|"now"` | 유휴면 새 런 시작. 실행 중이면 `steer`/`now` 는 다음 모델 요청 직전에 주입, `queue` 는 현재 런의 작업이 끝난 뒤 처리 |
| `abort` | — | 현재 런 취소. 유휴면 무시 |
| `tool_result` | `rpc_id`, `outcome` | `tool_call` 응답(§6) |
| `ticket_resolved` | `ticket_id`, `tool_call_id`, `resolution` | 보류 호출 해결(§7) |
| `subagent_cancel` | `subagent_id` | 서브에이전트 취소(§10). 모르거나 이미 끝난 서브에이전트면 오류 없이 무시 |
| `discard_deferred` | — | 복원한 보류 이어하기(continuation)를 실행하지 않고 버린다. 보류 호출·`delegate` 대기·미종료 서브에이전트·`deferred_messages` 를 지우고, 히스토리는 답이 없는 tool-call batch 앞까지 자른 뒤 `state_snapshot` 을 보낸다(§8). 실행 중이면 무시 |
| `subagent_steer` | `subagent_id`, `text` | 서브에이전트의 다음 모델 요청 직전에 사용자 메시지로 주입(§10). 모르거나 끝난 서브에이전트면 무시 |
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
| `deferred` | `calls[{tool_call_id, ticket_id, kind}]` | 런이 보류 호출 때문에 멈춤(`DeferredToolRequests`). 이후 `message_result{deferred}` |
| `state_snapshot` | `state{format, data}` | 직렬화된 대화 상태. **일관된 지점(런 종료: completed/aborted/failed/deferred)마다** 전송. 데몬은 최신 것만 보관 |
| `settled` | — | 실행 중인 런도, 처리할 메시지도 없음 |
| `context_compacted` | `subagent?` | 대화 요약으로 이전 메시지가 모델 컨텍스트에서 빠졌음. 데몬은 이를 이후의 `tool_call` 보다 먼저 처리해, 모델이 더는 볼 수 없는 도구 결과 기준(브라우저 스냅샷 id)을 폐기한다. `subagent` 가 없으면 부모 에이전트 |
| `error` | `code`, `message`, `fatal`, `message_id?` | §3 |
| `bye` | — | `shutdown` 완료 |

`state_snapshot`은 실행 종료 시점에 남기는 체크포인트일 뿐이며, 최종 해결 결과를 그대로 재생해도 된다는 근거가 아니다. 사이드카는 스냅샷을 복원할 때 저장되어 있던 미결 호출 해결 결과를 모두 폐기하고 데몬의 `ticket_resolved`를 대기한다.

### 5.1 `event.kind`
`turn.started{}`, `turn.ended{}`(모델 요청 1회 단위), `assistant.delta{text}`, `assistant.final{text}`, `thinking.delta{text}`, `subagent.lifecycle{subagent_id, status}`(`running`/`waiting_human`/`completed`/`failed`/`cancelled`), `subagent.progress{subagent_id, progress}`(`progress.kind`: `model_request`, `model_response`, `steer_queued`, `steer_injected`), `usage{model, input_tokens, output_tokens}`.
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

## 10. 서브에이전트
- 부모 모델이 데몬 툴 `delegate{task, model?, tools?, context?: "fresh"|"fork"}` 를 호출하면 데몬이 허용 여부를 판단한다. 모델은 세션 모델, `models.default`, `models.fallback` 중 하나여야 하며(아니면 `model_not_allowed` 로 거부), 툴은 세션에 노출된 툴 중 `delegate` 를 뺀 범위 안에서만 줄 수 있다. 허용되면 데몬이 `subagent_id` 를 정하고 `subagent.started` 를 기록한 뒤 `{subagent_id, model, tools, context}` 를 돌려준다.
- 사이드카는 이 결과를 받아 서브에이전트 런을 직접 돌린다. `fork` 는 부모 히스토리에서 아직 답이 없는 tool-call batch(그 `delegate` 호출이 든 batch 포함)를 잘라 낸 사본으로 시작한다.
- 서브에이전트의 reverse RPC 에는 `subagent` 가 붙는다. 그래서 서브에이전트의 승인·질문 티켓은 부모 세션 티켓 목록에 `origin.subagent_id` 와 함께 열린다. 승인 재호출은 그 티켓을 연 주체와 같은 `subagent` 일 때만 성립한다(부모·다른 서브에이전트는 쓸 수 없다).
- 서브에이전트가 보류되면 부모의 `delegate` 호출도 사이드카 안에서 보류된다(데몬 티켓 없음). 부모 런은 `deferred` 로 끝나고, 서브에이전트의 보류 호출이 `deferred`·`ready.pending_deferred` 에 실린다. 그 티켓이 해결되면 해당 서브에이전트만 재개된다. 서브에이전트가 끝나면 부모가 `{status:"completed", subagent_id, output}` / `{status:"failed", subagent_id, message}` / `{status:"cancelled", subagent_id}` 를 `delegate` 결과로 받아 재개된다.
- 서브에이전트 상태(대화, 보류 호출, steer 버퍼, 스크립트 위치)는 `state_snapshot` 에 들어간다. 스냅샷은 부모 또는 서브에이전트의 런 종료 지점에서만 찍힌다.
- 취소: 데몬은 티켓 게이트를 잡은 채 `subagent.ended{cancelled}` 를 기록하고 그 서브에이전트의 열린 티켓을 `rejected(reason:"subagent_cancelled")` 로 닫은 뒤 `subagent_cancel` 을 보낸다. 티켓 생성도 같은 게이트 안에서 서브에이전트가 살아 있는지 다시 확인하므로, 끝난 서브에이전트의 티켓은 생기지 않는다. 실행 직전에도 한 번 더 확인해 끝난 서브에이전트의 툴은 실행하지 않는다. 취소로 버려진 reverse RPC 에 데몬이 뒤늦게 보내는 `tool_result` 는 사이드카가 조용히 버린다(모르는 `rpc_id` 는 여전히 `bad_request`).
- 복구: 데몬 재기동 시 `running` 이던 서브에이전트는 `failed(reason:"interrupted")` 가 된다. 이미 끝난 서브에이전트에 열린 티켓이 남아 있으면(종료 기록과 티켓 거부 사이에 크래시) 기동 때 거부해 마무리한다. 미종료 서브에이전트가 있는 세션은 기동 단계에서 바로 사이드카를 열어 아래 대조를 한다(다른 동작을 기다리지 않음). 사이드카를 다시 연 뒤 `ready.pending_deferred` 에 보류 호출이 없는 미종료 서브에이전트(마지막 스냅샷 이후에 티켓이 열린 경우 등)는 깨울 방법이 없으므로 `failed(reason:"not_recoverable")` 로 끝내고 그 티켓을 닫는다. 이미 끝난 서브에이전트의 보류 호출은 해결을 전달하기 전에 `subagent_cancel` 로 정리한다. 사이드카도 복원 시 보류 호출이 없는 미종료 서브에이전트를 실패 처리한다.
- 보장 범위: 티켓 해결 전달은 최대 1회이고, 중단되면 §7 의 중단 대체 응답을 쓴다. 이미 기록된 최종 출력이 있는 이어하기는 §8 에 따라 버리므로 `assistant.final` 이 두 번 나오지 않는다. 부수 효과의 정확히 1회 실행은 보장하지 않는다. `subagent` 값은 세션 사이드카가 붙이는 값이다. 모델의 툴 인자로는 바꿀 수 없지만, 사이드카 자체는 세션 단위 주체로 신뢰한다.
- API: `GET /v1/sessions/{id}/subagents`(read), `POST /v1/sessions/{id}/subagents/{sid}/cancel`(steer, 끝났으면 409 `subagent_finished`), `POST /v1/sessions/{id}/subagents/{sid}/steer {text}`(steer, 202). SSE 는 `subagent.started|lifecycle|progress|ended` 를 보내며 `?subagents=false` 로 뺄 수 있다.
