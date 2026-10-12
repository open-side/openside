# MCP 서버 — `openside mcp`

Claude Code, Codex, omp 같은 MCP 클라이언트가 openside 데몬을 도구로 쓰게 하는 stdio 서버다. 별도 크레이트가 아니라 CLI(`crates/openside-cli/src/mcp.rs`)의 서브커맨드다. CLI의 `Client`를 그대로 쓰므로 소켓·토큰 해석과 `openside_proto::ROUTES` 검사가 CLI와 같다. 구현은 공식 Rust SDK `rmcp`를 쓴다.

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

- 연결 방식은 다른 CLI 명령과 같다. `--socket`(기본 소켓) 또는 `--url` + `--token`/`OPENSIDE_TOKEN`이다.
- 권한은 연결한 토큰의 범위가 정한다. 범위 검사는 데몬이 한다. 유닉스 소켓은 로컬 사용자라서 암묵적으로 `admin`이므로, MCP 클라이언트에는 `openside token create --scope …`로 만든 최소 범위 토큰을 `--url`로 연결하는 편이 안전하다.
- `--json`은 효과가 없다. stdout은 MCP 프로토콜 전용이고, 진단은 stderr로 나간다.
- 서버는 상태를 갖지 않는다. 모든 도구 호출은 `/v1/*` 공개 API 요청으로 바뀌며, ROUTES에 없는 요청은 프로세스 밖으로 나가기 전에 거부된다.

## 도구

| 도구 | API | 필요한 범위 |
|---|---|---|
| `session_create` | `POST /v1/sessions` | `steer` |
| `session_list` | `GET /v1/sessions` | `read` |
| `session_status` | `GET /v1/sessions/{id}` | `read` |
| `session_send` | `GET /v1/sessions/{id}`, `POST …/messages`, `wait=true`면 아래 대기 | `steer`(`wait=true`면 `read`도) |
| `session_wait` | `GET /v1/sessions/{id}`, `GET …/events`(SSE), `GET /v1/tickets` | `read` |
| `ticket_list` | `GET /v1/tickets` | `read` |
| `ticket_resolve` | `GET /v1/tickets/{id}`, `POST …/resolve` | `read` + `approve` |
| `memory_search` | `GET /v1/memory` | `read` |
| `browser_open` / `browser_snapshot` / `browser_act` / `browser_repl` | `POST /v1/sessions/{id}/repl` | `browse`(`steer`·`admin`도 포함) |

`steer`는 `read`를 포함하지 않는다. 메시지를 보내고 답을 받으려면 두 범위가 모두 필요하다.

### 세션 대기

`session_send`에 `wait=true`를 주면, 메시지를 보내기 직전의 `last_seq`(`after_seq`)부터 이벤트 스트림을 읽어 세션이 멈출 때까지 기다린 뒤 에이전트의 답(`replies`)을 돌려준다. `session_wait`는 같은 대기를 따로 부른다.

결과의 `status`는 다음 중 하나다.

- `settled`: 세션이 끝났다. `replies`에 `assistant.final` 텍스트가 순서대로 들어 있다.
- `waiting_human`: 사람의 티켓을 기다린다. `pending_tickets`에 `id`, `kind`, `summary`가 있다.
- `stopped`: 세션이 중지됐다.
- `error`: `runtime.error`가 왔다. `error`에 코드와 메시지가 있다.
- `timeout`: `timeout_secs`(기본 120초, 최대 3600초)가 지났다. 오류가 아니다. 돌려준 `last_seq`를 `after_seq`로 주고 `session_wait`를 다시 부르면 이어서 기다린다.

`session_send`로 기다릴 때는 먼저 이 메시지가 실제로 디스패치된 것(`message.injected`)을 확인하고, 그 뒤의 답(`assistant.final`)만 `replies`에 담는다. 이 메시지의 완료(`message.completed`/`message.dropped`) 뒤에 오는 정착만 종료로 본다. 앞서 대기 중이던 메시지의 답이나 정착을 이 메시지의 결과로 착각하지 않는다(요청의 `message_id`는 멱등 키일 뿐이다).

`session_wait`는 먼저 세션을 조회한다. `after_seq`가 없으면 현재 `last_seq`부터 기다린다. 실행 중이거나 대기열에 든 일이 없고 `after_seq`가 `last_seq`와 같으면 스트림을 열지 않고 현재 상태를 바로 돌려준다. `after_seq`가 `last_seq`보다 크면 오류다. 실행 중인 세션은 조회 시점 이후의 정착만 결과로 치고(이전 실행의 정착은 기록일 뿐이다), 멈춰 있는 세션은 마지막 정착까지 다시 읽어 `replies`에 담는다. `pending_tickets`는 이벤트 재생 전이 아니라 대기가 끝난 시점에 조회한다. 정착했는데 대기 중인 티켓이 남아 있으면 `waiting_human`이다.

### 티켓

`ticket_resolve`는 `secret_input` 티켓을 거부한다. 거부는 해결 요청을 보내기 전에 일어나고 값은 데몬으로도 전달되지 않는다. 비밀값은 사용자가 `openside ticket resolve <id> -`로 파이프에 넣어 직접 전달해야 한다. 티켓 거절과 사람 단계 완료(`done`)를 위한 도구는 없다.

### 브라우저

공개 API에 브라우저 경로는 `POST /v1/sessions/{id}/repl`(`docs/specs/browser-repl.md`) 하나뿐이라, 브라우저 도구는 모두 이 경로로 Playwright 문법 코드를 보낸다. 코드에 들어가는 URL·텍스트는 JSON 문자열로 이스케이프하고 `ref`는 영숫자·`-`·`_`만 받는다.

- `browser_open`: `page.goto(url)` 후 현재 URL을 돌려준다.
- `browser_snapshot`: `page.snapshot()`을 `tab_id: …` 줄과 함께 텍스트로 돌려준다. 16 KiB를 넘으면 REPL이 잘라서 보낸다.
- `browser_act`: 스냅샷의 `ref`에 `click`, `hover`, `check`, `uncheck`, `fill`, `press`, `select_option`을 실행한다.
- `browser_repl`: 임의 코드를 실행한다.

모든 브라우저 도구는 `tab_id`와 `approval_ticket`을 받는다. 정책이 사람의 승인을 요구하면 결과가 `{"status":"deferred","ticket_id":…}`로 오는데, 오류가 아니다. 티켓을 `ticket_resolve`로 승인한 뒤 같은 인자에 `approval_ticket`을 더해 다시 부르면 된다. 정책 거부, 실패, 스크립트 예외는 오류 결과가 된다.

## 오류

- 인자가 스키마와 맞지 않거나 알 수 없는 도구면 JSON-RPC `invalid_params` 오류다.
- 데몬이 거절하거나 실패하면(범위 부족 `403 forbidden`, 없는 세션, 정책 거부 등) 호출 자체는 성공하고 결과에 `isError: true`와 데몬 메시지가 담긴다. 토큰 값은 인자 오류와 stderr 진단을 포함해 서버 밖으로 나가는 모든 문자열에서 `[REDACTED]`로 바뀐다.

## 공개 API에 없어서 노출하지 못하는 것

- 스크린샷, 탭 열기/목록/닫기 전용 경로. 브라우저는 REPL로만 다루며 `page.screenshot`은 REPL이 지원하지 않는다.
- 세션 이벤트의 비스트림 조회. 대기는 SSE로만 가능하다.
- 메모리 쓰기·수정(`admin` 범위)과 세션 중지는 의도적으로 넣지 않았다. API는 있다.

## 검증

- `crates/openside-cli/src/tests/mcp.rs`: 도구 목록 고정, 대기·티켓 대기·타임아웃, `secret_input` 거부, 오류 매핑, 브라우저 코드 생성과 상태 매핑, 모든 요청이 ROUTES에 속함(가짜 데몬).
- `crates/openside-e2e/tests/mcp.rs`: 실제 데몬과 스크립트 사이드카를 `openside mcp`(stdio)에 rmcp 클라이언트로 연결. `initialize` → `tools/list` → `session_send` → 답 확인, 읽기 전용 토큰의 쓰기 도구 오류, 티켓 승인 후 재개, `secret_input` 거부, 로컬 페이지에서 열기·스냅샷·클릭(Chromium이 있을 때). 기록 프록시로 데몬이 받은 모든 요청이 공개 경로임을 확인한다.
