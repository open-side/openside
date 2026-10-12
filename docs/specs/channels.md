# 채팅 채널 (Telegram · Slack · Discord)

상태: **구현**(가짜 플랫폼 서버와 실제 데몬으로 검증). 실제 플랫폼 계정으로 확인한 적은 없다(blocked-external). 관련: A-13, G-5. 코드: `crates/openside-channels/`, 설정 `crates/openside-config/src/channels.rs`, 테스트 `crates/openside-channels/tests/channels.rs`, `crates/openside-e2e/tests/channels.rs`.

## 1. 구조
- 어댑터는 데몬 공개 API v1 의 클라이언트다. 세션 로직·정책·승인 판단은 모두 데몬에 있고, 어댑터는 채팅 메시지를 API 호출로, 이벤트 스트림을 채팅 메시지로 옮기기만 한다.
- 어댑터는 `openside_proto::ROUTES` 에 없는 경로를 호출하지 않는다. `DaemonClient` 가 요청마다 이를 검사하고, e2e 테스트는 기록용 프록시로 실제 요청 경로를 모두 대조한다(G-5).
- 데몬에는 TCP loopback 의 bearer 토큰으로 연결한다. 유닉스 소켓은 관리자 권한이라 쓰지 않는다. 토큰은 `read`·`steer`·`approve` 범위만 있으면 된다. `human_step` 범위가 없으므로 `secret_input`·기기 승인·인수 티켓은 데몬도 거부한다.
- 한 번에 `openside-channels run` 하나가 켜진 어댑터를 모두 돌린다. 종료는 SIGINT/SIGTERM.

```
openside-channels run              # 켜진 어댑터 실행
openside-channels check            # 설정·비밀·데몬 연결 점검(서버를 띄우지 않음)
openside-channels pair             # 봇용 기기 토큰 발급
```

## 2. 스레드 = 세션
- 채팅 스레드 하나가 세션 하나다. 스레드 키는 Telegram 은 채팅 ID, Slack 은 `channel` + `thread_ts`, Discord 는 채널 ID 다.
- 매핑은 `<state_dir>/threads.json`(기본 `<data_dir>/channels`, 디렉터리 0700, 파일 0600, 원자적 쓰기)에 둔다. 세션 ID, `/model` 로 고른 모델, 승인 카드, 이벤트 커서, 플랫폼 오프셋이 들어 있다. 비밀 값은 들어 있지 않다.
- 첫 메시지가 세션을 만들고(`POST /v1/sessions`) 이후 메시지는 같은 세션에 이어진다(`POST /v1/sessions/{id}/messages`). 어댑터를 재시작해도 같은 세션을 쓰고, 이미 보낸 답글을 다시 올리지 않는다.
- 세션은 데몬의 기본 정책 모드로 만든다. 어댑터가 모드나 작업 디렉터리를 바꾸지 않는다.
- 스레드마다 `GET /v1/sessions/{id}/events` SSE 를 하나 열고, 끊기면 백오프 후 `Last-Event-ID` 로 이어 받는다. `seq` 가 중복이면 버린다.
- 전달은 최소 한 번(at-least-once)이다. 커서는 답글이 확정된 시점에만 저장하므로, 재시작 도중 죽으면 마지막 답글이 한 번 더 올라올 수 있다. 사용자 메시지는 `ch-<해시>` 형태의 결정적 `message_id` 로 보내서 데몬이 중복을 걸러 낸다.

## 3. 명령
Telegram·Discord 는 `/`, Slack 은 `/` 를 플랫폼이 가로채므로 `!` 를 쓴다(`@bot /stop` 도 된다). 모르는 `!단어` 는 일반 메시지로 보낸다.

| 명령 | 동작 |
| --- | --- |
| (일반 메시지) | 이 스레드의 세션에 전달. 세션이 없으면 새로 만든다 |
| `/new` | 이 스레드에 새 세션을 만든다 |
| `/stop` | 실행 중인 세션을 멈춘다(`POST /v1/sessions/{id}/stop`) |
| `/model <이름>` | 다음 세션에 쓸 모델을 저장한다. 실행 중인 세션의 모델은 API 로 바꿀 수 없다 |
| `/sessions` | 세션 목록. 이 스레드의 세션을 표시한다 |
| `/usage` | 이 세션의 `usage` 이벤트를 모델별로 합산한다 |
| `/answer <텍스트>` | 선택지 없는 `info` 질문에 답한다 |
| `/help` | 명령 목록 |

봇의 답글은 영어다.

## 4. 승인 카드
- 세션에 `ticket.opened` 가 오면 스레드에 카드를 올린다. 카드는 티켓당 한 번만 올린다. 재연결로 지난 티켓이 다시 와도, 이미 처리된 티켓에는 카드를 올리지 않는다.
- `confirm`: Approve / Reject 버튼(`POST /v1/tickets/{id}/resolve` 또는 `/reject`). `info` 에 선택지가 있으면 선택지마다 버튼과 Dismiss, 선택지가 없으면 `/answer` 안내.
- 버튼을 누르면 어댑터는 카드가 이 스레드에서 올린 것인지, 티켓이 이 스레드 세션의 것이며 `pending` 인지, 종류가 `confirm`·`info` 인지를 다시 확인한다. 하나라도 어긋나면 데몬을 호출하지 않는다. 처리 후 카드는 버튼을 지우고 `Approved by user <id>` 같은 결과로 고친다. 이미 처리된 카드를 다시 누르면 `Already handled.` 로 답한다.
- `secret_input`, 기기 승인(`device_approval`), 인수(`takeover`) 티켓은 채팅에서 답할 수 없다. 버튼 없이 안내만 올린다. `secret_input` 안내는 웹 UI 또는 `openside ticket resolve <id> -` 를 가리킨다. 위조된 버튼 데이터나 `/answer` 로도 이 티켓을 처리할 수 없다.

## 5. 음성 전사(로컬 STT 옵션)
- `channels.stt_cmd` 에 로컬 명령을 지정하면 음성·오디오 첨부를 전사한다. 어댑터가 오디오를 임시 파일(0600)로 받아 명령의 마지막 인자로 넘기고, stdout 을 전사문으로 쓴다. 셸을 거치지 않으며, 임시 파일은 바로 지운다.
- 먼저 `Heard: <전사문>` 을 올려 무엇으로 들었는지 보여 주고, 같은 텍스트를 일반 메시지로 세션에 보낸다. 전사문은 명령으로 해석하지 않는다.
- `stt_cmd` 가 없으면 음성은 내려받지 않고 정중히 거절한다. 원격 STT 경로는 없다. `max_audio_bytes` 를 넘는 첨부, `stt_timeout_secs` 를 넘는 전사, 0 이 아닌 종료 코드는 거절 메시지로 끝난다.
- 오디오 다운로드에는 해당 플랫폼 출처일 때만 봇 토큰을 붙인다.

## 6. 허용 목록
- 켠 어댑터는 `allowed_*` 중 하나 이상이 채워져 있어야 한다. 비어 있으면 설정 검증이 거부한다(기본값은 아무도 받지 않음).
- 비어 있지 않은 목록은 **모두** 만족해야 처리한다. 예를 들어 Telegram 은 `allowed_chats` 와 `allowed_users` 를 둘 다 지정하면 두 조건을 모두 통과한 메시지만 받는다. 거부한 요청은 응답하지 않고 ID 만 로그에 남긴다.

## 7. 설정과 비밀
설정 키는 [`docs/CONFIGURATION.md`](../CONFIGURATION.md) 의 `[channels]` 를 본다. 비밀(봇 토큰, signing secret, 데몬 토큰)은 설정 파일에 값으로 쓰지 않는다. 환경변수 이름(`*_env`) 또는 명령 argv(`*_cmd`)로 지정한다. 값은 로그와 오류에 나오지 않으며(Telegram 은 토큰이 URL 경로에 있어 요청 URL 도 오류에서 뺀다), 어댑터는 프록시와 리다이렉트를 따라가지 않는다.

### 데몬 토큰 발급
```
openside-channels pair --out ./channels.token
# 또는
curl --unix-socket "$SOCKET" -X POST http://localhost/v1/devices/pair \
  -d '{"name":"channel bot","scopes":["read","steer","approve"]}'
```
토큰 파일은 0600 으로 쓴다. `[channels] token_file` 에 그 경로를 지정한다. 폐기는 `DELETE /v1/devices/{id}`.

## 8. 플랫폼별 설정
공통: 데몬은 `daemon.listen` 으로 TCP 를 열어 두어야 한다. 어댑터는 `api_url`(기본 `http://<daemon.listen>`)로 접속한다.

### Telegram
롱폴링(`getUpdates`)이라 공개 주소가 필요 없다. BotFather 로 봇을 만들고 토큰을 `token_env` 가 가리키는 변수에 둔다. `allowed_chats`·`allowed_users` 에 숫자 ID 를 쓴다. 오프셋은 `threads.json` 에 저장하므로 재시작해도 처리한 업데이트를 다시 받지 않는다. 승인 카드의 버튼은 인라인 키보드이고 `callback_data` 는 `a:<티켓>`·`r:<티켓>`·`o:<티켓>:<번호>` 다.

### Slack
- Event Subscriptions 의 Request URL 을 `https://<공개 주소>/slack/events`, Interactivity 의 Request URL 을 `https://<공개 주소>/slack/interactivity` 로 지정한다. 어댑터는 기본적으로 `127.0.0.1:8787` 만 연다. 공개 HTTPS 는 터널이나 리버스 프록시가 맡는다.
- 구독 이벤트: `app_mention`, `message.im`, `message.channels`. 봇 권한(scope): `chat:write`, `app_mentions:read`, `im:history`, `channels:history`, 오디오 첨부를 받으려면 `files:read`.
- 모든 요청은 `X-Slack-Signature`(HMAC-SHA256 `v0`)와 `X-Slack-Request-Timestamp` 를 검사한다. 서명이 없거나 틀리거나 5분을 넘긴 요청은 401. `url_verification` 은 서명이 맞을 때만 `challenge` 를 돌려준다.
- 스레드는 `thread_ts` 로 정한다. 채널에서 봇을 멘션하면 그 메시지가 스레드의 시작이 되고, 이후 그 스레드의 메시지는 멘션 없이도 처리한다. 매핑이 없는 채널 잡담은 무시한다. DM 은 채널 하나가 스레드 하나다. 봇이 쓴 메시지는 무시하고, 같은 `channel:ts` 이벤트가 다시 오면 한 번만 처리한다.

### Discord
- Gateway 웹소켓 대신 **Interactions Endpoint URL**(HTTP)과 REST 를 쓴다. Interactions Endpoint URL 을 `https://<공개 주소>/discord/interactions` 로 지정하고, 애플리케이션 공개키(hex)를 `public_key` 에 쓴다. 어댑터는 기본적으로 `127.0.0.1:8788` 만 연다.
- 모든 요청은 `X-Signature-Ed25519`, `X-Signature-Timestamp` 의 Ed25519 서명을 검사하고 틀리면 401. `PING`(type 1)에는 `PONG` 을 돌려준다.
- 메시지 입력은 슬래시 명령으로 받는다. 명령은 한 번 등록해야 한다.
  ```json
  [
    {"name":"ask","description":"Send a message to the agent","options":[
      {"type":3,"name":"text","description":"Message"},
      {"type":11,"name":"audio","description":"Voice message"}]},
    {"name":"new","description":"Start a new session in this channel"},
    {"name":"stop","description":"Stop the running session"},
    {"name":"model","description":"Model for the next session","options":[
      {"type":3,"name":"name","description":"Model name","required":true}]},
    {"name":"sessions","description":"List sessions"},
    {"name":"usage","description":"Token usage of this channel's session"},
    {"name":"answer","description":"Answer a pending question","options":[
      {"type":3,"name":"text","description":"Answer","required":true}]},
    {"name":"help","description":"Show commands"}
  ]
  ```
  이 JSON 을 `PUT /applications/<app id>/guilds/<guild id>/commands` 로 보낸다.
- 슬래시 명령에는 본인에게만 보이는(ephemeral) 확인 응답을 즉시 주고, 실제 답글은 REST(`POST`/`PATCH /channels/{id}/messages`)로 채널에 올린다. 버튼은 컴포넌트 상호작용(type 3)이며 `custom_id` 는 Telegram 의 `callback_data` 와 같은 형식이다. 스레드는 채널 하나다.
- 봇에 채널의 메시지 보내기 권한이 있어야 한다.

## 9. 필요한 데몬 엔드포인트
채널 전용 엔드포인트는 필요 없었다. 사용한 공개 경로: `GET /v1/settings`, `GET·POST /v1/sessions`, `GET /v1/sessions/{id}`, `POST /v1/sessions/{id}/messages`, `POST /v1/sessions/{id}/stop`, `GET /v1/sessions/{id}/events`, `GET /v1/tickets`, `GET /v1/tickets/{id}`, `POST /v1/tickets/{id}/resolve`, `POST /v1/tickets/{id}/reject`, `POST /v1/devices/pair`(유닉스 소켓, `pair` 명령만). `openside` CLI 에는 기기 발급 명령이 없어서 `openside-channels pair` 가 같은 공개 API 를 호출한다.

## 10. 검증
- `crates/openside-channels/tests/channels.rs`: 가짜 Telegram·Slack·Discord 서버로 허용 목록, Slack 서명(없음·틀림·변조·오래됨), Discord Ed25519(틀림·변조·`PING`), STT 거절·전사·크기 제한, 토큰이 다른 출처로 나가지 않는 것을 확인한다.
- `crates/openside-e2e/tests/channels.rs`: 실제 데몬과 스크립트 사이드카로 스레드=세션, 재시작 후 유지, 명령, 승인·거절·선택지·`/answer`, `secret_input` 거절, 음성 전사, Slack·Discord 왕복을 확인하고, 매번 봇이 호출한 모든 경로가 공개 `ROUTES` 에 있는지 대조한다.
- 실제 Telegram·Slack·Discord 계정으로 한 실험은 없다.
