# 사이드카 프로토콜 v1 (opensided ↔ Python `openside_agent`)

작성: 2026-10-11. 상태: **확정(v1)**. 근거: docs/08 §A, docs/09, docs/specs/human-step.md, 스파이크 S-8/S-9/S-15/S-16.
구현: Rust `crates/openside-agent-sidecar`, Python `agent/src/openside_agent/protocol.py`. 이 문서와 구현이 다르면 이 문서를 고치고 양쪽을 맞춘다.
메시지 모양의 기계 판독 계약은 [`sidecar-wire.schema.json`](sidecar-wire.schema.json)(JSON Schema 2020-12)이다. Python 모델에서 `uv run --project agent python -m openside_agent.wire_schema > docs/specs/sidecar-wire.schema.json` 으로 생성한다. `agent/tests/test_wire_schema.py` 는 커밋된 파일이 모델과 같은지, Rust `crates/openside-agent-sidecar/src/wire_contract_tests.rs` 는 데몬이 주고받는 모든 메시지 종류(변형을 빠짐없이 match)가 이 스키마를 통과하는지 확인한다.

## 1. 원칙
- 세션당 Python 프로세스 1개. 데몬이 띄우고, 데몬이 죽이면 같이 죽는다(프로세스 그룹).
- **툴은 데몬이 실행한다.** 사이드카에 노출되는 툴은 `open.tools` 로 받은 명세뿐이고, 호출은 `tool_call` reverse RPC 다. 사이드카가 자체 파일/셸/브라우저 툴을 노출하지 않는다.
- 외부 모델 요청을 시도할 때마다 인증된 비공개 소켓을 통해 데몬에서 모델 자격 증명을 가져온다. 계정 선택, 세션 어피니티, 갱신, 쿨다운, 유효하지 않은 계정 상태 관리는 모두 데몬이 전담한다. 사이드카는 임시 API 키나 액세스 토큰만 전달받으며 리프레시 토큰, 자격 증명 홈 디렉터리, 브로커 베어러 토큰은 절대 받지 않는다. 처리 결과 역시 업스트림 응답 본문 없이 애플리케이션 정의 결과만 보고한다. 자격 증명은 상태 스냅샷, 공개 API, 이벤트, 오류, 로그, argv, 환경변수에 일절 노출되지 않는다. Python 진단 도구 또한 데몬이 정제해 제공한 계정 메타데이터만 읽으며, 독립 실행 진단 도구가 로그인 준비 완료 상태를 자체 판정할 수 없다.
- 자격 증명이 담긴 직렬화된 Rust 프레임은 버퍼 메모리를 0으로 초기화(zeroizing)한다. Python은 소비되거나 해제된 자체 가변 JSONL 및 토큰 버퍼를 비우고, HTTP 시도가 끝나면 주입된 인증 헤더를 제거한다. 다만 CPython, JSON 디코딩, HTTP SDK 내부에서 생성되는 불변 문자열이나 전송 복사본은 완벽히 지우기 어려우므로 프로세스 메모리가 완전히 소거된다고 보장하지는 않는다.
- 사이드카는 대화 상태를 직렬화해 데몬에 넘기고(`state_snapshot`), 데몬은 그 내용을 해석하지 않고 보관했다가 재기동 시 `open.resume_state` 로 돌려준다.
- 큐는 데몬이 소유한다(`MessageQueue`). 사이드카는 받은 메시지를 즉시 처리할 뿐 별도 영속 큐를 갖지 않는다.

## 2. 전송
1. 데몬이 비공개 디렉터리(0700, 런타임 디렉터리 아래)에 유닉스 소켓을 만들고 0600 으로 바꾼 뒤 리슨한다.
2. 설정 `agent.command`(argv 배열) 를 실행한다. 환경: 데몬 환경 상속 + `OPENSIDE_SIDECAR_SOCKET=<경로>`, `OPENSIDE_SIDECAR_NONCE=<무작위 32 hex>`(일회용 접속 확인 값, 자격증명 아님). stdin 은 닫고, stdout 은 버리고, stderr 는 데몬이 줄 단위로 읽어 길이 제한(줄당 2 KiB, 최근 64줄) 진단 버퍼에 둔다.
3. 사이드카가 접속해 `hello` 를 보낸다. 데몬은 첫 접속 1개만 받고, `nonce` 가 다르거나 `agent.startup_timeout_ms`(기본 30000) 안에 `hello` 가 없으면 프로세스를 죽이고 `Unavailable`. 접속 후 소켓 파일은 지운다.
   `hello.protocol`은 반드시 1이어야 하고 `hello.versions.openside_agent`는 **`>=0.1.1,<0.2.0`** 범위 안에 있어야 한다. 그렇지 않으면 데몬이 프로세스를 중지하고 `Unavailable`을 반환한다. 0.1.0 버전은 데몬 풀 리스를 쓰지 않고 로컬에서 자격 증명을 불러올 위험이 있어 의도적으로 거부한다. 정제된 거부 응답에는 길이 제한이 적용된 버전 형태의 피어 텍스트만 표시될 수 있으며, 그 외 텍스트는 `<invalid>`로 대체된다. `0.1.1+local` 같은 접미사는 허용된다. Rust `supported_agent_versions()`, 생성된 `x-openside-agent-versions`, Python 패키지 버전은 함께 검증된다.
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
| `open` | `session`, `model{id, fallback[]}`, `system_prompt_parts[]`, `tools[ToolSpec]`, `resume_state?`, `config{credentials, agent}` | Opens exactly one session per connection, immediately after `hello`. `agent` = `{tool_retries, compaction_max_tokens, compaction_max_messages?, compaction_keep_messages, profile, orchestration, front_tools?, front_model?}` (`[agent]`, §11). The optional message threshold may be null. `ultrabrowse` adds high reasoning, planning-before-browser checks, final citation validation, and no candidate final-text deltas ([`ultrabrowse.md`](ultrabrowse.md) §4). The daemon supplies the reduced tool/prompt profile ([`profiles.md`](profiles.md)). |
| ↳ 스킬 | — | `system_prompt_parts` 에 `<available_skills>` 목록이 있고 `tools` 에 `skill` 이 있으면, 사용자 메시지의 `/이름`(목록에 있는 이름)마다 사이드카가 모델 요청 전에 `tool_call` 로 `skill` 을 호출해 결과를 대화 기록에 넣는다(`tool_call_id` 는 `skill-mention-…`). 거절·오류면 건너뛴다. [`skills.md`](skills.md) |
| `user_message` | `message_id`, `text`, `mode: "queue"\|"steer"\|"now"` | 유휴면 새 런 시작. 실행 중이면 `steer`/`now` 는 다음 모델 요청 직전에 주입, `queue` 는 현재 런의 작업이 끝난 뒤 처리 |
| `abort` | — | 현재 런 취소. 유휴면 무시 |
| `tool_result` | `rpc_id`, `outcome` | `tool_call` 응답(§6) |
| `credential_response` | `id`, `request_id`, `lease_id`, `generation`, `account{source,account_id}`, `auth{kind,value,upstream_account_id?,expires_at_ms?}` | 시도별 비공개 자격 증명 리스. 모델에는 일절 노출되지 않음(§6.1) |
| `credential_error` | `id`, `request_id`, `code`, `reason`, `retry_at_ms?`, `generation` | 고정·정제된 자격 증명 거부 응답. 로컬 소스 다운그레이드 불가(§6.1) |
| `credential_reported` | `id`, `request_id`, `lease_id`, `generation`, `action`, `retry_at_ms?`, `reason?` | 재취득 전 커밋된 리포트 반영을 확인(§6.1) |
| `ticket_resolved` | `ticket_id`, `tool_call_id`, `resolution` | 보류 호출 해결(§7) |
| `subagent_cancel` | `subagent_id` | 서브에이전트 취소(§10). 모르거나 이미 끝난 서브에이전트면 오류 없이 무시 |
| `discard_deferred` | — | 복원한 보류 이어하기(continuation)를 실행하지 않고 버린다. 보류 호출·`delegate` 대기·미종료 서브에이전트·`deferred_messages` 를 지우고, 히스토리는 답이 없는 tool-call batch 앞까지 자른 뒤 `state_snapshot` 을 보낸다(§8). 실행 중이면 무시 |
| `subagent_steer` | `subagent_id`, `text`, `control_id` | 자식 에이전트가 다음 모델 요청을 보내기 전에 지시 사항을 사용자 메시지(user message)로 주입한다. 전달 확인(ack)은 `control_id`로 대응시키며, 대상을 알 수 없거나 이미 종료된 자식 에이전트는 `delivered:false`로 확인을 반환한다. |
| `shutdown` | — | 진행 중 런 취소 → 마지막 `state_snapshot` → `bye` → 종료(0). 데몬은 5초 후 프로세스 그룹 SIGKILL |

`ToolSpec = {name, description, parameters(JSON Schema object), approval: "read"|"write"|"exec"|"sensitive", deferrable: bool}`. 스키마는 데몬의 `schemars` 출력이 유일한 원본이다(사이드카에서 이중 정의 금지).
`model.id`와 `fallback[]`은 명시적으로 설정된 `provider:model` 식별자다. `config.credentials`에는 비밀값이 아닌 요청 구성 옵션만 포함된다. 소스 설정, 자격 증명 경로, Vault 키, 브로커 베어러 토큰은 데몬에만 남는다. SDK 클라이언트는 추출된 계정 자격 증명이 아니라 플레이스홀더 키만 유지한다.
`resume_state = {format:"openside-agent-state/1", data:"<불투명 문자열>"}`.

선택 필드인 `open.team`은 `{role: "chief"|"teammate", agent_id, persona, roster}` 구조다. 생략하거나 null로 지정하면 기존 레거시 세션 방식으로 동작한다. 양쪽 모두 1,500 유니코드 문자를 초과하는 persona 문자열이나 2,000자를 초과하는 roster 문자열을 거부하며, 임의로 자르지 않는다. 데몬은 이름/persona와 이름/역할만 포함된 roster를 렌더링한다. 사이드카는 이 블록을 그대로 덧붙이고 기존 전면 지시문 대신 신뢰할 수 있는 `[CHIEF]` 또는 `[TEAMMATE]` 지시문 블록을 추가한다. 이 역할 지시문은 persona 길이 제한과 별개로 계산된다. 역할에 맞지 않는 `team_*` 도구 명세가 있으면 치명적인 `tool_gate` 오류와 함께 시작에 실패하며, 데몬은 실행 중에도 권한을 다시 검사한다.

chief는 조율을 총괄한다. 반복 작업에는 teammate를 배정하고, 위임에는 자체 완결형 패킷을 사용하며, 대기나 폴링 대신 실시간 제어(live steering)로 조율한다. 판단이 필요한 질문은 chief를 거쳐 회신하고, 중요한 작업에는 독립적인 리뷰어를 배치하며, 소유권 이전 시 사람에게 앞으로 해당 영역을 누가 맡는지 안내한다. teammate는 담당 영역을 책임지며, `team_ask_chief`로 결정을 묻고 `team_report`로 근거를 보고한다. 두 역할 모두 사람의 명시적 승인 없이 배포하거나 게시할 수 없다.

## 5. 사이드카 → 데몬
| type | 필드 | 의미 |
| --- | --- | --- |
| `hello` | `protocol:1`, `nonce`, `pid`, `versions{python, pydantic_ai, pydantic_ai_harness, openside_agent}` | 접속 직후 1회 |
| `ready` | `tools[]`(실제 노출 이름), `pending_deferred[{tool_call_id, ticket_id, kind}]`, `startup_ms` | `open` 처리 완료. 데몬은 `tools` 집합이 `open.tools` 이름 집합과 **정확히 같지 않으면** 세션 시작을 거부(`tool_gate`) |
| `event` | `event{kind, ...}` | 관측 이벤트(§5.1). 데몬이 세션 이벤트 로그로 정규화 |
| `message_accepted` | `message_id` | 메시지가 대화 히스토리에 들어감(모델 요청에 포함) |
| `message_result` | `message_id`, `outcome: completed\|aborted\|failed\|deferred`, `reason?` | 메시지를 포함한 런의 종료. 런에 들어간 모든 메시지마다 1회. `deferred` 로 끝난 메시지는 재개 런이 끝날 때 최종 결과(`completed`/`aborted`/`failed`)를 한 번 더 받는다. 재개 런이 다시 보류되면 `deferred` 를 반복해서 보내지 않는다 |
| `tool_call` | `rpc_id`, `tool_call_id`, `name`, `args`, `subagent?`, `approval_ticket?` | reverse RPC. 동시에 여러 개 가능. 데몬은 같은 `rpc_id` 로 `tool_result` 를 1회 보낸다 |
| `credential_request` | `id`, `request_id`, `provider`, `model`, `attempt`, `subagent?` | 연결에 바인딩된 데몬 세션에서 새 시도용 자격 증명 발급(§6.1) |
| `credential_report` | `id`, `request_id`, `lease_id`, `outcome`, `stream_started`, `retry_after_ms?`, `scope`, `usage?`, `usage_windows[]` | 리스에 바인딩된 분류 결과, 관측된 토큰 수, 정제된 사용량 윈도우(§6.1) |
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

어시스턴트의 첫 번째 응답이 완료되면 `session.title_proposed{title}`이 발행된다. 사이드카는 자체 백그라운드 작업에서 도구를 사용하지 않는 별도의 제목 생성 요청을 보낸다. 이 요청은 최대 48개 출력 토큰과 10초 제한 시간을 가지며, 완료 후 스냅샷에 `title_attempted`를 기록한다. 제목 생성 작업 때문에 `message_result`, `settled`, 후속 메시지, 세션 종료가 지연되지 않으며, 제목 제안 이벤트는 `settled` 이후에 도착할 수도 있다. 세션이 종료되면 제목 생성 작업은 취소되고 회수된다. 제목 생성에 실패하더라도 어시스턴트 응답이 실패 처리되지는 않는다. 데몬은 최대 6단어로 정규화된 제목만 수락하며, URL 형태는 절대 허용하지 않는다. 세션 생성 시 명시적으로 지정한 제목이나 사용자가 `session.updated`로 변경한 제목이 항상 우선하며, 변경된 제목이 현재 자동 생성 제목과 같더라도 마찬가지다. 제목 생성 요청은 별도의 `usage` 이벤트를 발생시키며, 화면에 보이는 대화 내용이나 스크립트 커서는 변경하지 않는다.

데몬은 제목을 이미 사용자가 직접 설정했거나 제안된 제목이 수락된 경우 비공개 설정인 `open.config.agent.title_needed`를 false로 변경한다. 그러면 사이드카는 제목 생성 요청을 보내지 않으며, 이는 이전 스냅샷이 저장되기 전에 비정상 종료된 경우에도 마찬가지다. 스크립트 픽스처에서 해당 항목이 생략되면 기본값은 true로 지정된다. 이미 진행 중인 제목 생성 요청이 있더라도 사용자가 제목을 변경하면 데몬 리듀서에서는 항상 사용자가 변경한 제목이 우선 적용된다.

생성된 제목, 사용량, 이름 변경, 고정(pinning) 같은 메타데이터는 태스크 종료 마커 뒤에 전달될 수 있다. 실행 중이지 않은 세션에서 새로운 텍스트 없이 `openside session resume`을 실행하면 확인된 `Session.last_seq`까지 수신되지 않은 모든 이벤트를 다시 재생한 뒤 반환하며, 마지막 이벤트가 `session.settled`일 것이라고 단정하지 않는다. 새 메시지가 있거나 실행 중인 세션인 경우에는 여전히 새로운 종료 이벤트를 대기한다. 재생된 모든 이벤트는 저장된 커서 위치를 전진시키므로, 이후 다시 resume을 실행하더라도 메타데이터가 중복 재생되지 않는다.




## 6. 툴 호출(reverse RPC)
`tool_result.outcome`:
- `{"status":"done","for_model": <JSON 값>}` → 툴 반환값.
- `{"status":"deferred","ticket_id":"…","kind":"approval"}` → 사이드카는 `ApprovalRequired(metadata={ticket_id})`. 정책 `Ask` 의 경로.
- `{"status":"deferred","ticket_id":"…","kind":"external"}` → `CallDeferred(metadata={ticket_id})`. `request_human_step`, `ask_user` 의 경로.
- `{"status":"denied","reason":"…","hint":"…"|null}` / `{"status":"error","message":"…"}` → `ModelRetry`(이유와 대안 문장). 재시도 한도는 사이드카 설정(기본 3).
사이드카는 동일한 `tool_call_id`에 `approval_ticket`을 실어 재호출할 수 있다. 데몬은 세션, 호출 ID, 도구, 호출 주체(부모 또는 같은 `subagent`)가 일치하는지, 그리고 승인된 `Confirm`으로 해결되었는지 검증한다. 데몬은 `Ask` 확인 티켓을 열 때 화면 표시용으로 가공된 인자가 아니라 원본 인자 전체와 권한(capability) 및 대상을 포함한 정책 동작(policy actions) 전체를 재귀적으로 키 정렬한 canonical JSON의 SHA-256 해시를 내부에 영구 보관한다. 재호출이 성립하려면 이 바인딩이 일치해야 하며, 도구 실행 전에 해당 승인을 원자적으로 영구 보관 장치에 `consumed`로 표기해야 한다. 인자나 정책 동작의 권한 및 대상이 불일치하면 `approval_args_mismatch`로 거부하고, 이미 사용된 승인은 `approval_already_used`로 거부한다. 명시적 `Deny`는 여전히 우선권을 가진다. 선점 표기 후 크래시가 발생했다면 해당 승인은 재시도할 수 없다.

### 6.1 Credential RPC

비공개 자격 증명 채널은 `tool_call`과 독립적이며 모델 툴 게이트에 절대 노출되지 않는다. 요청에는 길이 제한이 있는 불투명 상관관계 `id`와 논리적 `request_id`를 사용한다. `request_id`는 재시도 전반에서 바뀌지 않는다. `attempt`는 0부터 63까지의 0 기반 인덱스로, 업스트림 시도는 최대 64회까지 허용된다. 선택 항목인 `subagent`는 데몬이 인식하는 자식 ID로 해당 Agent 제약을 적용하는 데 쓴다. 어떤 프레임도 다른 세션 ID를 지정할 수 없다. 유효한 정렬 계정 풀과 웜 세션 어피니티의 소유권은 오직 데몬에만 있다.

`auth.kind`는 `api_key`, `claude_code`, `codex_cli`, `bearer` 중 하나다. 유일한 비밀값은 `auth.value`다. Codex는 요청 헤더에 필요한 비공개 업스트림 계정 ID도 함께 전달받을 수 있다. 리프레시 토큰이나 자격 증명 홈 디렉터리 필드는 허용되지 않는다. Python은 현재 시도에 필요한 자체 가변 토큰 버퍼만 보관하고 `finally` 블록에서 이를 소거하며, 선택된 프로바이더 오리진에만 인증 정보를 주입한다. Codex는 SDK의 자격 증명 파일 로드와 OAuth 갱신을 건너뛰도록 `openai_client`를 주입한다. SDK 수준 재시도는 비활성화하여 데몬 정책을 우회한 채 동일 자격 증명으로 다시 전송하지 못하도록 막는다.

결과(outcome) 종류는 `success`, `auth`, `rate_limit`, `quota`, `policy`, `transient`, `cancelled`다. 세부 정보가 없는 단순 429는 계정 레이트 리밋이고, 구조화된 쿼터 코드는 쿼터 소진이며, 일반 401은 인증 실패, 그 외 403 권한 오류는 정책 거부다. 입증된 분당/동시성 한도 초과나 전송/5xx 실패는 일시적 오류(transient)로 분류한다. `retry_after_ms`는 프로바이더가 알린 유한하고 제한된 대기 시간만 담으며, 재시도/초기화 시각 정보가 없으면 임의로 60초를 채우지 않고 알 수 없음(unknown)으로 둔다. 범위는 구조화된 쿼터/정책 데이터로 `model` 범위임이 입증되지 않는 한 `account`다. 리포트에는 원시 응답 본문, 헤더, 예외 텍스트, 업스트림 식별자를 일절 포함하지 않는다.

`credential_reported.action`은 `complete`, `refresh`, `rotate`, `stop` 중 하나다. 데몬은 계정, 리비전, 리스 변경 사항을 먼저 커밋한 뒤 확인(ack)을 보낸다. Python은 실패 확인을 받은 후에야 동일한 논리 ID와 다음 시도 번호로 자격 증명을 다시 요청한다. 오직 데몬의 `refresh`나 `rotate` 지시가 있을 때만 재전송이 허용된다. 스트리밍 컨텍스트가 한 번이라도 스트림을 호출자에게 넘겨준 뒤에는 실패하더라도 해당 요청을 다시 전송하거나 로테이션하지 않는다. 취소/중단/종료 시에 한해 Python은 확인을 기다리지 않고 `cancelled` 리포트를 전송하며 늦게 도착하는 응답은 무시해 셧다운 디스패치 데드락을 방지한다.

거부되거나 풀이 소진·차단되면 고정된 기계 판독 사유와 신뢰할 만한 재시도 시각을 담아 타입화된 `AuthUnavailable` 오류를 발생시킨다. 기본 원칙은 닫힌 채로 실패한다(fail-closed). 목록에 없는 소스로의 다운그레이드는 허용되지 않는다. G-2 규정에 따른 혼합 소스 계정 로테이션은 사람이 명시한 유효 풀 내부에서만 동작한다. 다른 프로바이더로의 변경은 명시적으로 구성된 폴백 모델이 필요하며, 인증 불가(`AuthUnavailable`)는 폴백 조건이 아니다. 사이드카 스냅샷은 더 이상 자격 증명 pin을 보관하지 않으며, 레거시 `pins`가 포함된 기존 스냅샷은 해당 키를 버리고 정상 디코딩한다.

데몬이 실행한 사이드카는 신뢰받는 실행 코드이며 공격 성향을 지닌 보안 주체로 취급되지 않는다. 비공개 소켓과 nonce는 해당 프로세스를 인증하고, 리스 및 최대 64회 시도 규칙은 상호 협력 프로토콜 요청을 제어한다. 즉, 보안이 손상된 사이드카가 복사된 업스트림 토큰을 RPC 외부에서 사용하거나 분류된 결과/사용량 리포트를 조작하는 행위 자체를 방지하지는 못한다. 적대적 사이드카를 완전히 격리하려면 데몬 소유의 업스트림 아웃바운드 제어나 독립 검증된 상태 증명이 필요하다.


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

스크립트 수준의 선택적 `title` 문자열은 스크립트 전용 별도 제목 모델을 지정하며, 기본값은 `Discuss the request`다. 제목 생성은 `responses` 항목을 소모하지 않고 실제 프로바이더를 호출하지도 않으며, 1회 실행 여부는 `title_attempted`를 통해 복원된다.

- `echo`: 가장 최근에 입력된 사용자 프롬프트를 그대로 출력한다.
- `{{result:call.field.path}}`: substitutes a returned tool field or numeric array index inside string arguments, recursively through nested objects and arrays.
- `script.subagents`: `create_task` 및 `delegate` 호출 ID를 하위 에이전트 응답 목록과 매핑하며, 하위 작업마다 독립적인 커서와 steer 이벤트를 관리한다.
- Scripted front fixtures use explicit `send_message` or `stay_silent` output tool calls. `{{user_text}}` and `{{media_digest}}` in tool arguments supply the latest user text or media digest without enabling plain-text front output; worker `echo` and `media_digest` response parts remain unchanged.

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
- The front tool surface is `send_message(text, attachments?)`, `create_task(goal, model?, tools?)`, `cancel_task(task_id)`, `get_tasks()`, `steer_task(task_id, message)`, `get_models()`, and the existing `team_*` delegation tools. There are no legacy aliases.
- The daemon denies all other front calls with `front_agent_cannot_act`, including browser reads, memory reads and `present`. Tasks do the work and raise person-facing Cards.
- The front ends each turn through one output tool: `send_message{text, attachments?}` publishes one complete `assistant.final{text, attachments?}` through daemon validation, or `stay_silent{}` ends without a bubble. Plain-text output is disabled; front narration, deltas and thinking never become chat events. Work-management tools remain ordinary calls before the terminal output. At most one message is supported per turn; a rejected output is retried without claiming delivery. Messages contain only a useful answer, a question needing the person, or a result; routing, model selection and task bookkeeping stay silent. Long answers use up to two Markdown Output attachments `{artifact, display:"inline"|"panel", title?}`: inline for brief tables/summaries, panel for long reports. The event carries only `{path,title,display,bytes,content_type}` metadata; clients fetch admitted bytes through the session artifact route.
- Front-only tools are not delegated to children. `create_task` returns immediately with a `task_id`; lifecycle remains `subagent.started` (`background: true`), `subagent.lifecycle` and `subagent.ended`.
- Tasks use `write_document{title,markdown}` to produce a Markdown Output artifact without a workspace. Its result contains `artifact`, `title`, `bytes`, and `content_type`; `get_tasks` includes each Task's document paths in `artifacts`. Documents share the chief session's artifact store.
- Child tool activity is recorded as `subagent.progress{subagent_id, progress:{kind:"tool.started"|"tool.ended", ...tool_event_fields, status_text}}`, not parent-level tool rows. `status_text` is a short past-tense line; browser completions include only the hostname from the daemon's typed result URL, never its path, query, fragment or input arguments. Task detail and History render these steps; the chief thread renders only a compact Task status. Person-facing Cards retain their top-level events and schema-based privacy filtering; Cards are not copied into the untyped progress payload.
- Admission routing fields (`subagent_id`, `model`, `tools`, `context`) are daemon-issued protocol data, not redactable content. Unknown lifecycle identities produce a diagnostic and reconcile against the sidecar's explicit `known_subagents` live inventory; IDs are never guessed. Restored children with redaction markers in routing fields fail with `corrupt_child_identity` and cannot resume pending actions.
- Failed lifecycle reports carry optional `error_code` and `diagnostic`. The sidecar asks the daemon's internal `__redact_diagnostic` RPC to sanitize exception detail before retaining it in child state. The daemon redacts again before logging and storing the primary code in `reason` and diagnostic in `output`; subsequent recovery cannot overwrite a terminal primary failure.
- Session status remains running while foreground work or any child remains active. Foreground settlement alone does not imply an idle session; the final child transition clears activity coherently.

### 대기열 처리 및 제어(Steer) 동작
- 데몬은 백그라운드 작업이 실행되는 동안 `session.settled` 이벤트 전달을 보류한다. 다만 Front 에이전트가 유휴 상태(idle settled)가 되면 대기열에 쌓인 사용자 메시지를 바로 다음 작업으로 넘겨 처리한다. 즉, 하위 작업 티켓이 Front 대기열을 가로막지 않는다.
- `steer_task`는 이미 돌고 있는 블로킹 도구를 강제로 끊지 않고, 다음 모델 요청 직전에 전달된다.
- 호스트의 `steer_task` 응답이 `pending` 상태인 것은 요청이 초기 진입 단계에서 수용(admission)되었을 뿐 아직 대상에 전달이 완료되지 않았음을 의미한다. 사이드카는 지시 사항을 정상적으로 수락했을 때만 `steered`를 반환하며, 작업 완료 처리가 우선하면 `task_finished` 거부를 반환한다.
- Task completion reports use durable `task-report-<task_id>` message IDs on the queue lane. Startup repairs a crash between Task completion and report creation.
- Sidecar `settled` signals front idleness, including after a background Task finishes or resumes from a ticket. The daemon uses it to dispatch queued reports, but publishes `session.settled` only when no Task is running and the durable message queue is empty.
- Resolving the last pending ticket for a deferred agent call projects the session as `running` immediately, before the resumed runtime emits `turn.started`. Resume clients must not mistake the ticket-resolution boundary for a settled answer. Standalone API tickets with no agent continuation remain idle.
- Completed, failed and waiting-for-person transitions automatically wake the front without another person message. Waiting notifications use `task-report-<task_id>-waiting-<ticket_id>` so each pending request is delivered once, including across restart. Reports contain the Task id, title, state and at most 1,024 characters of result or needs-you reason; full Task output remains in its event record. The front decides silently whether a useful answer or question needs `send_message`.
- Task 완료 인도의 정확히 한 번(exactly-once) 보장 대상은 영속화된 'Task-finished' 메시지 행(`task-report-<task_id>`)과 이벤트 로그에 보존되는 작업 출력(`subagent.ended.output`)이다. Front가 생성하는 텍스트 요약은 최선 노력(best-effort)으로 동작한다. 리포트 주입(`MessageInjected`) 후 `assistant.final` 전에 사이드카가 종료되더라도 해당 리포트는 실패 또는 중단(`failed`/`interrupted`)으로 안전하게 마킹되어 재실행되지 않으며, 영속화된 행과 작업 출력은 그대로 유지된다.
- Authenticated person provenance selects the steer lane regardless of the client's requested mode. Agent clients, channels, routines and internal reports cannot promote themselves; they wait behind the current owner turn. Steering is injected before the next model request and never cancels an in-flight generation or host mutation.
- Steering is automatic from ordinary person messages: the front chooses `steer_task` for corrections or clear additions to live work, `create_task` for a new request, `cancel_task` for a stop request, and `send_message` for chat. There is no person-facing steering mode. “Actually make it weekly” updates the running research Task; “also check Naver” starts a new Task unless it clearly extends the existing work.
- Idle non-owner bursts wait in the daemon's durable queue for a fixed 100 ms window (not extended by new arrivals). Each dispatch takes at most 32 messages and 1 MiB of message text. The sidecar coalesces delivery for another fixed 100 ms, retaining individual acceptance and completion events; an owner arrival bypasses both waits and leaves reports queued behind that owner turn.
- Restart re-delivers only messages never injected into the runtime. Injected messages without terminal results are marked failed with `reason:"interrupted"`, never silently replayed. One internal interruption report covers the interrupted batch and tells the chief to reconcile uncertain effects before retrying. Its durable ID uses the first interrupted message ID in lexical order; it is queued before terminal results, and an already-pending recovery report is reused if recovery itself restarts.
- `user_message.author_kind` carries `person`, `agent_client`, `channel` or `internal`; omitted fields default to `person` for the sidecar wire only. The daemon always supplies the authenticated value. Non-person messages become a separate provenance metadata content part followed by the original text and attachments, and that structured content survives snapshots. A forged non-person `mode:"steer"` is still queued by the sidecar.

### 브라우저 탭 격리 및 상태 표현
- Tasks share the session browser workspace. Another live Task's tab cannot be acted on (`tab_owned_by_task`); Task read-only observations do not claim ownership. The front cannot read or act on tabs and must delegate browser work.
- Ended Tasks close their unreferenced agent-owned tabs. Pending person steps and Cards retain their referenced tabs; person-owned tabs are never closed. Failed closes retain the Task's ledger reservation for later cleanup, and only successful browser closure emits a closed event.
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

## 공유 Card

`present`는 데몬 카탈로그가 제공하며 기존 도구 RPC를 통해 실행된다. 사이드카는 비공개 UI 도구를 직접 실행하지 않는다. 호출에 성공하면 `{card_id}`만 반환하며, `card.presented`는 `present:<tool_call_id>` 아래에 표준 프레젠테이션을 단 한 번 영속화한다. 유효하지 않은 초안은 도구 성공 반환값이나 프레젠테이션 이벤트 없이 재시도 프롬프트를 받는다.

사람 개입 단계(human-step) 도구는 원래의 도구 호출 ID를 계속 보류하고 재개한다. 사람 대상 Ticket/Card 메타데이터와 응답 내용은 모델 컨텍스트로 전달되지 않는다. 양식/OTP/로그인 결과는 실제 입력값을 담지 않으며, 페이지/양식 응답은 `state_snapshot`에 절대 들어가지 않는다. 터미널 수명주기 이벤트는 교체 Card를 원자적으로 전달하며, Card가 누락된 과거 이벤트는 API 재생 시 데몬 프로젝터가 채워 넣는다.
