# 원격 사용: 페어링, 푸시, OTP, 상시 실행

openside 데몬은 항상 loopback(127.0.0.1)에서만 수신 대기한다. 외부 기기에서 원격으로 접속하려면 Tailscale Serve를 사용하거나, 데몬이 관리하는 Cloudflare Tunnel을 띄우거나, 아웃바운드 연결 전용 자체 호스팅 릴레이를 거쳐야 한다. 어떤 전송 방식을 쓰더라도 기기별로 발급한 디바이스 토큰이 동일한 권한 스코프를 검증한다. 자세한 보안 경계는 [`specs/threat-model.md`](specs/threat-model.md)에, 원격 UI 구성 및 동작은 [`specs/remote-ui.md`](specs/remote-ui.md)에 정리되어 있다.

## 1. 전송 방식 선택

`remote.mode`는 `"tailscale"`, `"cloudflare"`, `"relay"`, `"off"` 중 하나를 지정하거나 `["tailscale", "relay"]`처럼 배열로 여러 방식을 동시에 활성화할 수 있다. 기본값은 관리형 전송 방식을 켜지 않는 상태다. 여러 방식을 켜 두었을 때는 페어링 시 `--via` 옵션을 주지 않는 한 목록의 첫 번째 방식을 기본으로 사용한다.

설정 명령어는 기존 주석을 유지하면서 설정 파일을 수정하지만, 데몬 서비스를 자동으로 재시작하지는 않는다. 데몬을 직접 재시작해야 새 설정이 반영된다. 전송 모드를 `"off"`로 바꾸더라도 이미 발급한 기기 토큰은 취소되지 않는다.

현재 전송 상태는 `openside remote status`로 확인하고, 진단은 `openside doctor`로 실행한다. 진단 도구는 `remote.tailscale`, `remote.cloudflare`, `remote.relay` 항목을 통해 각 도구의 설치 여부와 설정값, 토큰 파일 및 환경 변수 존재 여부(외부 인증 유효성 검증은 아님)를 검사하며, `remote.listener`와 `remote.tls`로 loopback 바인딩과 안전한 TLS 적용 여부를 판별한다. 테스트 환경의 가짜 바이너리나 로컬 일반 HTTP 기반 픽스처 라우터 검증을 실제 클라우드 서비스 검증과 혼동해서는 안 되며, 외부 상용 서비스 연동 자체는 자동화 테스트 범위에 들어가지 않는다.

### Tailscale Serve

기존 tailnet 경로를 그대로 쓰며, 시스템에 Tailscale이 설치되어 로그인되어 있고 MagicDNS가 켜져 있어야 한다. TLS 종단은 사용자의 로컬 Tailscale 노드에서 직접 이루어지므로 제공업체 서버에 평문 트래픽이 노출되지 않는다.

`openside remote setup tailscale`을 실행하면 등록에 필요한 Serve 명령어를 출력하며, `--apply` 플래그를 붙이면 명령어를 바로 실행한다.

```toml
[daemon]
listen = "127.0.0.1:47820"

[remote]
mode = "tailscale"
public_url = "https://desk.tail1234.ts.net"
```

백그라운드에서 실행하는 명령어는 다음과 같다.

```sh
tailscale serve --bg https / http://127.0.0.1:47820
```

페어링 시에는 `openside pair --via tailscale`을 쓴다. 같은 tailnet에 속한 기기만 접속할 수 있다. 리버스 프록시를 직접 쓰는 환경이라면 `mode` 없이 기존 `remote.public_url`만 지정해도 되며, Caddy를 쓸 때는 SSE 스트리밍이 끊기지 않도록 버퍼링을 꺼야 한다.

```caddy
desk.example.com {
    reverse_proxy 127.0.0.1:47820 {
        flush_interval -1
    }
}
```

브라우저 인수인계(takeover) 기능은 `Origin` 검증을 거치므로, 프록시가 원래 `Host` 헤더를 변경하지 않고 데몬에 넘겨주어야 한다.

### Cloudflare Tunnel

시스템에 `cloudflared` 바이너리를 별도로 설치해야 한다. 빠른 터널(Quick Tunnel)은 별도 계정 설정 없이 즉시 띄울 수 있다.

```sh
openside remote setup cloudflare
```

```toml
[remote]
mode = "cloudflare"

[remote.cloudflare]
tunnel = "quick"
```

데몬은 내부적으로 `cloudflared tunnel --no-autoupdate --url http://<listen>`을 실행하고, 표준 출력 및 표준 에러에서 생성된 `https://*.trycloudflare.com` 주소를 파싱한다. 빠른 터널은 프로세스를 재시작할 때마다 접속 주소가 바뀌므로 재시작 후에는 페어링 QR 코드를 다시 생성해야 한다. 터널 연결이 끊어지면 1초, 2초, 4초 순으로 최대 60초까지 지연 시간을 늘려 가며 재시작하고, 60초 이상 안정적으로 유지되면 지연 시간을 초기화한다. 데몬을 종료하면 자식 프로세스도 함께 정리한다.

도메인을 직접 지정하는 네임드 터널(Named Tunnel)을 쓰려면 Cloudflare 대시보드에서 터널과 호스트명을 먼저 만든 뒤, 발급받은 토큰을 권한이 제한된 파일(`chmod 0600`)에 저장한다.

```sh
openside remote setup cloudflare --named --hostname desk.example.com --token-file /path/to/cloudflare-token
```

설정 파일에 `token_file`을 지정하지 않았다면 `OPENSIDE_CLOUDFLARE_TOKEN` 환경 변수를 사용할 수 있다. 토큰은 오직 `TUNNEL_TOKEN` 환경 변수로만 프로세스에 전달되며, 명령어 인자(argv)에는 노출되지 않는다.

Cloudflare 방식은 클라우드 제공업체 인프라에서 HTTPS가 종단되므로 평문 트래픽에 대해 제공업체를 신뢰해야 하며, 제공업체를 상대로 한 종단간 암호화(E2EE)는 성립하지 않는다. 페어링 시에는 `openside pair --via cloudflare`를 쓴다.

### 자체 호스팅 릴레이 (openside-relay)

공유 릴레이 서버를 직접 운영하는 방식이다. 외부 인터넷에서 들어오는 트래픽은 TLS 리버스 프록시가 먼저 받아야 한다.

웹 UI 애셋을 빌드하고 릴레이 서버를 컴파일한다.

```sh
pnpm -C web build
cargo build -j 4 -p openside-relay
```

릴레이 서버를 실행한다. 등록 토큰 파일에는 한 줄에 16자 이상의 등록 토큰을 하나씩 적어 넣으며, `#` 주석을 지원한다. 환경 변수 `OPENSIDE_RELAY_TOKENS`에 쉼표로 구분해 등록할 수도 있다. 등록 토큰은 데몬마다 독립적으로 발급해야 하며, 단일 토큰 공유는 단일 관리자가 단일 데몬을 운영할 때만 허용된다.

```sh
openside-relay --listen 127.0.0.1:8080 --token-file /path/to/daemon-tokens --web-dir web/dist
```

데스크톱 데몬 측에서는 발급받은 토큰 파일(`0600`)을 지정해 릴레이 모드를 설정한다. 환경 변수 `OPENSIDE_RELAY_TOKEN`도 지원한다.

```sh
openside remote setup relay --relay-url https://relay.example.com --token-file /path/to/relay-token
```

```toml
[remote]
mode = "relay"

[remote.relay]
url = "https://relay.example.com"
token_file = "/path/to/relay-token"
```

데몬은 릴레이 서버를 향해 아웃바운드 WebSocket 연결을 맺으므로 방화벽 인바운드 포트를 열 필요가 없다(로컬 TCP 리스너는 loopback에서 계속 동작한다). 데몬은 `<data_dir>/relay-key`(0600 권한)에 고유 X25519 개인키를 보관하며, 데몬 ID는 이 공개키에서 유도된다.

등록 과정에서는 도메인이 분리된 챌린지(`openside-relay-registration/1\n{id}`) 기반 Noise NK 핸드셰이크를 거치며, 공개키 해시와 라우팅 ID의 일치 여부를 검증한다. 릴레이 프로세스가 실행되는 동안 각 등록 토큰은 최초 인증된 데몬 ID에 바인딩되며 다른 토큰이 이를 대체할 수 없다. 서버가 재시작되면 바인딩은 초기화되지만 개인키 소유 증명은 여전히 요구된다.

핸드셰이크 완료 후의 모든 HTTP 본문, 헤더, Bearer 토큰, SSE 및 WebSocket 스트림은 `Noise_NK_25519_AESGCM_SHA256` 전송 암호화를 거치므로 릴레이 서버에 평문으로 노출되지 않는다. 핸드셰이크 시의 공개키 및 임시 키 교환 값은 암호화되지 않으며, 릴레이 서버는 데몬 ID, 클라이언트 IP, 전송량, 접속 시간 등 세션 메타데이터를 확인할 수 있다. 릴레이 서버는 전역적으로 최대 256개의 데몬 소켓과 1024개의 클라이언트 소켓을 허용하며, 데몬당 최대 32대의 클라이언트와 클라이언트당 8MiB 버퍼를 유지한다. 재접속 시에도 페어링된 공개키 정보를 유지한다.

- **프로토콜 및 등록 제한**: 릴레이 통신은 `Noise_NK_25519_AESGCM_SHA256` 핸드셰이크를 사용한다. 서버 프로세스가 실행되는 동안 등록 토큰당 라우팅 ID 1개만 바인딩할 수 있으며, 동시 데몬 소켓은 최대 2개(활성 연결 1개 + 재연결 증명 1개)로 제한된다. 테넌트 간 토큰을 공유해서는 안 되며 데몬 및 테넌트마다 고유 토큰을 발급해야 한다.
- **전역 및 다중화(Mux) 버퍼 예산**: 릴레이는 전역 최대 256개 데몬 소켓과 1024개 디바이스 소켓을 허용하며, 데몬당 최대 32대 클라이언트와 클라이언트당 8MiB 전달 버퍼를 기본값으로 둔다. 스트림별 다중화는 양방향 각각 최대 256개 대기 프레임과 256KiB 크레딧을 적용하고, 실제 송신된 바이트에 대해서만 크레딧을 보충한다. 특정 느린 클라이언트의 큐가 포화되면 해당 클라이언트 연결만 닫아 다른 클라이언트가 차단되는 현상을 방지한다.
- **프레임 제한, 타임아웃 및 스트림 수명**: 브라우저 수신은 최대 128개 암호화 프레임(4MiB), 송신은 1024개 프레임(8MiB)으로 제한된다. HTTP 헤더 및 트레일러는 128줄(64KiB), WebSocket 단편은 1024개(16MiB)까지 허용한다. 타임아웃은 핸드셰이크 10초, 쓰기 5초, 닫기 1초를 적용하며, 64비트 논스 소진($2^{64}-1$) 시 연결을 영구히 종료한다. 스트림 ID는 재사용되지 않고 32비트 공간이 소진되기 전에 세션을 종료하므로 지연된 오래된 프레임이 종료된 스트림을 되살릴 수 없다.

CLI에서 릴레이 데몬에 연결할 때는 페어링 코드가 프로세스 목록(argv)에 노출되지 않도록 주소 없이 명령어를 실행한 뒤 숨겨진 프롬프트나 표준 입력(stdin)으로 링크를 붙여넣는 방식을 권장한다(인자로 링크를 넘길 수도 있으나 stderr 경고가 출력된다).

```sh
openside connect --name desk
openside --remote desk session list
```

환경 변수 `OPENSIDE_REMOTE=desk`를 지정하거나 `openside mcp --remote desk`로 원격 데몬에 MCP 서버를 연결할 수 있다. 원격 목록은 `openside remote profiles list`와 `openside remote profiles rm <이름>`으로 관리한다. 인증 토큰은 시스템 키링에 저장하며, 지원되지 않는 환경에서는 `0600` 권한의 파일(`OPENSIDE_REMOTE_TOKEN_STORE=file`)을 사용한다.

**신뢰 모델 주의사항**: 독립적으로 설치된 CLI와 MCP는 페어링 QR 코드나 링크에 고정(pinning)된 데몬 공개키와 Noise 프로토콜 종단간 암호화(E2EE)로 릴레이 연결을 보호한다. 반면 릴레이가 제공하는 PWA는 릴레이 운영자가 서빙하는 HTML과 JS 코드를 신뢰해야 한다. 따라서 자체 호스팅 릴레이를 운영하거나, 릴레이를 거치지 않고 데몬 또는 Tailscale 오리진에 직접 연결하여 PWA를 불러오는 구성을 권장한다.

릴레이 서버가 제공하는 웹 페이지의 JavaScript 코드가 변조된 경우, 페어링 과정에서 URL 프래그먼트에 담긴 `#c=<페어링코드>&k=<데몬공개키>` 값을 읽어 들일 수 있다. 이 코드는 3분의 유효 시간 동안 1회만 교환 가능하지만, 악의적인 스크립트가 사용자가 정상 교환하기 전에 이를 가로채 로컬 CLI가 허용한 스코프 범위 내에서 공격자 소유의 영구 기기 토큰으로 등록할 위험이 있다. URL 프래그먼트는 일반적인 HTTP 요청이나 프록시 로그에는 남지 않지만 해당 페이지에서 실행되는 스크립트에는 그대로 노출된다. 독립 바이너리로 설치되어 페어링 시 고정된 Noise NK 공개키를 자체 검증하는 CLI나 MCP 클라이언트와 달리, 릴레이 PWA는 브라우저가 원격 코드를 내려받아 실행하는 구조다. 따라서 신뢰할 수 있는 자체 호스팅 릴레이를 직접 운영하거나, `openside pair --via tailscale`을 이용해 데몬이나 신뢰할 수 있는 Tailscale 오리진의 PWA를 직접 사용하는 방식을 권장한다. 현재 생성되는 릴레이 페어링 링크는 릴레이 오리진으로 바로 이동하며, 웹 앱 셸과 서비스 워커 업데이트가 네트워크 우선(network-first)으로 동작하므로 PWA를 한 번 설치했다고 해서 실행 코드의 불변성이 보장되지는 않는다(코드 해시 고정이나 사용자 승인 기반 업데이트 체계는 구현되어 있지 않다). 다만 데몬의 정적 개인키(`relay-key`)는 브라우저나 릴레이 서버로 절대 전송되지 않으므로, 이러한 웹 페이지 스크립트 및 URL 프래그먼트 탈취 경로를 통해 데몬 개인키가 직접 노출되지는 않는다.

### 릴레이 환경의 셸 CSP 정책

데몬 직접 접속 및 Tailscale 환경과 릴레이 환경 모두 셸 페이지의 기본 CSP로 `script-src 'self'`와 `frame-src 'self' blob:`을 공통 적용한다. 모든 환경에서 애플리케이션 자체 스크립트만 실행할 수 있으며 인라인 스크립트는 허용되지 않는다. `frame-src`는 허용되는 iframe 출처를 지정하며 스크립트 실행 권한과는 무관하다. 차이점은 오직 스타일에만 국한된다. 데몬 직접 접속 및 Tailscale 환경에서는 프리뷰가 데몬의 독립된 HTTP 응답 CSP 헤더를 직접 받으므로 셸이 `style-src 'self'`를 유지한다. 반면 릴레이 환경에서는 터널을 거쳐 생성된 불투명 샌드박스 내부의 Blob 프리뷰가 셸의 스타일 제약을 그대로 상속받으므로, 차트 및 UI 시각 요소의 인라인 스타일 렌더링을 위해 릴레이 서빙 셸에 한해 `style-src 'self' 'unsafe-inline'`을 허용한다. 프리뷰 내부에는 `script-src 'none'`을 포함한 엄격한 메타 CSP가 별도로 강제되므로 외부 스크립트 실행이나 네트워크 유출은 원천 차단된다.


## 2. 페어링(QR)

데스크톱 터미널에서:

```sh
openside pair                                   # 기본 범위: read, approve, human_step
openside pair --scope read --scope steer --name 태블릿
openside pair --agent --via relay --scope read --scope steer --scope browse
```

사람의 판단과 승인 권한은 프롬프트 지침이 아니라 데몬이 직접 강제하는 규칙이다. 기기와 토큰의 역할(`kind`)은 코드를 교환할 때가 아니라 운영자가 오퍼나 토큰을 발급할 때 결정된다. 데몬 호스트에서 일반 `openside pair`를 실행하면 `person_device` 오퍼가 생성되고(웹 UI 선택지 'Pair a phone/browser'), `openside pair --agent`를 실행하면 `agent_client` 오퍼가 생성된다(웹 UI 선택지 'Connect an agent / CLI'). `openside token create`는 기본적으로 `agent_client` 토큰을 발급하며, 로컬 소켓 전용인 `openside token create --person`을 실행하면 확장 프로그램·웹·모바일 기기용 `person_device` 토큰을 명시적으로 발급한다. `openside connect`는 오퍼에 지정된 역할을 그대로 물려받을 뿐 변경할 수 없으므로, `person_device` 오퍼를 교환하면 그대로 `person_device`가 된다. 로컬 Unix 소켓 자체는 식별자가 `local`이며 `agent_client`로 분류된다. `person_device` 토큰을 에이전트에 넘기면 사람의 승인 권한까지 부여되므로 에이전트에는 절대 `person_device` 토큰을 주어서는 안 된다. 로컬 소켓에 접근할 수 있는 동일 UID 프로세스는 의도적으로 `person_device` 토큰을 발급할 수 있으므로, 이 구분은 OS 샌드박스 경계가 아니라 신뢰받는 운영자의 제어 수단이다.

사람의 판단이나 승인이 필요한 작업은 기존 스코프에 더해 반드시 `person_device` 역할이 있어야만 수행할 수 있다. 여기에는 확인(confirm) 티켓 승인(`true` 또는 `"approve"`), 모든 선택지(`choice`) 티켓 응답, `secret_input`의 `{totp}` 영구 등록, `login`의 `save: true` 저장이 포함된다. `agent_client`나 로컬 소켓(`local`)이 이러한 작업을 시도하면 해당 티켓을 직접 생성하거나 세션을 구동한 주체가 아니더라도 `403 person_approval_required` 오류로 거부된다. 거부된 티켓은 승인 대기(pending) 상태를 그대로 유지하며 페이지 조작이나 Vault 등록·저장도 일절 실행되지 않는다. 반면 확인 티켓 거절(`false` 또는 `"reject"`)과 거절·취소 엔드포인트 호출은 필요한 스코프만 있으면 에이전트에서도 허용된다.

기존 외부 에이전트 프로필은 `openside pair --agent`로 새 오퍼를 발급받아 다시 페어링해야 한다.

`POST /v1/pairing` 요청(`{name, scopes, via?, kind?}`)을 보내면 선택한 전송 방식에 맞는 QR 코드와 링크를 생성하며, 전송 방식을 따로 지정하지 않으면 활성화된 첫 번째 방식을 기본값으로 사용한다.

- **직접 연결(기존 방식)**: `<public_url>/pair#<코드>` 형태의 링크를 쓴다.
- **릴레이 연결**: `<relay_url>/pair#c=<코드>&t=relay&d=<데몬ID>&k=<공개키>` 형태를 쓰며, 민감한 파라미터는 프래그먼트(`#`)에만 들어가므로 프록시 서버 로그나 요청 헤더에 남지 않는다.
- 모든 페어링 코드는 전송 방식과 관계없이 **3분 동안 한 번만** 유효하며, 발급받은 기기 토큰의 스코프 규칙도 동일하게 적용된다.
- 비활성화된 전송 방식을 `--via`로 지정하면 409 `transport_not_enabled`를 반환하고, 전송 설정은 켜져 있으나 아직 준비되지 않은 상태라면 503 `transport_unavailable`로 거부한다.
- 데몬의 릴레이 개인키(`relay-key`)가 바뀌면 클라이언트에 저장된 상대방 공개키 불일치로 핸드셰이크 연결이 실패하므로 기기를 다시 페어링해야 한다. 이때 기존에 발급받은 기기 베어러 토큰 자체가 취소되는 것은 아니다.

- 코드는 **3분** 동안 **한 번만** 쓸 수 있다. 만료되면 410 `pairing_expired`, 이미 썼거나 없는 코드는 404 `pairing_not_found` 다.
- 코드는 URL 프래그먼트(`#`)에 실려 프록시 로그에 남지 않는다. 데몬은 해시만 메모리에 두므로 재시작하면 발급한 코드가 모두 무효가 된다.
- `POST /v1/pairing`은 비공개 로컬 소켓 또는 소유자 브라우저의 검증된 로컬 사람 증명(소켓에서 발급한 사람/관리자 토큰 + 현재 부팅별 시크릿, 실제 루프백 피어, 리터럴 루프백 Host, 프록시 헤더 없음)을 요구한다. 페어링된 원격 기기나 릴레이/터널 호출자는 관리자 권한이 있어도 페어링 제안을 생성할 수 없다. 자세한 내용은 [`remote-ui.md` §8](specs/remote-ui.md#8-로컬-원격-접속-설정)을 참고한다.
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

> Web Push 없는 데스크톱 알림. 패치된 openside 브라우저는 푸시 서비스 키 없이 배포되므로 `PushManager.subscribe()` 호출 시 `AbortError`로 거부된다. Web Push를 사용할 수 없을 때(Push API 미지원, 사이트 권한 차단, subscribe 호출 거부 등) openside 확장 프로그램이 설치되어 있다면 Settings → Notifications → Enable notifications에서 확장 프로그램 기반 데스크톱 알림으로 전환할 수 있다. 이 토글을 켜면 확장 프로그램과 1회용 페어링 코드로 페어링하며(`read`, `steer`, `approve`, `human_step` 스코프, 기기 이름은 "openside extension"으로 등록되고 알림 끄기 시 다시 해지됨), 확장 프로그램의 백그라운드 워커가 데몬 상태를 평소에는 30초마다, 작업 실행 중에는 5초마다 확인한다. 실행 중인 작업이 끝나면 대화 제목과 함께 "Task finished" 알림을 띄우고, 새로운 대기 요청이 생길 때마다 알림을 표시한다. 포커스된 브라우저 창에 열려 있는 대화는 알림을 띄우지 않고 무음으로 유지된다. 알림을 클릭하면 해당 대화가 열리거나 요청 처리를 위한 사이드 패널이 열린다. 확장 프로그램은 토큰을 세션 스토리지에 보관하므로, 브라우저를 재시작해도 토글이 켜져 있으면 웹 앱이 자동으로 다시 페어링한다. 휴대전화나 다른 브라우저는 기존 Web Push 방식을 그대로 사용한다. 127.0.0.1/localhost의 데몬 UI만 확장 프로그램을 켤 수 있으며(`externally_connectable`), 확장 프로그램은 다른 데몬과의 기존 페어링을 임의로 덮어쓰지 않는다.

iOS 앱 아이콘 배지 역시 홈 화면에 웹 앱을 추가한 상태에서만 지원된다. 앱을 설치하고 나면 모바일 공유 시트에서 링크, 텍스트, 이미지를 openside로 바로 전송할 수 있으며(remote-ui.md 4절 참고), 테일넷 외부 등 데몬과 통신할 수 없는 상태에서 작성한 메시지는 기기에 안전하게 보관되었다가 다시 연결될 때 자동으로 전송된다.

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

## 모바일 엔드투엔드 검증 증거

`crates/openside-e2e/tests/mobile_remote.rs`는 임시 포트에서 비공개 데몬, 스크립트 기반 Python 사이드카, 픽스처 웹사이트, 암호화된 셀프 호스팅 릴레이를 실행한다. 데스크톱 브라우저는 소켓 전용 로컬 접근 권한을 얻어 Remote access 화면을 열고 휴대전화 페어링 제안을 생성한다. `zbarimg`로 렌더링된 스크린샷의 QR 코드를 디코딩하고, 휴대전화는 디코딩된 링크로 접속한다. 이 흐름을 검증할 때 CLI로 만든 페어링 제안이나 미리 주입한 모바일 토큰으로 대체하지 않는다.

두 모바일 프로필은 390×844 CSS 픽셀, DPR 3, 터치 포인트 5개, iPhone Safari 및 Android Chrome과 유사한 User-Agent를 갖춘 헤드리스 Chromium을 사용한다. 프로덕션 매니페스트 및 서비스 워커, 대화 목록, 새로 시작한 채팅, 새로고침 없는 증분 어시스턴트 이벤트 및 실시간 이벤트 갱신, 사람 전용 승인 규칙, 빨간색에서 파란색으로 바뀌는 실제 프리뷰 JPEG 픽셀, 스크립트 모델 입력에 SHA-256이 나타나는 사진을 모두 검증한다.

휴대전화의 대화 메뉴에서 새 대화를 열면 현재 화면이 이미 새 대화 화면이더라도 메뉴가 닫히므로, 메뉴가 메시지 작성 창을 가리지 않는다.

포그라운드 배지 검증은 Chromium의 실제 배지 API 호출을 확인하는 것이며, 설치된 OS 아이콘이 바뀌었음을 증명하지는 않는다. 알림 단계에서는 VAPID를 검증하고 RFC 8291 복호화를 수행하는 로컬 푸시 서비스를 거친 뒤 DevTools를 통해 실제 서비스 워커에 해당 바이트를 온전히 전달한다. 실제 GCM/APNs 전달, 물리 카메라 스캔, 네이티브 앱 설치, iOS WebKit 환경은 테스트하지 않는다. iPhone User-Agent를 사용하더라도 실행 환경은 여전히 Chromium이다.

사전 준비 사항: `web/dist` 빌드, 사이드카 동기화, Chromium 및 `zbar-tools` 설치가 필요하다. 표준 검증 게이트는 비활성화된 릴레이 테스트를 명시적으로 **두 번** 실행하여 로그와 캡처를 모두 보존하며, 두 모바일 프로필의 검증 마커가 모두 확인되지 않으면 단순 Cargo 통과만으로는 승인하지 않는다.

```sh
python3 tools/qa/mobile_remote.py --results /tmp/openside-qa/round2/MobileE2E/gate --jobs 2
```

동일한 테스트 과정을 실제 일회용 Cloudflare Quick Tunnel로 실행할 수도 있으나, 공용 네트워크 사용에 명시적으로 동의하고 `cloudflared`가 설치되어 있어야만 한다. 격리된 표준 게이트 환경에서는 절대 활성화되지 않는다.

Cloudflare [공식 문서에 따르면 Quick Tunnel은 SSE를 지원하지 않는다](https://developers.cloudflare.com/tunnel/get-started/quick-tunnels/#limitations). 실제 Quick Tunnel 프로브 테스트에서도 1초 간격으로 플러시된 5개 이벤트가 업스트림 스트림이 닫힐 때에만 한꺼번에 전달되는 것을 확인했다. 따라서 웹 수신기는 버퍼링되거나 지연된 SSE 응답을 감지하면 `/v1/sessions/{id}/events/poll`을 통해 마지막 시퀀스 번호부터 다시 이어받는다. Quick Tunnel 기반 모바일 테스트는 해당 폴링 엔드포인트로 실제 브라우저 네트워크 요청이 발생하는지 검증해야 하며, 로컬 SSE의 성공이나 `cloudflared` 감독 프로세스 흉내로는 이 검증을 대체할 수 없다.

```sh
OPENSIDE_MOBILE_CLOUDFLARE=1 python3 tools/qa/mobile_remote.py \
  --transport cloudflare --results /tmp/openside-qa/round2/MobileE2E/cloudflare-gate --jobs 2
```

테스트 하네스는 Tailscale Serve 설정을 절대 임의로 변경하지 않는다. 일회용 데몬용 핸들러가 없는 상태에서 tailnet이 실행 중이라고 해서 이를 Serve를 통한 원격 PWA 접근 증거로 볼 수는 **없다**. 이러한 전송 방식은 루프백 직접 HTTP 통신으로 조용히 대체되지 않으며 실행되지 않은 상태로 남는다.

