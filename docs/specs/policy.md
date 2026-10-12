# 정책·스코프·외부 전송

데몬의 권한 판단 규칙을 정리한다. 구현은 `crates/openside-policy`, `crates/opensided/src/policy_api.rs` 에 있다.

## 1. 토큰 스코프

| 스코프 | 허용 범위 |
|---|---|
| `read` | 조회 API |
| `steer` | 세션 생성·메시지·중지. `browse` 를 포함한다 |
| `approve` | `confirm`/`info` 티켓 응답 |
| `human_step` | `secret_input`/`device_approval`/`takeover` 티켓 응답 |
| `browse` | `POST /v1/sessions/{id}/repl` 만 |
| `vault_use`, `otp_ingest` | 예약. 현재 이 스코프를 요구하는 라우트는 없다 |
| `admin` | 모든 라우트 |

- Unix 소켓 호출자는 `admin` 으로 취급한다. 소켓은 0700 디렉터리 안의 0600 파일이다.
- loopback TCP 는 Bearer 토큰이 필요하다. 토큰이 없거나 모르는 토큰이면 401, 스코프가 모자라면 403.
- 라우트별 스코프 표는 `openside_proto::ROUTES` 하나뿐이다. OpenAPI 의 `x-capability` 도 여기서 생성한다.
- 발급: `openside token create --scope browse [--scope ...] [--name NAME]`. 출력은 `device_id<TAB>token` 한 줄이다. 토큰은 다시 볼 수 없다.
- 발급(`POST /v1/devices/pair`)은 Unix 소켓으로만 된다. TCP 로 호출하면 `admin` 토큰이어도 403 `local_socket_required` 다. 유출된 토큰으로 새 토큰을 만들지 못하게 하는 규칙이다.
- `browse` 는 라우트 단위 스코프이고 세션 단위로 가두지 않는다. 토큰 소지자는 세션 id 를 알면 어느 세션의 REPL 이든 호출할 수 있다.

## 2. 세션 모드와 판단 순서

모드는 `read-only`, `guard`(기본), `full` 이다. 행동(`fs_read`, `fs_write`, `exec`, `browse`, `fetch`, `vault_use`, `sensitive`)마다 아래 순서로 판단한다.

1. 일치하는 `deny` 규칙이 있으면 거부한다.
2. origin 목록: `deny_origins` 에 맞거나, `allow_origins` 가 비어 있지 않은데 맞지 않으면 `browse`·`fetch`·`vault_use`·`sensitive` 를 거부한다.
3. `sensitive` 는 최소 `ask` 다. 허용 규칙으로도 낮출 수 없다.
4. 가장 위 레이어에서 일치한 규칙이 결정한다. 같은 레이어 안에서는 Deny > Ask > Allow.
5. 일치하는 규칙이 없으면 모드 기본값을 쓴다.

레이어는 아래에서 위로 쌓인다. 데몬 정책(에이전트 기본값) 위에 세션 규칙이 올라간다. 세션 규칙은 에이전트의 Deny 를 약하게 만들 수 없다.

`Deny` 는 승인으로 뒤집히지 않는다. `Ask` 는 `confirm` 티켓을 연다. 티켓에는 id, 요약, 대상 origin(있을 때), 만료 시각이 있다. 응답이 없으면 만료되고 거부로 처리한다.

민감 행동 키워드는 `openside_policy::SENSITIVE_KEYWORDS` 가 정한다. 결제·구매·삭제·전송·게시 계열이며, 버튼 이름과 역할로 판정한다. 키워드로 분류하는 방식이므로, 이름이 키워드와 맞지 않는 버튼(예: "Continue")은 민감 행동으로 보지 못한다. 그런 행동은 origin 목록과 모드 기본값이 제한한다.

## 3. 정책 API

| 메서드 | 경로 | 스코프 | 설명 |
|---|---|---|---|
| GET | `/v1/policy` | `read` | 현재 정책 |
| PUT | `/v1/policy` | `admin` | 정책 교체. 검증에 실패하면 400 `invalid_policy` |
| POST | `/v1/policy/test` | `read` | 행동 하나를 실행하지 않고 판단만 한다 |

문서 형식:

```json
{"default_mode":"guard","allow_origins":[],"deny_origins":["https://*.evil.example"],
 "rules":[{"effect":"deny","capability":"fs_write","pattern":"/etc/**"}]}
```

- origin 은 `scheme://host[:port]` 다. 목록마다 항목은 최대 512개이고 길이는 1..=1024다. 브라우저가 실제로 보내는 origin 과 같은 URL 정규화(WHATWG, IDNA)를 거쳐 비교하므로, 쓴 모양과 상관없이 브라우저가 접속하는 origin 에 맞는다.
  - scheme: `http`, `https`, `ws`, `wss`. 대소문자는 구분하지 않는다.
  - host: 소문자와 punycode 로 정규화하고 끝 점은 지운다. `*.example.com` 은 모든 하위 도메인에 맞고 `example.com` 자신에는 맞지 않는다. `*` 만 쓰면 모든 host 다. 그 밖의 자리에 `*` 가 있으면 거부한다.
  - port: 숫자 또는 `*`. 생략하면 scheme 의 기본 포트(`http` 80, `https` 443)만 뜻한다. 그래서 `https://example.com` 은 `https://example.com:8443` 에 맞지 않는다. 명시한 기본 포트(`:443`)는 생략과 같다.
  - 사용자 정보(`user@`), 경로, 쿼리, 역슬래시, 공백이 있거나 scheme 이 없으면 400 `invalid_policy` 다. 아무것도 막지 못하는 항목을 조용히 저장하지 않는다.
  - 매칭 시간은 패턴 길이에 선형이다.
- `sensitive` 규칙은 `deny` 만 쓸 수 있다. 모르는 필드는 거부한다.
- PUT 이 성공하면 `<data_dir>/policy.json`(0600)에 원자적으로 저장한다. 이후에는 config 의 `[policy]` 대신 이 파일을 쓴다. 파일이 깨져 있으면 데몬이 시작하지 않는다. PUT 은 저장과 반영을 한 단위로 하나씩 처리하며, 요청이 중간에 끊겨도 끝까지 마친다. 그래서 실행 중인 정책, 디스크의 파일, 재시작 뒤의 정책이 항상 같다.
- 이미 열린 세션도 다음 툴 호출부터 새 정책을 따른다. 이미 실행 중인 호출은 시작할 때 읽은 정책으로 끝난다(§4 의 REPL 이동 검사는 예외). `default_mode` 는 새 세션·루틴에 적용된다.
- CLI: `openside policy show`, `openside policy set <file>`, `openside policy test <target> <capability> [--mode M] [--workspace P]`. `test` 는 `effect<TAB>reason` 을 출력한다.

## 4. 브라우저 이동과 비밀 입력

origin 목록과 `browse` 규칙은 도구 호출 앞단(`host.rs`)뿐 아니라 브라우저 컨트롤러에서도 강제한다. 도구가 요청한 URL 만 보면 리다이렉트나 팝업으로 거부된 origin 에 닿을 수 있기 때문이다.

- 에이전트 소유 탭(그 탭이 연 팝업 포함)의 최상위 문서 요청은 전부 요청이 네트워크로 나가기 전에 현재 정책으로 판단한다. 첫 로드, 리다이렉트 매 홉, 링크·폼 이동, `target=_blank`, `window.open` 이 모두 해당한다. 거부된 요청은 중단되고 탭은 직전 문서에 남는다.
- `Deny` 는 어떤 경우에도 통과하지 못한다. `Ask` 인 origin 은 사람이 그 origin 으로 승인한 호출이 실행되는 동안에만 통과한다. 호출이 끝나면 승인도 끝난다. 승인한 페이지에서 돌아와 다시 같은 origin 으로 이동하려면 새 승인이 필요하다.
- 정책은 요청마다 새로 읽는다. 정책을 바꾸면 이미 열린 탭의 다음 이동부터 적용된다.
- 소유자를 아직 모르는 팝업(opener 가 아직 장부에 없는 경우)과 소유자 조회에 실패한 요청은 보류했다가, 브라우저 `attach_timeout` 안에 소유자가 정해지지 않으면 거부한다. 하위 프레임, `page` 가 아닌 타깃, opener 가 없는 페이지는 확인된 경우에만 통과시킨다.
- 페이지 스크립트가 보내는 요청(XHR, `fetch`, 하위 리소스)은 에이전트의 행동이 아니므로 판단하지 않는다.
- 실행 중인 REPL 스니펫은 시작할 때의 정책 스냅샷으로 끝까지 돈다. 같은 시간에 브라우저 쪽 이동 검사는 요청마다 현재 정책을 읽으므로, 정책 PUT 이후의 이동은 실행 중인 스니펫 안에서도 바로 막힌다.

비밀 입력(`secret_input`) 티켓을 열 때는 대상 탭의 origin 에 대한 `browse`, `vault_use` 행동을 만들어 같은 정책으로 판단한다. 티켓에 응답해 값을 채우는 순간에는 그 origin 을 현재 정책으로 다시 판단한다. 그 사이에 `Deny` 가 생겼다면 403 `policy_denied` 를 돌려주고, 티켓은 대기 상태로 남으며 필드는 건드리지 않는다. 사람의 응답은 `Ask` 를 풀 뿐 `Deny` 는 풀지 못한다.

## 5. 외부 전송(egress)

- 허용 목록의 기준 타입은 `openside_policy::Egress` 다. loopback 은 항상 허용한다. 나머지는 사용자가 설정한 모델·채널 엔드포인트와 config `[network] allow` 항목(`host`, `*.domain`, `host:port`, `scheme://host[:port]`)만 허용한다. 데몬은 시작할 때 `allow` 문법을 검사하고, 틀리면 시작하지 않는다.
- 강제하는 곳은 세 군데다.
  - 사이드카: 세션을 열 때 `httpx` 계열 HTTP 전송을 감싼다. 허용 대상은 loopback, 만든 모델 제공자의 base URL, 자격 증명 broker, `network.allow` 다. 리다이렉트는 홉마다 검사하고, 막히면 `outbound connection to X is not in network.allow` 로 실패한다.
  - 채널 어댑터(`openside-channels`): 서비스 API 주소와 플랫폼 첨부 호스트, `network.allow` 만 허용하고, 요청을 보내기 전에 거부한다.
  - `openside memory model fetch`: 지정한 엔드포인트(기본 엔드포인트는 Hugging Face CDN 포함)와 `network.allow`. 리다이렉트도 검사한다.
- 사이드카 안의 검사는 그 프로세스 안에서 도는 장치이지 커널 수준 격리가 아니다. 샌드박스에서 돌지 않는 사이드카는 신뢰하는 코드이며, 사이드카나 그 의존성이 직접 소켓을 열면 이 검사로는 막지 못한다. 데몬이 대신 실행하는 도구(파일, `bash`, 브라우저)는 Host 훅과 샌드박스가 제한한다.
- 데몬 자체에는 외부 HTTP 클라이언트가 없다. CDP 는 loopback 이다.
- Chromium 이 페이지를 불러오는 트래픽은 `Egress` 를 거치지 않는다. 에이전트가 열 수 있는 페이지는 §4 의 origin 목록으로만 제한된다.
- 번들 Chromium 은 백그라운드 통신을 끄는 플래그로 띄운다. 업데이트, 네트워크 시간, AI 모드 자격 확인, 검색엔진 preconnect 를 끈다. 끌 스위치가 없는 GCM checkin 은 끄는 것이 아니라 로컬 포트 `127.0.0.1:9` 로 돌린다. 컴포넌트 업데이트 주소 역시 `127.0.0.1:9` 로 돌린다. `--disable-component-update` 는 정기 확인만 막고 on-demand 요청(최신 Chromium 이 시작할 때 `update.googleapis.com` 에 기기 내 AI 모델 매니페스트를 요구하는 동작)은 막지 못하지만, 업데이트 주소가 일반 HTTP 라 Chromium 이 요청 자체를 보내지 않고 버린다. 그 포트에서 듣는 로컬 서비스가 있으면 GCM 요청이 그쪽에 닿는다.
- 검증:
  - `crates/openside-e2e/tests/egress.rs`: 데몬, 사이드카, Chromium 을 모든 경로가 loopback 으로 돌아오는 전용 네트워크 네임스페이스에서 실행한다. loopback 패킷 캡처로 세션·루틴·메모리를 거치는 동안 나간 모든 목적지가 `Egress` 를 통과하는지 확인한다. 캡처 자체는 장수 TCP, 즉시 실패하는 TCP, 단발 UDP, DNS 대조군으로 검증한다. 네임스페이스 안의 허용 목록 상대에 실제로 닿는지, 목록 밖 호스트와 리다이렉트 홉이 연결 전에 거부되는지, 브라우저와 REPL 작업이 성공했는지도 단언한다.
  - 사이드카 강제는 `agent/tests/providers/test_egress.py`, 채널은 `openside-channels` 의 `http.rs` 단위 테스트, 모델 다운로드는 `openside-cli` 의 `model_fetch` 테스트가 확인한다.
  - 이 테스트가 실제로 구동하는 경로만 본다. LLM 제공자와 주고받는 실제 트래픽은 대상이 아니다.

## 6. 텔레메트리

기본값도 설정도 없고 보내는 코드도 두지 않는다. 테스트가 확인하는 것은 두 가지다. 하나는 Rust 의존성 그래프와 `agent/uv.lock` 에 알려진 텔레메트리 SDK(sentry, OTLP exporter, posthog, logfire 등)가 없다는 것이다. 이름 목록(블랙리스트) 검사라서, 목록에 없는 패키지가 들어오는 것은 이 검사로 못 잡는다. 다른 하나는 위 네트워크 관찰에서 허용되지 않은 목적지로 나간 패킷이 없다는 것이다. `logfire-api`, `opentelemetry-api` 는 전송 기능이 없는 API 패키지다.
