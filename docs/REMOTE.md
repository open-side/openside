# 원격 사용: 페어링, 푸시, OTP, 상시 실행

휴대폰이나 다른 컴퓨터에서 데스크톱의 openside 를 쓰는 방법이다. 데몬은 늘 loopback 에서만 듣는다. 외부 노출과 TLS 는 앞단에 맡기고, 데몬은 기기별 범위 토큰으로 요청을 걸러 낸다. 위협 모델은 [`specs/threat-model.md`](specs/threat-model.md), 화면 동작은 [`specs/remote-ui.md`](specs/remote-ui.md) 에 있다.

## 1. 노출: TLS 는 앞단이 맡는다

```toml
[daemon]
listen = "127.0.0.1:7777"

[remote]
public_url = "https://desk.tail1234.ts.net"
```

loopback 이 아닌 `daemon.listen` 은 데몬이 거부한다. 다른 기기는 다음 중 하나를 거쳐 들어온다.

- **Tailscale Serve**(권장): 같은 tailnet 기기만 닿고, 인증서도 자동으로 붙는다.

  ```sh
  tailscale serve --bg https / http://127.0.0.1:7777
  ```

- **리버스 프록시**: 예를 들어 Caddy 라면 다음과 같다. SSE(`/v1/sessions/*/events`)가 끊기지 않도록 응답 버퍼링을 끈다.

  ```
  desk.example.com {
      reverse_proxy 127.0.0.1:7777 {
          flush_interval -1
      }
  }
  ```

  takeover WebSocket(`GET /v1/tickets/{id}/takeover`)은 브라우저가 보낸 `Origin` 을 `Host` 와 비교한다. 그래서 프록시는 원래 `Host` 를 바꾸지 말고 넘겨야 한다. Caddy `reverse_proxy` 는 기본 설정에서 그렇게 동작한다. `Host` 를 upstream 주소로 바꾸는 프록시를 쓰면 웹 UI 의 takeover 연결이 `403 origin_forbidden` 으로 거부된다. 다른 프록시와 Tailscale Serve 의 동작은 확인하지 않았다.

평문 http 로 다른 호스트에 노출하면 토큰과 페어링 코드가 그대로 네트워크를 지난다. `openside doctor` 는 이 상태를 `remote.tls` 실패로, loopback 이 아닌 리스너를 `remote.listener` 실패로 표시한다.

## 2. 페어링(QR)

데스크톱 터미널에서:

```sh
openside pair                                   # 기본 범위: read, approve, human_step
openside pair --scope read --scope steer --name 태블릿
```

QR 코드는 `<public_url>/pair#<코드>` 를 가리킨다. 휴대폰으로 열면 웹 앱이 `POST /v1/pairing/exchange` 로 코드를 그 기기 전용 토큰으로 바꾼다.

- 코드는 **3분** 동안 **한 번만** 쓸 수 있다. 만료되면 410 `pairing_expired`, 이미 썼거나 없는 코드는 404 `pairing_not_found` 다.
- 코드는 URL 프래그먼트(`#`)에 실려 프록시 로그에 남지 않는다. 데몬은 해시만 메모리에 두므로 재시작하면 발급한 코드가 모두 무효가 된다.
- 코드 발급(`POST /v1/pairing`)은 로컬 소켓에서만 된다. TCP 로 오면 admin 토큰이어도 403 `local_socket_required` 다.
- 볼트 관리(초기 설정, 잠금 해제, 로그인 또는 인증자 추가, 삭제, 항목별 정책 설정)에는 `admin` 스코프가 필요하다: `openside pair --scope admin --name phone`. 비밀번호와 인증자 키는 모바일 기기에서 입력되어 데몬으로만 전송되며, 어떤 볼트 라우트도 이 값을 다시 반환하지 않는다.
- 발급, 페어링, 만료, 폐기는 `<data_dir>/audit.jsonl` 에 `tool: "device"` 로 남는다. 코드와 토큰 값은 기록하지 않는다.

```sh
openside device list            # id, 이름, 범위, 만든 시각
openside device revoke <id>     # 즉시 폐기
```

폐기는 바로 적용된다. 그 기기의 다음 요청은 401 이고, 열려 있던 이벤트 스트림은 닫히며, 그 기기의 푸시 구독도 지운다.

`openside device revoke`를 실행하면 해당 기기는 즉시 폐기 상태로 바뀌어, 이후 들어오는 새 요청은 모두 401을 받는다. 아직 본문 수신 중인 요청은 승인되기 전에 취소되며, 열려 있던 스트림이나 진행 중인 푸시 전송(DNS 조회, HTTP 전송)도 기기나 네트워크 응답을 기다리지 않고 즉시 중단된다. 반면 본문을 온전히 전달받아 이미 승인된 요청은 연결이 끊겨도 끝까지 실행된다. 폐기 처리는 변경 사항 저장, 감사 기록 및 이벤트 생성, 세션 재개(필요한 경우)가 끝날 때까지 기다린 뒤에야 비로소 기기 토큰을 삭제하고 푸시 구독을 디스크에서 영구히 지운다. 이렇게 처리해야 명령어가 반환된 뒤에 처리 중이던 옛 구독 쓰기 작업이 끼어들어 폐기된 구독을 되살리는 일을 막을 수 있다. 해당 기기에서 이미 승인된 설정 변경 역시 마찬가지로 먼저 처리되므로, 나중에 이루어진 로컬 설정을 덮어쓸 염려가 없다. 기기가 자기 자신을 직접 폐기할 수도 있고, 두 기기가 서로를 동시에 폐기할 수도 있다.

형식이 잘못된 코드는 400 `invalid_code`, 등록되지 않은 코드는 404를 반환하며 둘 다 감사 로그에 `pairing_failed`로 기록한다. 1분 안에 이러한 실패가 20회 쌓이면 이후 잘못되거나 알 수 없는 코드로 시도할 때는 1분이 지날 때까지 429 `too_many_attempts`를 반환하고 로그도 매번 남기지 않는다. 다만 이미 등록된 유효한 코드는 이런 제한 상태에서도 언제나 정상 작동한다. 코드 만료 시점은 단조 시계(monotonic clock)를 기준으로 측정하므로 시스템 시계를 바꾸어도 유효 기간이 늘어나지 않는다. `expires_at`은 화면 표시와 감사 로그 기록에만 쓰인다.

## 3. Web Push

웹 앱에서 알림을 켜면 그 기기의 브라우저 구독이 `POST /v1/push/subscriptions` 로 등록된다. 데몬은 다음 두 경우에 알림을 보낸다.

| 언제 | 제목 | 누르면 열리는 곳 |
|---|---|---|
| 티켓이 열림(승인·질문·코드 입력 등) | 종류별 제목 + 티켓 요약 | `/sessions/<세션>?ticket=<티켓>` |
| 세션이 끝났고 남은 티켓이 없음 | `Task finished` + 세션 제목 | `/sessions/<세션>` |

- 페이로드는 RFC 8291(aes128gcm)로 그 구독에만 암호화하고, RFC 8292 VAPID 로 서명한다. VAPID 키는 `<data_dir>/push-vapid.key`(0600)에 처음 쓸 때 만든다. 키를 지우면 모든 기기가 알림을 다시 켜야 한다.
- 지금 화면을 보고 있는 기기에는 보내지 않는다. 웹 앱이 `POST /v1/push/presence` 로 `focused: true` 를 알리면 60초 동안 그 구독을 건너뛰므로, 화면을 보는 동안에는 60초보다 짧은 간격으로 다시 알려야 한다.
- 푸시 엔드포인트는 연동된 기기가 직접 지정하므로, 데몬은 알려진 브라우저 푸시 서비스(`fcm.googleapis.com`, `jmt<숫자>.google.com`(Chrome 이 실제로 발급하는 호스트), `android.googleapis.com`, `*.push.services.mozilla.com`, `*.push.apple.com`, `*.notify.windows.com`)의 `https://` 엔드포인트나 사용자가 `network.allow`에 직접 등록한 목적지(예: 자체 구축한 푸시 서비스나 테스트용 `http://127.0.0.1:<port>`)로만 요청을 보낸다. 데몬이 자체 구성한 서비스와 달리 루프백 주소를 기본 허용하지 않으며, 사용자 인증 정보가 포함된 URL도 거부한다. 전송 전마다 호스트 이름을 다시 확인하며, `network.allow`에 명시하지 않은 공인 IP 외 주소(루프백, 사설 IP, 링크 로컬, CGNAT, ULA 등)는 모두 차단한다. 또한 검증을 마친 특정 주소로만 연결을 고정하고 리다이렉트는 절대 따르지 않는다.
- 푸시 서비스가 404/410 을 돌려준 구독은 지운다.
- 알림 본문에는 비밀 값이 들어가지 않는다. 티켓 요약과 세션 제목만 담는다.

iOS 는 홈 화면에 추가한 웹 앱(16.4 이상)에서만 Web Push 를 받는다.

## 4. OTP 자동 입력

SMS 나 메일을 전달하는 앱(예: 휴대폰 단축어, 메일 규칙)이 받은 문자를 그대로 보낸다.

```sh
curl -H "Authorization: Bearer $OTP_TOKEN" -H 'content-type: application/json' \
  -d '{"sender":"BANK","text":"인증번호 [481516]"}' https://desk.tail1234.ts.net/v1/otp
```

```toml
[otp]
senders = ["BANK", "+15550100"]
ttl_secs = 300

[otp.sites]
"https://login.example.com" = '\[(\d{6})\]'
```

- 토큰은 `openside token create --scope otp_ingest` 로 만든다. 이 토큰은 `/v1/otp` 말고는 아무것도 못 한다.
- 허용 목록 밖 발신자는 403 `sender_not_allowed` 이고 감사 기록에 남는다.
- 사이트 정규식에 맞는 코드가 나오면 그 origin 의 가장 최근 `secret_input` 티켓을 바로 채운다. 아직 티켓이 없으면 `ttl_secs` 동안 메모리에 두었다가, 그 사이 티켓이 열리면 채운다.
- 코드는 디스크, 로그, 이벤트, 감사 기록 어디에도 남지 않는다. 감사 기록에는 사이트와 티켓 id 만 적는다.

문자 메시지 하나에 담긴 코드가 `otp.sites`의 여러 사이트에 동시에 들어맞으면, 하나의 SMS로 사이트 두 곳을 채울 수 없도록 409 `ambiguous_otp`로 거부하고 감사 로그를 남긴다. 코드를 보관하는 작업과 대기 중인 단계에 값을 채우는 작업은 단일 락으로 보호되므로, 코드가 들어오는 순간 막 진입한 단계라도 여전히 채워지며, 정확히 한 번만 채워진다. 사용하지 않은 코드는 `otp.ttl_secs`가 지나면 메모리에서 완전히 지운다.

## 5. 상시 실행

```sh
openside service install        # Linux: ~/.config/systemd/user/openside.service
                                # macOS: ~/Library/LaunchAgents/dev.openside.daemon.plist
openside service unit           # 쓰지 않고 내용만 출력
```

- systemd 유닛은 `Restart=on-failure`, `WantedBy=default.target` 이고, launchd 에이전트는 `RunAtLoad` 에 `KeepAlive.SuccessfulExit=false` 다. 비정상 종료 때만 다시 띄우므로 일부러 멈춘 데몬은 멈춘 채로 남는다.
- 데몬은 인자 없이 실행되므로 경로와 리스너는 설정 파일 `[daemon]` 에 둔다.
- `install` 은 파일만 쓰고, 켜는 명령(`systemctl --user enable --now openside.service`, `launchctl bootstrap gui/$(id -u) …`)을 출력한다. 로그아웃한 뒤에도 돌게 하려면 Linux 에서 `loginctl enable-linger` 를 쓴다.
- 재시작한 데몬은 이벤트 로그를 다시 읽어 진행 중이던 세션을 이어 간다.

절전 방지는 끄는 것이 기본이다.

```toml
[service]
inhibit_sleep = true
```

옵션을 켜면 실행 중인 태스크가 있는 동안에만 `systemd-inhibit`(Linux)이나 `caffeinate`(macOS)를 띄워 두고, 마지막 태스크가 끝나면 바로 내린다. 메시지가 전송된 순간부터 처리가 끝날 때까지를 태스크 실행 중으로 간주하며, 모델 요청 사이의 대기 시간이나 도구 및 브라우저가 작업 중인 시간도 여기에 포함된다. 인간 개입 티켓을 기다리는 동안에는 절전 방지를 해제하고 세션을 `waiting_human`으로 표시하며, 티켓에 응답하면 다시 절전을 방지한다. 다른 도구를 쓰려면 `inhibit_cmd`를 설정한다.
