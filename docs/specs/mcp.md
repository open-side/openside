# MCP 서버 — `openside mcp`

Claude Code, Codex, omp 같은 MCP 클라이언트가 openside 데몬을 도구로 쓰게 하는 stdio 서버다. 별도 크레이트가 아니라 CLI(`crates/openside-cli/src/mcp.rs`)의 서브커맨드다. CLI의 `Client`를 그대로 쓰므로 소켓·토큰 해석과 `openside_proto::ROUTES` 검사가 CLI와 같다. 구현은 공식 Rust SDK `rmcp`를 쓴다.

브라우저는 두 가지 방식으로 쓴다. (1) openside 브라우저 에이전트에 목표를 위임하거나(`agent_*` 도구), (2) 에이전트와 같은 도구 및 여러 단계를 한 번에 묶어 실행하는 `browser_repl`을 써서 세션 브라우저를 직접 제어한다. CLI에도 `openside run`과 `openside browser` / `openside repl`이라는 같은 두 경로가 있다(`docs/reference/cli.md` 참고).

## 연결

`mcp.json`(또는 각 클라이언트의 MCP 설정)에 다음처럼 등록한다.

```json
{
  "mcpServers": {
    "openside": {
      "command": "openside",
      "args": ["--url", "http://127.0.0.1:<포트>", "mcp"],
      "env": { "OPENSIDE_TOKEN": "<기기 토큰>" }
    }
  }
}
```

- 연결 방식은 다른 CLI 명령과 같다. `--socket`(기본 소켓) 또는 `--url` + `--token`/`OPENSIDE_TOKEN`이다. 릴레이, Tailscale, Cloudflare Tunnel로 페어링한 원격 데몬에는 `--remote <NAME>` 또는 `OPENSIDE_REMOTE=<NAME>`을 쓴다(예: 인자 `["--remote", "home", "mcp"]`). MCP 서버는 로컬에서 돌며 모든 호출을 원격 데몬으로 전달한다.
- 권한은 연결한 토큰의 범위가 정하며, 검사는 데몬이 한다. 유닉스 소켓은 로컬 사용자라서 암묵적으로 `admin`이므로, MCP 클라이언트에는 `openside token create --scope …`로 만든 최소 범위 토큰을 `--url`로 연결하는 편이 안전하다. 원격 호출자는 TCP 호출자로 다룬다. 에이전트 실행, 브라우저 도구, REPL, 티켓은 페어링된 토큰 범위(`steer`, `browse`, `approve`, `human_step`) 안에서 동작한다. 정책, 설정, 모드 확대만 로컬 소켓 전용으로 남는다.
- `--json`은 효과가 없다. stdout은 MCP 프로토콜 전용이고, 진단은 stderr로 나간다.
- 서버는 상태를 갖지 않는다. 모든 도구 호출은 `/v1/*` 공개 API 요청으로 바뀌며, ROUTES에 없는 요청은 프로세스 밖으로 나가기 전에 거부된다.

## 도구

| 도구 | API | 필요한 범위 |
|---|---|---|
| `agent_run` | `POST /v1/sessions`(`session_id` 없음), `POST …/messages`, `wait=true`면 아래 대기 | `steer`(`wait=true`면 `read`도) |
| `agent_result` | `GET …/events`(SSE), `GET /v1/tickets`, `GET …/subagents` | `read` |
| `agent_status` | `GET /v1/sessions/{id}`, `GET …/subagents`, `GET /v1/tickets` | `read` |
| `agent_steer` | `POST …/messages`(`mode: steer`) 또는 `POST …/subagents/{sid}/steer` | `steer` |
| `agent_cancel` | `POST …/stop` 또는 `POST …/subagents/{sid}/cancel` | `steer` |
| `session_create` | `POST /v1/sessions` | `steer` |
| `session_list` | `GET /v1/sessions` | `read` |
| `session_status` | `GET /v1/sessions/{id}` | `read` |
| `session_send` | `GET /v1/sessions/{id}`, `POST …/messages`, `wait=true`면 아래 대기 | `steer`(`wait=true`면 `read`도) |
| `session_wait` | `GET /v1/sessions/{id}`, `GET …/events`(SSE), `GET /v1/tickets` | `read` |
| `ticket_list` | `GET /v1/tickets` | `read` |
| `ticket_resolve` | `GET /v1/tickets/{id}`, `POST …/resolve` | `read` + `approve` |
| `ticket_wait` | `GET /v1/tickets/{id}`(폴링) | `read` |
| `memory_search` | `GET /v1/memory` | `read` |
| 데몬 브라우저 도구(`browser_tabs`, `browser_open_tab`, `browser_navigate`, `browser_snapshot`, `browser_act`, `browser_pointer`, `browser_screenshot`, `browser_close_tab`, `request_login`, `request_choice`, `wait_for_approval`, `request_otp`, `register_authenticator`, `request_human_step`, `ask_user`, `vault_login`, `vault_otp`, `vault_passkey`) | `GET /v1/tools`, `POST /v1/sessions/{id}/tools/{name}` | `browse`(`vault_*` 도구는 `vault_use`도 필요) |
| `browser_repl` | `POST /v1/sessions/{id}/repl` | `browse` |

`steer`는 `read`를 포함하지 않는다. 메시지를 보내고 답을 받으려면 두 범위가 모두 필요하다. `steer`와 `admin`은 `browse`를 포함한다.

## 위임 실행

- `agent_run {goal, session_id?, wait?, timeout_secs?, model?, mode?, browser_space?}`는 웹 입력창과 똑같이 목표를 새 세션(또는 `session_id`)에 메시지(`mode: now`)로 보낸다. 실제 작업은 데몬 에이전트가 맡는다(앞단 오케스트레이션에서는 앞단 에이전트와 그 에이전트가 띄운 백그라운드 Task가 함께 수행한다). 이 실행은 일반 세션 실행과 같아서, 인앱 실행과 동일한 이벤트·티켓·감사 기록이 남고 웹과 모바일 UI에서도 실시간으로 보인다.
- `wait`를 주지 않으면 `session_id`, `message_id`, `after_seq`를 즉시 돌려준다. `agent_result {session_id, after_seq, message_id?, timeout_secs?}`는 해당 실행이 끝날 때까지 기다린 뒤 `replies`, `tasks`, `pending_tickets`를 돌려준다. `wait=true`면 `agent_run`이 이 과정을 직접 처리한다.
- 기다리는 동안 클라이언트가 진행률 토큰(`_meta.progressToken`)을 보냈다면, 서버는 Task 시작·종료, 브라우저 활동(`browser.state`), 티켓 생성·응답마다 한 줄씩 `notifications/progress`를 보낸다. 진행률 알림을 지원하지 않는 클라이언트에는 최종 결과만 돌아간다.
- `agent_status {session_id}`는 기다리지 않고 세션 상태, `tasks`(`task_id`, `task`, `status`, `background`, 최근 진행 상황 `last_progress`, 결과 `output`), `pending_tickets`를 즉시 돌려준다.
- `agent_steer {session_id, text, task_id?}`는 `task_id`가 있으면 해당 Task의 진행 방향을 바로잡고, 없으면 세션 에이전트에 `steer` 메시지를 보낸다. `agent_cancel {session_id, task_id?}`는 `task_id`가 있으면 해당 Task를 취소하고, 없으면 세션 전체 실행을 중지한다.
- 에이전트가 시작한 파일 다운로드는 실행 이벤트(`tool.updated`/`tool.ended` 안의 `browser_event` `download.started`/`download.finished`)로 나타난다. 별도 다운로드 도구는 없다.

## 세션 대기

`session_send`에 `wait=true`를 주면, 메시지를 보내기 직전의 `last_seq`(`after_seq`)부터 이벤트 스트림을 읽어 세션이 멈출 때까지 기다린 뒤 에이전트의 답(`replies`)을 돌려준다. `session_wait`는 같은 대기를 따로 부른다. `agent_run`과 `agent_result`도 같은 대기 로직과 같은 상태 값을 쓴다.

결과의 `status`는 다음 중 하나다.

- `settled`: 세션이 끝났다. `replies`에 `assistant.final` 텍스트가 순서대로 들어 있다.
- `waiting_human`: 사람의 티켓을 기다린다. `pending_tickets`에 `id`, `kind`, `summary`가 있다.
- `stopped`: 세션이 중지됐다.
- `error`: `runtime.error`가 왔다. `error`에 코드와 메시지가 있다.
- `timeout`: `timeout_secs`(기본 120초, 최대 3600초)가 지났다. 오류가 아니다. 돌려준 `last_seq`를 `after_seq`로 주고 `session_wait`를 다시 부르면 이어서 기다린다.

MCP 대기는 티켓 완료까지 붙잡고 있지 않고 `waiting_human` 상태에서 곧바로 반환된다. 이때는 `ticket_wait`를 호출한 뒤, 돌아온 `last_seq`를 `after_seq`로 넘겨 `agent_result`를 다시 호출하면 된다.

`session_send`로 기다릴 때는 먼저 이 메시지가 실제로 디스패치된 것(`message.injected`)을 확인하고, 그 뒤의 답(`assistant.final`)만 `replies`에 담는다. 이 메시지의 완료(`message.completed`/`message.dropped`) 뒤에 오는 정착만 종료로 본다. 앞서 대기 중이던 메시지의 답이나 정착을 이 메시지의 결과로 착각하지 않는다(요청의 `message_id`는 멱등 키일 뿐이다).

`session_wait`는 먼저 세션을 조회한다. `after_seq`가 없으면 현재 `last_seq`부터 기다린다. 실행 중이거나 대기열에 든 일이 없고 `after_seq`가 `last_seq`와 같으면 스트림을 열지 않고 현재 상태를 바로 돌려준다. `after_seq`가 `last_seq`보다 크면 오류다. 실행 중인 세션은 조회 시점 이후의 정착만 결과로 치고(이전 실행의 정착은 기록일 뿐이다), 멈춰 있는 세션은 마지막 정착까지 다시 읽어 `replies`에 담는다. `pending_tickets`는 이벤트 재생 전이 아니라 대기가 끝난 시점에 조회한다. 정착했는데 대기 중인 티켓이 남아 있으면 `waiting_human`이다.

## 티켓

`ticket_resolve`는 `secret_input`과 `login` 티켓을 거부한다. 거부는 해결 요청을 보내기 전에 일어나며 값은 데몬에 아예 전달되지 않는다. 비밀값은 사용자가 `openside ticket resolve <id> -`로 파이프에 넣어 직접 전달해야 하고, 로그인 정보는 웹 UI 카드로 입력한다. MCP는 티켓 거절이나 사람 개입 단계 완료(`done`)를 처리하는 전용 도구를 제공하지 않으며, 확인 티켓 거절은 `ticket_resolve`에 `false` 또는 `"reject"`를 전달하여 처리한다.

`ticket_wait {ticket_id, timeout_secs?}`는 웹이나 모바일 UI에서 사람이 응답할 때까지 상태를 폴링하며 `status`, `kind`, `summary`, `resolution`을 돌려준다. 타임아웃이 지나면 `status: pending`을 돌려준다.

티켓 승인 및 사람의 판단 권한은 지침 권고가 아니라 서버가 직접 강제하는 규칙이다. 페어링 역할은 운영자가 오퍼나 토큰을 만들 때 결정된다. 에이전트용 CLI와 MCP는 `openside pair --agent`로 만든 오퍼를 `openside connect`로 교환하거나 `openside token create`(기본값 `agent_client`)로 토큰을 발급받아 등록한다. 일반 `openside pair`나 로컬 소켓 전용 `openside token create --person`은 `person_device` 자격 증명을 발급하며, `person_device` 토큰을 에이전트에 주면 사람의 승인 권한이 넘어가므로 절대 에이전트에 사용해서는 안 된다. 로컬 소켓의 식별자는 `local`(`agent_client`)이다. MCP의 `ticket_resolve` 도구는 확인 티켓 승인(`true` 또는 `"approve"`)과 모든 `choice` 티켓 응답을 요청 전송 전에 미리 거부하며, 페어링된 `person_device`에서 사용자가 응답하도록 안내하고 `ticket_wait`로 대기하라는 `person_approval_required` 메시지를 돌려준다(사전 거부는 로컬 검사 결과이며 데몬에 직접 금지된 API나 CLI 요청을 보낼 때 비로소 실제 HTTP `403`이 반환된다). 또한 `ticket_resolve`는 모든 `secret_input`과 `login` 티켓도 사전에 거부하여 값을 데몬에 전달하지 않는다. 반면 비밀값이 아닌 일반 정보(`info`) 응답과 확인 티켓 거절(`false` 또는 `"reject"`)은 `ticket_resolve`로 처리할 수 있다. 역할 정보가 없는 기존 외부 에이전트 프로필은 `openside pair --agent`로 새 오퍼를 만들어 `openside connect`로 다시 페어링해야 한다.

외부 호출자(에이전트 실행 또는 직접 도구 호출)가 만든 티켓도 일반 티켓과 같다. 다른 티켓과 마찬가지로 웹 UI, 모바일 앱, 알림 채널에 그대로 뜬다.

## 브라우저 직접 제어

도구 목록(`tools/list`)은 호출할 때마다 데몬의 카탈로그(`GET /v1/tools`)를 읽어 오므로, 도구 이름과 인자 스키마는 인앱 에이전트가 쓰는 것과 항상 같다. 각 도구는 데몬 고유 인자에 더해 `session_id`(필수), `approval_ticket`, `wait`, `timeout_secs`를 받는다. 카탈로그를 읽지 못하면 `tools/list`가 실패하며, 오래된 목록으로 대신하는 예외 처리는 하지 않는다.

도구 호출은 `POST /v1/sessions/{id}/tools/{name}`을 거치며, 에이전트 호출과 똑같은 파이프라인을 지난다. 즉 프로필 가시성, 정책 모드, 시크릿 모드 vault 거부, 오리진 정책, 승인 티켓, 마스킹, 도구 이벤트, 감사가 모두 동일하게 적용된다. 워크스페이스 파일, 셸, 메모리, 스킬, Task 도구는 에이전트 전용으로 남는다.

결과는 `{status, tool_call_id, result, artifacts?}` 형식이다. `browser_screenshot`은 저장된 PNG 경로를 `artifacts`에 담아 돌려준다(`GET /v1/sessions/{id}/artifacts/{path}`로 조회 가능).

사람의 개입 단계와 승인은 기본적으로 사람이 웹이나 모바일 UI에서 응답할 때까지 기다린다(`timeout_secs`, 기본 120초). 승인이 떨어지면 도구는 `approval_ticket`을 넣어 호출을 자동으로 다시 실행한다. 로그인, 코드 입력, 선택지, 사람 개입 단계 티켓은 티켓 해결 결과가 그대로 도구 호출 결과(`status: resolved`)가 된다. `wait=false`를 주거나 타임아웃이 지나면 `{status: "deferred", ticket_id, deferral: "approval"|"external", next}`가 반환된다. 이는 오류가 아니다. 티켓을 승인한 뒤 `approval_ticket`을 붙여 다시 호출하거나 `ticket_wait`를 쓰면 된다. 직접 호출로 생긴 티켓은 세션 에이전트에 넘겨지지 않는다.

`browser_repl {session_id, code, tab_id?, approval_ticket?, wait?, timeout_secs?}`는 세션 탭 안에서 Playwright 문법 스크립트(`docs/specs/browser-repl.md`)를 실행하며, 한 번의 호출로 여러 단계를 묶어 처리할 수 있다. `browser_snapshot`과 같은 ref 체계를 쓴다. 승인 동작은 위와 같다.

정책 거부, 실행 실패, 스크립트 예외는 오류 결과로 반환된다.

## 오류

- 인자가 스키마와 맞지 않거나 알 수 없는 도구면 JSON-RPC `invalid_params` 오류다.
- 데몬이 거절하거나 실패하면(범위 부족 `403 forbidden`, 없는 세션, 정책 거부 등) 호출 자체는 성공하고 결과에 `isError: true`와 데몬 메시지가 담긴다. 토큰 값은 인자 오류와 stderr 진단을 포함해 서버 밖으로 나가는 모든 문자열에서 `[REDACTED]`로 바뀐다.

## 의도적으로 노출하지 않는 것

- 메모리 쓰기·수정(`admin` 범위). API에는 마련되어 있다.
- 워크스페이스 파일, 셸, 스킬 도구. 에이전트 전용으로 유지하며, 대신 `agent_run`으로 위임한다.
- `secret_input` 및 `login` 티켓 해결(‘티켓’ 절 참고).

## 검증

- `crates/openside-cli/src/tests/mcp.rs`(가짜 데몬): 닫힌 스키마를 갖춘 고정 도구 목록과 카탈로그 도구, 대기·티켓 대기·타임아웃, `secret_input` 거부, 오류 매핑, 카탈로그 도구의 도구 경로 호출 및 사람 응답 반환, `browser_repl` 상태 매핑, `agent_run`/`agent_result`/`agent_status`/`agent_steer`/`agent_cancel`, 모든 요청이 ROUTES에 속함을 검증한다.
- `crates/openside-e2e/tests/mcp.rs`: 실제 데몬과 스크립트 사이드카를 stdio 환경의 `openside mcp`에 rmcp 클라이언트로 연결해 검증한다. `initialize` → `tools/list` → `session_send` → 응답 확인, 읽기 전용 토큰의 쓰기 도구 차단, 승인 후 재개, `secret_input` 거부, 로컬 페이지에서 카탈로그 도구를 통한 열기·스냅샷·스크린샷 및 `browser_repl` 배치 1회로 다음 페이지를 클릭·확인(Chromium 사용 가능 시), `agent_run`이 인앱 실행과 동일한 이벤트 순서를 기록하는지 확인한다. 기록 프록시로 데몬이 받은 모든 요청이 공개 경로인지도 검증한다.
- `crates/openside-e2e/tests/mcp_remote.rs`: 릴레이 페어링으로 연결된 데몬을 대상으로 `openside mcp --remote <name>`을 실행해 `agent_run`과 브라우저 도구를 검증한다.
- `crates/openside-cli/src/tests/agent.rs`: 가짜 데몬을 대상으로 `openside run`/`steer`/`cancel`/`tasks`/`browser …`를 검증한다.
