# 루틴 (Routines) — 예약 실행

상태: **구현됨**. 관련: A-10. 코드: `crates/openside-sched`(스케줄러·저장소), `crates/opensided/src/{routines,session_delete}.rs`(데몬 연결), `crates/openside-proto/src/routines.rs`(API 타입), `crates/openside-cli/src/routines.rs`(`openside routine`).

## 1. 동작
루틴은 스케줄과 액션의 쌍이다. 데몬이 1초마다 만기된 루틴을 확인해 실행한다.

| 항목 | 내용 |
| --- | --- |
| 스케줄 | `cron`(정확히 5필드, 요일은 POSIX: 0·7=일요일, 시간대는 IANA 이름, 기본 UTC. §8) 또는 `interval`(초) |
| 액션 `new_session` | 새 세션을 만들고 프롬프트를 보낸다. 모델·모드·작업공간을 생략하면 데몬 기본값을 쓴다 |
| 액션 `heartbeat` | 이미 있는 세션에 프롬프트를 후속 메시지(큐 레인)로 넣는다. 새 세션을 만들지 않는다 |
| 겹침 | 이전 실행의 메시지가 아직 끝나지 않았으면 이번 실행은 보내지 않고 `skipped_overlap` 으로 기록한다 |
| 대상 세션 삭제 | heartbeat 대상이 삭제되면 루틴을 pause(`enabled=false`, `paused_reason=target_missing`)하고 `paused_target_missing` 을 기록한다. 만기 시점에 대상이 이미 없어진 경우도 같다 |
| 지금 실행 | `POST /v1/routines/{id}/run`. 스케줄과 `next_run_at` 은 그대로 둔다. 겹침 규칙은 똑같이 적용되고, 비활성 루틴도 실행할 수 있다 |
| pause / resume | `PATCH {enabled}`. 재개하면 `next_run_at` 을 지금부터 다시 계산한다. 멈춘 시간은 `missed` 로 세지 않는다 |
| 반복 요청 제안 | `GET /v1/routines/suggestions`. 사용자가 보낸 비슷한 메시지가 3번 이상, 간격이 규칙적일 때 초안(이름·프롬프트·스케줄)을 돌려준다. 루틴을 만들지는 않으며, 이미 같은 프롬프트의 루틴이 있거나 루틴이 보낸 메시지(`routine-` 접두 ID)는 제외한다 |

실행 기록(`RunRecord`) 상태: `started`, `completed`, `failed`, `skipped_overlap`, `paused_target_missing`, `missed`. `started` 는 조회할 때 세션 상태를 보고 `completed`/`failed` 로 바뀐다.

## 2. 재시작 복구
- `next_run_at` 은 실행 전에 먼저 저장한다(at-most-once). 데몬이 중간에 죽어도 같은 만기를 두 번 실행하지 않는다. 실행 자체도 §6 처럼 호스트 호출 전에 기록한다.
- 데몬이 꺼져 있던 동안의 만기는 다시 실행하지 않고 `missed` 로 기록한다. 기록은 최대 50건이고 넘치면 요약 한 건으로 남긴다.
- `catch_up=true` 인 루틴은 놓친 것 중 가장 최근 1건만 시작 직후 실행한다.
- 시스템이 멈췄다 늦게 깨어난 경우(틱 지연)도 같은 규칙으로 최신 1건만 실행한다.
- 열린 실행과 heartbeat 대상은 시작할 때 다시 확인한다(§6).

## 3. API / CLI
권한: 조회는 `read`, 생성·수정·삭제·실행은 `admin`.

| 메서드·경로 | 설명 | CLI |
| --- | --- | --- |
| `GET /v1/routines` | 목록 | `routine list` |
| `POST /v1/routines` | 생성(201) | `routine add --name N (--cron EXPR [--timezone Z] \| --interval-secs S) --prompt P [--heartbeat SESSION] [--model M --mode MODE --workspace PATH] [--catch-up] [--disabled]` |
| `GET /v1/routines/suggestions` | 반복 요청 초안 | `routine suggest` |
| `GET/PATCH/DELETE /v1/routines/{id}` | 조회·수정·삭제(삭제는 삭제된 `Routine` 을 200 으로 돌려준다) | `routine show/pause/resume/rm` |
| `POST /v1/routines/{id}/run` | 지금 실행(`RunRecord`) | `routine run` |
| `GET /v1/routines/{id}/runs?limit=` | 실행 기록, 최신순 | `routine runs` |

오류 코드: `invalid_schedule`, `invalid_timezone`, `invalid_value`, `invalid_action`, `target_session_missing`(heartbeat 대상 없음), `not_found`, `routines_unavailable`(스케줄러 미기동, 503).

## 4. 세션 삭제 `DELETE /v1/sessions/{id}`
루틴의 "대상 세션 삭제" 를 만들기 위해 추가한 엔드포인트다(`admin`, 204).
- 세션을 멈추고(열린 티켓은 거부) `session.deleted` 묘비 이벤트를 남긴 뒤 목록·조회에서 숨긴다. 이후 그 세션에는 어떤 이벤트도 추가할 수 없다.
- 이벤트 로그는 append-only 이므로 디스크에서 지우지 않는다. 데이터 정리(purge)는 하지 않는다.
- SSE 스트림은 `session.deleted` 를 보낸 뒤 끝난다. 이미 삭제되었거나 없는 id 는 404. 로그의 마지막 seq 보다 큰 `Last-Event-ID` 는 영원히 채울 수 없으므로 400 `invalid_event_id` 로 거절하고, 재생이 끝났는데 세션이 삭제되어 있으면 스트림을 닫는다.
- 삭제 뒤 정리는 `session_delete::on_session_deleted` 한 곳에서 한다. 지금은 루틴 pause 만 하고, 다른 정리 작업도 여기에 붙인다.

## 5. 저장소
`<data_dir>/routines.db`(SQLite): `routines`, `runs`. 세션 이벤트 로그(`events.db`)와 분리되어 있고, 루틴을 지워도 실행으로 만들어진 세션은 남는다.

## 6. 실행 예약과 겹침 판정 (재시작 안전)
- 실행 한 건은 호스트(세션 엔진)를 건드리기 **전에** `started` 상태로 먼저 저장한다. 이때 세션 ID(`new_session` 은 새로 정한 ID, `heartbeat` 는 대상)와 `routine-` 접두 메시지 ID를 함께 기록한다. 호스트는 이 ID 를 그대로 쓰고, 메시지 추가는 ID 기준으로 멱등이다.
- 따라서 프롬프트가 큐에 들어간 직후 데몬이 죽어도 재시작 뒤 그 실행이 "열린 실행"으로 남아 겹침 판정이 유지된다. 복구 시 열린 실행은 메시지 단위로 확인한다: 큐에 있거나 진행·대기 중이면 계속 진행 중, 완료면 `completed`, 실패·중단·삭제면 `failed`, 세션에 메시지가 없으면(전달 전에 죽음) `failed`("never queued").
- 호스트 오류가 나도 프롬프트가 이미 큐에 있으면 실행은 `started` 로 남는다. 아무것도 큐에 넣지 못했으면 `failed` 로 닫아 다음 실행을 막지 않는다.
- 세션이 `deferred`(티켓 대기 등)로 멈춘 메시지는 아직 끝난 것이 아니다. 티켓이 해결되어 사이드카가 원래 메시지 ID 로 `completed` 를 다시 보고하면 그때 완료로 바뀐다. 세션을 `stop` 하면 `aborted`(실행은 `failed`), 재시작 때 깨울 티켓이 없으면 `failed` 로 정리한다. 이미 끝난 메시지에 늦게 도착한 중복 결과는 무시한다.
- 복구(`recover`)는 활성 heartbeat 루틴의 대상 세션이 실제로 있는지도 확인한다. 묘비는 남았는데 삭제 후처리 전에 데몬이 죽은 경우에도 루틴이 pause 된다.

## 7. 동시성
`get`·`list` 를 제외한 스케줄러의 모든 쓰기(`create`·`tick`·`recover`·`run_now`·`update`·`delete`·`on_session_deleted`)는 하나의 게이트로 직렬화된다. 틱이 호스트 호출 중이어도 생성·pause·삭제·스케줄 변경은 그 틱이 끝난 뒤에 적용되고, 돌아온 뒤에는 어떤 진행 중 틱도 옛 상태로 동작하지 않는다. heartbeat 대상 세션이 존재하는지는 `create`·`update`(액션 변경)가 **게이트를 잡은 채 저장 직전에** 확인한다(없으면 400 `target_session_missing`). 따라서 세션 삭제 후처리는 이미 저장된 heartbeat 루틴을 보고 pause 하거나, 저장 시도가 대상 없음으로 거절된다. 틱은 루틴마다 저장소에서 다시 읽어 처리하며, 삭제된 루틴의 실행 기록은 외래 키(`ON DELETE CASCADE`)로 함께 지워져 이후 추가될 수도 없다.

## 8. 크론 문법과 서머타임
- 정확히 5필드(`분 시 일 월 요일`)만 허용한다. 6필드(초)·7필드(초·연도)와 연도 필드는 `invalid_schedule`. `@daily` 같은 별칭과 `MON`·`JAN` 이름은 허용한다.
- 일과 요일을 모두 지정하면 둘 중 하나만 맞아도 만기다(표준 cron).
- 서머타임이 있는 시간대(예: `America/New_York`)의 동작은 croner 규칙을 따르며 테스트로 고정되어 있다.
  - 시각이 고정된 작업이 **건너뛰는 구간**(예: 3월 2:30)에 걸리면 건너뛰기가 끝난 첫 시각(3:00)에 한 번 실행한다.
  - 고정 시각이 **겹치는 구간**(예: 11월 1:30)에 걸리면 앞선 시각에 한 번만 실행하고 뒤의 같은 벽시계 시각에는 실행하지 않는다.
  - 시를 `*` 로 둔 작업(예: `0 * * * *`)은 겹치는 시간의 두 번 모두 실행한다.
- 유효성 검사와 만기 계산은 주입된 시계를 쓰므로 테스트는 벽시계 시간에 의존하지 않는다.

## 9. 세션 삭제와 묘비
삭제된 세션에는 **어떤 메시지 ID 로도** 이벤트를 추가할 수 없다(이전에 저장된 ID 를 되풀이하는 멱등 경로 포함). 삭제는 세션 액터 안에서 진행 중 실행·열린 티켓·대기 중 메시지를 닫고 묘비를 남긴 뒤 액터를 종료하므로, 삭제와 동시에 들어온 Deliver·heartbeat 가 런타임을 다시 열거나 도구를 실행하지 못한다.

## 10. Agent proposals and owner consent

The person's explicit `POST /v1/routines` action still creates a routine directly.
The local CLI uses the Unix socket; the web app and paired person devices use their
existing person credentials. Direct creation and payload-changing `PATCH` requests
require `person_device` in addition to `admin` (the local socket is exempt). An agent
client may pause an existing routine with `{"enabled":false}`, but may not change its
name, schedule, action, catch-up setting or resume it without a new person action.

Both the chief/front and workers can call `propose_routine`. Its arguments are the
existing `CreateRoutineRequest`: `name`, `schedule`, `action`, optional `enabled`
(default `true`) and `catch_up` (default `false`). The tool validates the request and
its live target without inserting anything into `routines.db`. Secret-bearing text
is refused rather than silently changing what will be approved.

```json
{
  "name": "Morning briefing",
  "schedule": {"type": "cron", "expr": "0 9 * * 1-5", "timezone": "Asia/Seoul"},
  "action": {"type": "agent", "agent_id": "chief", "prompt": "Summarize today's priorities"},
  "enabled": true,
  "catch_up": false
}
```

The result is an existing external `confirm` ticket and Confirmation Card. Its
instructions show the complete normalized request, including the exact prompt,
schedule/timezone and dispatch target. The durable `Ticket.tool.args` is the sole
proposal payload; there is no in-memory proposal map and no briefly enabled routine
that is disabled afterward. Agent targets resolve to that active agent's permanent
home thread at dispatch, never to an arbitrary fallback session.

Approval uses the existing `POST /v1/tickets/{id}/resolve` path and requires a
distinct person identity with approval authority. The answer must be `true` or
`"approve"`; it cannot supply an edited configuration. The daemon creates exactly
the stored request and resolves the ticket with
`{"approved":true,"status":"created","routine_id":"…"}`. Workers resume through
the existing external-ticket result continuation, rather than calling creation
again. A new prompt, schedule or target needs a fresh proposal and fresh approval;
reusing the same tool-call identity with changed arguments is refused.

Declining, rejecting, expiring or cancelling an unanswered proposal creates no
routine. Pending tickets and their payloads survive daemon restart. Replaying the
same proposal call reuses its original ticket, including a terminal one; replaying
an HTTP resolution cannot create another routine.

`routines.db` additionally stores `approved_routines` receipts, keyed by ticket id.
The receipt and routine insertion commit in one SQLite transaction. If creation
committed but the ticket-resolution append was interrupted, retry or ticket closure
reports that prior approved creation instead of duplicating it or falsely claiming
it was declined. Receipts retain the exact approved payload and outlive routine
deletion, so replay cannot recreate a routine the person removed. They are creation
receipts, not resumable external-side-effect workflows.
