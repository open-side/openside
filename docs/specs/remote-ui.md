# 원격 UI (웹/PWA)

상태: **구현됨**. 관련: F-1, E-4, PP-6, PP-8, PP-16, PP-22. 코드: `web/`(React 19 + TypeScript + Vite), `crates/opensided/src/{ui,session_meta}.rs`(정적 서빙·세션 메타데이터), `crates/openside-proto/src/session_meta.rs`(API 타입), `crates/openside-e2e/tests/web.rs`(헤드리스 E2E).

## 1. 구조
- UI는 데몬 HTTP API만 쓴다(E-4). 모든 요청은 `web/src/api/client.ts` 를 거치고, 타입은 OpenAPI 스냅샷(`docs/specs/openapi.json`)에서 `pnpm -C web gen:api` 로 만든 `web/src/api/schema.ts` 를 쓴다(PP-8). 스냅샷은 `cargo run -q -p openside-proto --example openapi > docs/specs/openapi.json` 로 갱신하고, 생성 문서와 다르면 `openside-proto` 테스트가 실패한다.
- 라이브 스트리밍은 `GET /v1/sessions/{id}/events` SSE 를 `fetch` + `ReadableStream` 으로 읽는다. EventSource 는 `Authorization` 헤더를 보낼 수 없어서 쓰지 않는다. 재연결할 때 `Last-Event-ID` 로 이어 받고, 이미 받은 `seq` 이하는 버린다. 401·403·404 는 재시도하지 않는다.
- 데몬과 맺은 장기 연결(세션 이벤트 스트림 `GET /v1/sessions/{id}/events`, 브라우저 미리보기 및 제어용 WebSocket)은 페이지가 숨겨질 때(`pagehide`) 닫고, 뒤로/앞으로 가기 캐시(bfcache)에서 복원될 때(`persisted`인 `pageshow`) 다시 연결하며 이벤트 스트림은 `Last-Event-ID`부터 재개한다. Chromium은 다른 페이지로 이동해도 캐시에 페이지를 보관하면서 연결을 계속 유지하는데, 데몬이 HTTP/1.1로 동작하므로 이런 페이지가 몇 개만 쌓여도 호스트당 최대 6개 연결 제한에 걸려 현재 화면의 모든 요청이 무한정 대기하기 때문이다. 관련 로직은 `web/src/api/pageLifecycle.ts`(`whilePageShown`)에 있으며, 회귀 테스트는 `crates/openside-e2e/tests/web_connections.rs`에 작성되어 있다.
- 화면 상태는 이벤트 로그만으로 다시 만든다(PP-6). `web/src/state/session.ts` 의 `reduceSession` 은 순수 함수이고, 처음부터 재생하든 중간에 재연결하든 같은 상태가 나온다.
- 기기 토큰은 `localStorage` 에 둔다. PWA 를 다시 열어도 로그인이 유지돼야 하기 때문이다. 대신 CSP 로 외부 스크립트를 막고, 401 을 받으면 토큰을 지우고 토큰 입력 화면으로 돌아간다. `secret_input` 값은 저장하지 않고, 상태(state)에도 넣지 않으며, 보낸 직후 입력란을 비운다.
- 모든 UI 기능과 해당 기능이 호출하는 엔드포인트는 타입이 지정된 레지스트리 `web/src/api/uiFeatures.ts`에 정의하며, `pnpm -C web gen:ui-map`으로 `docs/specs/ui-api-map.md`를 생성한다. `web/src/api/uiApiMap.test.ts`는 다음 경우에 실패한다. (a) 클라이언트 메서드가 등록된 엔드포인트와 다른 경로를 호출할 때, (b) 엔드포인트가 `docs/specs/openapi.json`에 없을 때, (c) 특정 기능에 사유 없이 엔드포인트가 지정되지 않았을 때, (d) OpenAPI 엔드포인트를 사용하지 않으면서 사유도 명시하지 않았을 때, (e) `web/src/api/` 외부의 웹 코드에서 `fetch`, `XMLHttpRequest`, `EventSource`, `WebSocket`, `sendBeacon`을 직접 호출할 때(오프라인 셸 전용이자 `/v1`을 호출하지 않는 `public/sw.js`만 예외), (f) 생성된 맵 문서가 최신 상태가 아닐 때. 브라우저 확장 프로그램의 사이드 패널도 동일한 클라이언트를 가져와 쓰므로 해당 호출 역시 함께 등록된다.

## 2. 데몬 정적 서빙
`[ui] dir` 또는 `OPENSIDE_UI_DIR` 에 `pnpm -C web build` 결과(`web/dist`)를 지정하면 데몬이 같은 포트(유닉스 소켓·TCP)에서 UI 를 서빙한다. 디렉터리에 `index.html` 이 없으면 경고만 남기고 UI 라우트를 붙이지 않는다.

| 항목 | 동작 |
| --- | --- |
| SPA 폴백 | 확장자 없는 경로는 `index.html`. 확장자가 있는데 파일이 없으면 404 |
| API 보호 | `/v1`, `/v1/*` 는 UI 가 가로채지 않는다. 없는 API 경로는 JSON `not_found` |
| 경로 검사 | 퍼센트 디코딩 후 `..`·NUL·역슬래시는 400. 심볼릭 링크로 디렉터리 밖을 가리키면 404 |
| 메서드 | GET·HEAD 만. 나머지는 405 |
| 보안 헤더 | `Content-Security-Policy`(`script-src 'self'`, `frame-ancestors 'none'` 등, 인라인 스크립트 없음), `X-Content-Type-Options: nosniff`, `Referrer-Policy: no-referrer`, `X-Frame-Options: DENY` |
| 캐시 | `assets/`(해시 파일명)는 immutable, 나머지는 `no-cache` |

정적 파일에는 인증이 없다. API 는 기존처럼 토큰과 권한 범위를 검사한다.

## 3. 세션 메타데이터 API
| 메서드·경로 | 권한 | 설명 |
| --- | --- | --- |
| `GET /v1/sessions?q=&archived=` | read | `archived` 기본값은 `false`다. `q`는 대소문자를 구분하지 않고 제목과 첫 번째 메시지를 검색한다. 상단 고정(`pinned`) 세션을 먼저 표시하며, 각 묶음 안에서는 가장 최근에 활동한 세션부터 정렬한다. 이때 활동은 제목·고정·보관 상태 변경을 제외한 모든 이벤트를 뜻하므로, 세션 이름을 바꿔도 정렬 순서는 바뀌지 않는다 |
| `PATCH /v1/sessions/{id}` | steer | 요청 본문은 `{title?, pinned?, archived?}` 형태다. 실제로 변경된 필드만 `session.updated` 이벤트로 기록하며, 본문이 비어 있으면 400 `invalid_value`를 반환한다. `title`은 단일 행으로 정규화된다(제어 문자, 줄바꿈, 양방향 텍스트 제어 문자를 공백으로 바꾸고 연속된 공백은 하나로 축약하며 앞뒤 공백은 다듬음). 정규화 후 빈 문자열이 되거나 길이가 200자를 초과하면 400 `invalid_title`을 반환한다 |
| `GET /v1/settings` | read | 기본 모델·대체 모델·정책 모드(모델 선택용). 자격증명은 없다 |
| `GET`·`PUT /v1/settings/config` | admin | `[credentials]`를 제외한 모든 설정 키를 JSON으로 조회하고 수정한다(수정 사항은 데몬 재시작 후 반영). 응답에는 설정 JSON Schema(`schema`), 페어링된 기기가 수정할 수 있는 키 목록(`remote_editable`, 끝이 `.`이면 해당 접두사 하위 전체), 호출자가 페어링 기기인지 여부(`remote`)가 함께 포함되며 설정 화면은 이를 기반으로 폼을 구성한다. TCP 연결에서 그 외 키를 수정하려고 하면 403 `local_socket_required`로 거부되고 `settings/refused_remote`로 감사 로그에 기록된다. 자세한 내용은 [`docs/CONFIGURATION.md`](../CONFIGURATION.md)를 참고한다 |

- 제목 없이 `POST /v1/sessions`를 호출하면 첫 어시스턴트 응답이 올 때까지 `New session`을 사용한다. 이후 사이드카가 `session.title_proposed`를 통해 URL이 아닌 최대 6단어 길이의 제목을 제안한다. 첫 번째 유효한 제안이 대화 제목이 되며, 세션 생성 시 명시한 제목이나 사용자가 직접 변경한 제목이 항상 우선한다. 자동 생성된 제목을 그대로 유지한 채 `PATCH`를 호출하더라도 해당 제목은 사용자 지정 제목으로 확정된다. 명시적 제목은 기존과 동일하게 단일 행 정규화 및 200자 제한(`400 invalid_title`)이 적용된다.
- `POST /v1/sessions/{id}/messages` 및 `PATCH /v1/sessions/{id}/queue/{mid}` 요청에서 첨부 파일이 없는데 본문이 비어 있거나 공백만 있으면 400 `empty_message`로 거부한다. 데몬이 요청 본문이나 쿼리 문자열을 파싱할 수 없는 경우, 다른 오류와 마찬가지로 `{code, message}` JSON 형식으로 응답한다(파서 상세 내용을 담은 400 또는 422 `invalid_request`, 415 `unsupported_media_type`, 413 `payload_too_large`. 메시지 엔드포인트에서는 413 `attachments_too_large`).
- 데몬의 데이터 디렉터리가 위치한 볼륨 용량이 부족할 때(SQLite `SQLITE_FULL`, 파일 쓰기 중 `ENOSPC` 또는 `EDQUOT` 발생 등) 이벤트 로그 추가나 파일 쓰기가 필요한 요청(`POST /v1/sessions`, `POST /v1/sessions/{id}/messages`, `PATCH /v1/sessions/{id}`, `POST /v1/devices/pair`, 첨부 파일 업로드 등)은 409 `invalid_transition` 대신 HTTP 507 및 `{code: "storage_full", message}`로 거절한다. 거절된 요청은 이벤트 순번의 연속성을 깨지 않으며 메시지를 중복 기록하지도 않는다. 조회 요청, `GET /v1/health`, 이벤트 스트림은 정상 동작을 유지하며, 디스크 공간을 확보하면 데몬을 재시작하지 않아도 같은 요청이 다시 정상 처리된다. 클라이언트는 맹목적으로 재시도하지 말고 디스크 공간을 확보하라는 안내를 사용자에게 표시해야 한다.

### 페어링·푸시 API
| 메서드·경로 | 권한 | 설명 |
| --- | --- | --- |
| `POST /v1/pairing` | admin + 로컬 사람 권한 증명 | `{name, scopes, via?, kind?}` → `{code, path, url, transport, scopes, expires_at, kind}`. 로컬 소켓 또는 검증된 소유자 브라우저에서 생성하는 3분 유효 단발성 제안(§8 참조). 원격 페어링된 admin은 제안을 발행할 수 없다. |
| `POST /v1/pairing/exchange` | 없음(코드가 자격) | `{code}` → `{device_id, token, scopes}`(201). 만료 410 `pairing_expired`, 사용했거나 없는 코드 404 `pairing_not_found` |
| `GET /v1/devices`, `DELETE /v1/devices/{id}` | admin | 기기 목록(이름·범위)과 즉시 폐기. 폐기하면 다음 요청이 401 이고 열린 SSE 가 닫힌다 |
| `GET /v1/push/vapid` | read | VAPID 공개키(`applicationServerKey`) |
| `GET`·`POST /v1/push/subscriptions`, `DELETE /v1/push/subscriptions/{id}` | read | 이 기기의 구독만 보고 고친다. `POST` 는 `PushSubscription.toJSON()` 그대로(`{endpoint, keys:{p256dh, auth}}`), https 또는 loopback http 엔드포인트만 |
| `POST /v1/push/presence` | read | `{subscription_id, focused}`. focused 면 60초 동안 그 구독에 푸시하지 않는다 |
| `GET /v1/browser/tabs` | read | 비공개 세션이 아니며 에이전트가 명시적으로 소유한 탭만 나열한다. 반환 항목은 CDP `tab_id`, 인증 정보·쿼리·프래그먼트를 제외한 URL, 제목, 소유권 정보, 소유 세션 ID다. 일반 사용자 탭, 외부 탭, 소유자를 알 수 없는 탭, 비공개 탭은 목록에서 제외된다. 브라우저가 아직 실행되지 않은 상태라면 브라우저를 띄우지 않고 곧바로 200 `{"tabs":[]}`를 반환한다. 브라우저를 띄웠으나 컨트롤러 연결이 끊어진 상태라면 503 `browser_disconnected`를 반환한다. 이 API가 브라우저를 직접 실행하거나 재연결하지는 않는다 |

알림 페이로드는 `{kind:"ticket"|"settled", session_id, ticket_id?, title, body, url}` 이고 `web/public/sw.js` 가 `url`(같은 origin 경로만)을 알림 데이터로 담아 클릭 시 그 창을 열거나 포커스한다. 노출·TLS·서비스 등록은 [`docs/REMOTE.md`](../REMOTE.md).

## 4. PWA
- `web/public/manifest.webmanifest`, 아이콘 `web/public/icons/icon.svg`·`icon-192.png`·`icon-512.png`(any)과 `icon-maskable-192.png`·`icon-maskable-512.png`(maskable).
- `web/public/sw.js` 는 오프라인 셸, Web Share Target, Web Push 를 담당한다. `/v1/*` 는 가로채지도 캐시하지도 않는다. 페이지 이동은 네트워크 우선이고, 오프라인이면 캐시된 셸을 보여 준다. 매니페스트(`/manifest.webmanifest`), 아이콘(`/icons/*`), `/theme-boot.js`, 셸 진입점(`/`) 역시 네트워크 우선으로 받아오고 오프라인일 때만 캐시를 쓰며, 내용 해시가 붙은 `/assets/*` 파일만 캐시 우선으로 다룬다. 캐시 이름은 `openside-shell-v5`이며(maskable 아이콘도 함께 보관한다) 셸 파일 목록이 바뀔 때마다 버전을 올려야 한다. `push` 이벤트는 알림을 띄우고 `notificationclick` 은 알림의 deep link 를 연다. 서비스 워커는 프로덕션 빌드에서만 등록한다.
- 매니페스트: `id`, `display_override`(`window-controls-overlay`, `standalone`), `shortcuts`, `share_target`을 정의한다. `shortcuts` 항목 중 `New chat`은 `/?launch=new`를 열고, `Needs you`는 `/?launch=needs-you`로 연결되어 처리 대기 중인 가장 오래된 티켓 대화로 이동한다(대기 중인 대화가 둘 이상이면 남은 개수를 안내하고, 없으면 대기 항목이 없다고 안내한다). window-controls-overlay 모드에서는 헤더가 창 조작 버튼과 겹치지 않도록 여백을 확보하며, 헤더 빈 영역을 드래그해 창을 이동할 수 있다.
- `index.html`은 스크립트 실행 전 브라우저 프레임 색상을 맞추기 위해 라이트·다크 미디어 쿼리별로 `theme-color` 메타 태그를 하나씩 둔다. 런타임에는 `theme-boot.js`와 `theme.ts`가 저장된 팔레트 값으로 이를 갱신하며, 사용자가 라이트 또는 다크 모드를 직접 지정하면 두 메타 태그 모두 해당 캔버스 색상으로 고정한다. iOS 환경을 위해 `apple-mobile-web-app-capable`, `black-translucent` 상태바 스타일, 타이틀, 180×180 크기 `apple-touch-icon`, `viewport-fit=cover`를 적용하고 헤더·대화 열·입력창·드로어·바텀시트의 좌·우·하단에 `env(safe-area-inset-*)` 패딩을 둘러 가로 모드 노치 영역까지 대응한다. 단, 실제 iPhone 실기기에서의 상태바 가독성은 자동화 점검으로 검증하지 못했음을 밝혀 둔다.
- Web Share Target: 모바일 공유 시트에서 `/share-target`으로 `multipart/form-data`(`title`, `text`, `url`, `image/*` 또는 `text/plain` 타입의 `files`)를 POST 전송한다. 요청 본문은 `sw.js`만 읽을 수 있으므로, 서비스 워커가 제목·본문·URL과 최대 4개의 파일 바이트를 `IndexedDB`(`openside-pwa` 데이터베이스의 `share` 저장소)에 저장한 뒤 `/?share=<id>`로 303 리다이렉트한다. 읽기에 실패하면 `/?share=failed`로 이동해 앱에 오류 토스트를 띄운다. 로그인 후 앱이 해당 항목을 불러와 삭제하고 URL 쿼리를 정리한 다음, 입력창에 공유받은 내용(제목, 텍스트, URL이 서로 다르면 각각 한 번씩 반영)과 첨부파일을 채워 새 대화 화면을 연다. 한 시간 동안 열지 않은 항목은 삭제한다. 데몬의 첨부파일 제한(2 MiB)을 넘는 이미지는 크기가 맞을 때까지 해상도를 줄이고 JPEG 품질을 낮춰 다시 인코딩한다. 공유 대상 기능은 서비스 워커가 페이지를 제어하는 설치된 앱 상태에서만 동작하며, 데몬의 `/v1` API는 직접 호출하지 않는다.
- 배지: 앱이 포그라운드에 있는 동안에는 15초마다, 그리고 백그라운드에서 복귀할 때마다 `GET /v1/tickets?status=pending`을 호출한다. 대기 중인 티켓 수에 맞춰 `navigator.setAppBadge`를 설정하고(0개이거나 로그아웃하면 초기화), 대기가 풀린 티켓 알림은 닫으며 새 티켓이 들어오면 짧은 햅틱 반응을 준다. 앱이 닫혀 있을 때는 `sw.js`가 화면에 남아 있는 'ticket' 알림 개수로 배지를 표시하고 알림을 닫거나 누르면 이를 갱신한다. Badging API를 지원하지 않는 브라우저에서는 아무 동작도 하지 않는다.
- 오프라인 보관함: 네트워크 오류나 상태 코드 0처럼 데몬에 닿지 않아 전송에 실패한 메시지는 오류로 처리하지 않는다. 해당 메시지는 고유 `message_id`와 함께 `IndexedDB`의 `outbox` 저장소에 보관하며(새 대화 생성 메시지는 대화 생성에 필요한 데이터도 함께 보관), 입력창 상단에 대기 건수와 함께 'Send now', 'Discard queued messages' 버튼을 표시한다. 보관된 메시지는 `online` 이벤트가 발생하거나 앱이 화면에 다시 나타날 때, 또는 대기 항목이 있는 동안 10초 주기로 오래된 순부터 순차 전송한다. 전송 중 데몬 접속 불가 징후가 감지되면 즉시 전송을 중단한다. 데몬이 `message_id`로 중복을 걸러내므로 응답 유실 후 재전송해도 중복 생성되지 않으며, 새 대화 생성 전송이 유실된 경우라도 대화는 정확히 한 번만 생성되고 사용자가 빈 새 대화 화면에 머물러 있다면 생성된 대화로 바로 이동한다. 401, 429, 5xx 응답은 나중에 재시도하지만, 그 외의 거부 응답을 받으면 해당 메시지를 실패 상태로 바꾸고 실패 사유를 표시한 뒤 사용자가 삭제할 때까지 유지한다. 여러 탭이 같은 메시지를 중복 전송하지 않도록 Web Lock과 `BroadcastChannel`로 동기화한다.
- 제스처: 화면 왼쪽 가장자리(24 px 이내)에서 오른쪽으로 56 px 이상 수평에 가깝게 스와이프하면 모바일 대화 드로어가 열리고, 열린 드로어를 왼쪽으로 스와이프하면 닫힌다. 바텀시트는 핸들이나 헤더를 잡고 아래로 96 px 이상 드래그하거나 빠르게 튕겨 내리면 닫힌다(동작 줄이기 설정 사용자는 제외). 문서 전체에 `overscroll-behavior: none`을 적용해 당겨서 새로고침(pull-to-refresh)과 내부 스크롤이 서로 충돌하지 않도록 막는다. 화면 가장자리를 시스템 뒤로 가기 제스처로 사용하는 브라우저에서는 가장자리 스와이프가 가로채일 수 있으나, 드로어 버튼으로는 언제든 열고 닫을 수 있다.
- 햅틱: `navigator.vibrate`를 써서 티켓을 승인하거나 답변했을 때는 짧은 진동 1회, 반려하거나 거절했을 때는 진동 2회, 새 티켓이 도착했을 때는 가벼운 알림 진동을 준다. Android Chrome에서만 동작하며, Vibration API가 없는 iOS에서는 동작하지 않는다.
- 설치 안내: 아직 앱을 설치하지 않은 터치 기기 기준, Android에서는 `beforeinstallprompt` 이벤트를 가로채 앱 로드 4초 뒤 설치 배너를 띄운다. 배너를 닫으면 14일 동안 다시 표시하지 않는다. iOS 브라우저 탭에서는 공유 메뉴를 눌러 `Add to Home Screen`을 선택하도록 안내하는 배너를 띄우며, 한 번 닫으면 다시 표시하지 않는다. 이미 설치된 앱 내부나 자체 설치 인터페이스를 갖춘 데스크톱 브라우저에서는 배너를 표시하지 않는다.

## 5. F-1 체크리스트
| 기능 | API | UI 위치 | 상태 |
| --- | --- | --- | --- |
| 토큰 입력·로그아웃 | `GET /v1/health` | `TokenEntry`, Settings 다이얼로그의 Sign out 버튼 | 구현 |
| 대화 목록 | `GET /v1/sessions` | `Sidebar`(데스크톱 사이드바, 820px 이하는 Conversations 드로어) | 구현 |
| 검색 | `GET /v1/sessions?q=` | 사이드바 검색창 | 구현 |
| 고정 | `PATCH /v1/sessions/{id}` `{pinned}` | 대화 Options 메뉴 Pin/Unpin | 구현 |
| 보관 | `PATCH /v1/sessions/{id}` `{archived}`, `?archived=true` | 대화 Options 메뉴 Archive/Unarchive, Archived 탭 | 구현 |
| 새 대화 및 이름 변경 | `POST /v1/sessions`, `PATCH /v1/sessions/{id}` `{title}` | 첫 메시지를 전송할 때 새 대화가 생성되며, 첫 응답 이후 모델 제목이 붙는다. 대화 메뉴에서 이름을 변경할 수 있다. | 구현됨 |
| 라이브 스트리밍(SSE) | `GET /v1/sessions/{id}/events` | `SessionPage` 타임라인, Thinking 표시 | 구현 |
| 도구 활동 접기 | (이벤트 `tool.*`) | `Timeline` 의 `<details>` | 구현 |
| 큐 편집·삭제 | `PATCH`·`DELETE /v1/sessions/{id}/queue/{mid}` | `QueueList`: 대기 메시지마다 "..." 메뉴(Edit message, Cancel message). | 구현 |
| steer / Send now | `POST /v1/sessions/{id}/messages` `mode`, `POST .../queue/{mid}/promote` | `Composer`는 항상 대기열 추가(`mode: "queue"`). 대기 메시지 메뉴의 Send now로 현재 실행에 즉시 반영(`promote`). 승인 대기 중에는 입력창 placeholder가 Follow up으로 바뀌고 "Waiting for you above" 알약 버튼 클릭 시 해당 승인 카드로 스크롤. | 구현 |
| 중지 | `POST /v1/sessions/{id}/stop` | 세션 화면 Stop | 구현 |
| 모델 선택 | `GET /v1/settings`, `POST /v1/sessions` `model` | `ModelPicker` | 구현 |
| 티켓·승인 카드 | `POST /v1/tickets/{id}/resolve`·`/reject`·`/done` | `TicketCard`(confirm·info·device_approval·takeover) | 구현 |
| secret_input 입력 | `POST /v1/tickets/{id}/resolve` `{value}` | `TicketCard` 비밀번호 입력란 | 구현 |
| 화면 테마 | — | 설정 대화상자의 Appearance 섹션(`AppearanceSettings`): Theme mode(System/Light/Dark), Light theme·Dark theme 팔레트 프리셋, Primary color(색상 피커, 프리셋 스와치, Theme default). 브라우저 로컬 저장; 첫 렌더링 전 `public/theme-boot.js`가 즉시 적용. | 구현 (`web/src/components/AppearanceSettings.test.tsx`) |
| 반응형 | — | `styles.css`(모바일 우선) | 구현 |
| 데몬 정적 서빙 | `[ui] dir` | — | 구현 |
| PWA manifest·서비스 워커 | — | `web/public` | 구현(`theme-boot.js` 포함 오프라인 셸) |
| 서브에이전트 보기 | `subagent.*` 이벤트, `POST /v1/sessions/{id}/subagents/{sid}/steer`·`/cancel` | `AgentsTray`: 상태, 진행 이벤트 목록, Steer·Cancel. 승인 카드의 칩에는 서브에이전트 task(미확인 시 id 앞 8자리)를 표시하고, 전체 id는 `data-subagent-id`에 유지. | 구현 (`web/src/components/SubagentPanel.test.tsx`, E2E `web_ui_subagent_view_and_approval_resume_same_call_and_reload_rebuilds_screen`) |
| 입력창 스킬 멘션 칩 | `GET /v1/skills?q=&session=&enabled=true` | `Composer`: `/` 를 치면 후보 목록(↑↓·Enter·Tab), 멘션한 스킬은 칩으로 표시하고 칩에서 지울 수 있다. 멘션 인식과 칩 삭제는 사이드카와 같은 경계 규칙(공백·괄호·구두점)을 쓴다 | 구현 (`web/src/components/Composer.test.tsx`, E2E `web_ui_subagent_view_and_approval_resume_same_call_and_reload_rebuilds_screen`: 실제 데몬 스킬 → 선택기 → 칩 → 전송 → `skill` 로드) |
| visual 미리보기·소스 보기 | `GET /v1/sessions/{id}/visuals` 의 `preview_url` | `Timeline` 의 `VisualFrame`: 빈 `sandbox` iframe(스크립트 없음, opaque origin), Source/Preview 토글. 미리보기 URL 은 9분마다 다시 받는다. 스크립트로 그리는 차트는 실행하지 않는다(정적 HTML/SVG 차트만) | 구현 (`web/src/components/Timeline.test.tsx`, E2E 위와 같음: SVG·HTML·정적 막대 차트를 프레임 밖 스크린샷 픽셀로 확인) |
| 파일 목록·미리보기 | `GET /v1/sessions/{id}/artifacts`, `.../artifacts/{path}`(토큰), `.../preview` | `FilesTray`: 경로·출처·크기, 선택한 파일을 sandbox iframe 으로 미리보기, 텍스트는 Source(앞 200,000바이트까지, 잘렸으면 전체 크기와 Download 안내), Download | 구현 (`web/src/components/FilesTray.test.tsx`) 파일 목록은 작성기 도구 행의 "N files" 알약 버튼을 누르면 시트(행 구성: 아이콘, 이름, 크기, 열기, 다운로드)로 열리며, 에이전트 목록도 마찬가지로 "N agents" 버튼에서 열린다. |
| 스킬 | `/v1/skills*` | `/skills`: 사용자 스킬 목록 및 로드 실패 폴더와 사유 표시, 스킬 생성, 설명·본문 편집, 활성화·비활성화, 삭제 | 구현 (`web/src/pages/SkillsPage.test.tsx`) |
| 루틴 | `/v1/routines*` | `/routines`: 주기 및 다음 실행 시각 목록 표시, 생성(타임존 지원 크론·간격 반복·단발 실행, 새 대화 또는 기존 대화에서 실행), 일시정지·재개, 즉시 실행, 실행 이력 확인, 삭제, 반복 요청에서 제안된 루틴 바로 생성 | 구현 (`web/src/pages/RoutinesPage.test.tsx`) |
| 메모리 | `/v1/memory*` | `/memory`: 검색 모드 전환(키워드 전용 / 시맨틱), 종류별 필터 검색, 태그 및 보존 기간 지정 생성, 수정, 삭제, 버전 이력 확인, 시크릿 거부 사유 안내 | 구현 (`web/src/pages/MemoryPage.test.tsx`) |
| 계정 및 사용량 | `GET /v1/accounts` | `/accounts`: 자격증명 소스별 마스킹된 계정, 모델, 사용량 주기(백분율 및 리셋 시각), 레이트 리밋 차단 상태 표시, 사용 불가 소스 및 사유 안내 | 구현 (`web/src/pages/AccountsPage.test.tsx`) |
| 볼트 | `GET /v1/vault`, `POST /v1/vault/setup`·`unlock`·`lock`, `GET`·`POST /v1/vault/items`, `PATCH`·`DELETE /v1/vault/items/{id}`, `GET /v1/vault/audit` | `/vault`: 상태 카드(passphrase 또는 키링 설정, 잠금 해제, 잠금), 로그인 또는 인증자 추가(`otpauth://` 링크나 설정 키 입력, 값은 데몬으로만 전송되며 화면에 다시 표시되지 않음), 확인 대화상자를 거치는 삭제, 민감 정보가 제외된 항목 메타데이터, 항목별 사용 정책 설정, 최신순 감사 로그(항목/세션/시간 필터 지원). `admin` 스코프가 없는 기기에는 권한 획득 방법 안내. 모바일 화면 너비 지원 | 구현 완료(`web/src/pages/VaultPage.test.tsx`, E2E `paired_phone_adds_an_authenticator_to_the_vault`, `paired_web_device_manages_devices_vault_audit_and_settings`) |
| Settings 및 Workspace 다이얼로그 | `GET /v1/health` | `/settings`: 현재 화면 위에 Settings 다이얼로그 오버레이(모바일은 바텀 시트): Workspace 진입점, Connection(Daemon 상태 및 API version 표시, 열려 있는 동안 `GET /v1/health`로 5초마다 재확인), Notifications, Appearance, Sign out. `/workspace`는 Skills, Routines, Memory, Accounts, Vault, Devices, Configuration 타일로 구성된 Workspace 다이얼로그를 띄움. 각 섹션은 전용 라우트(`/skills`, `/routines`, `/memory`, `/accounts`, `/vault`, `/devices`, `/config`)를 제공하며, 해당 라우트로 직접 접근하면 새 채팅 위에 다이얼로그를 열고 Close 시 원래 화면으로 복귀. 삭제 및 기기 폐기 시 확인 다이얼로그 표시 | 구현 (`web/src/App.manage.test.tsx`, `web/src/pages/SettingsPage.test.tsx`) |
| 설정 | `GET`·`PUT /v1/settings/config` | `/config`(Workspace → Configuration): 데몬 스키마 기반 동적 폼 생성(전체 키 대상, 페어링된 기기에서는 로컬 전용 키를 읽기 전용으로 표시), 문서 전체를 보여주는 JSON 탭, 재시작 안내 표시 | 구현 (`web/src/pages/ConfigPage.test.tsx`, E2E 위와 같음) |
| 기기 | `GET /v1/devices`, `DELETE /v1/devices/{id}` | `/devices`: 페어링된 기기 목록 및 스코프 표시, 현재 기기 표시, 확인 다이얼로그를 거친 폐기, 새 기기 페어링 안내 | 구현 (`web/src/pages/DevicesPage.test.tsx`, E2E 위와 같음) |
| 브라우저 라이브 카드(JPEG)·브라우저 시트 | `GET /v1/sessions/{id}/browser/preview` (WebSocket, `browse` 또는 이를 포함하는 `steer` 스코프 필요. takeover 티켓 엔드포인트 아님), `GET /v1/browser/tabs` (read) | 세션 화면에서 대화가 브라우저를 사용하면 대화 열 상단에 라이브 브라우저 카드(`BrowserPreview`)를 표시하며, takeover 티켓 없이 에이전트의 활성 탭을 추적한다. 에이전트가 실행 중일 때나 카드가 슬림 바로 접혔을 때만 헤더 아래에 고정되고, 유휴 상태이거나 사용자 확인을 기다릴 때는 대화와 함께 스크롤되어 요청 카드를 가리지 않는다. 카드는 흰색 배경에 탭의 읽기 전용 실시간 이미지를 보여준다. 카드 하단 테두리에는 상태 칩(실행 중에는 `components/toolStatus.ts`를 바탕으로 `Clicking on nid.naver.com` 등 에이전트의 현재 동작, 유휴 상태일 때는 페이지가 화면에 보일 때 2초마다 원장 API `GET /v1/browser/tabs`를 조회해 현재 세션 소유 탭의 탭 ID로 확인한 라이브 탭 제목)이 겹쳐 표시된다. 세션에 대기 중인 티켓이 생기면 상태 표시가 `Action needed`로 바뀌고, 탭하면 해당 요청 위치로 스크롤된다. `Minimize`를 누르면 카드 전체가 얇은 막대 형태(썸네일, 상태, `Restore`, ×)로 접힌다. 실행 중 ×를 누르면 `Stop everything?`(`Keep running` / `Stop everything`)를 확인한 뒤 작업을 중단하고, 유휴 상태에서는 에이전트가 브라우저를 다시 쓸 때까지 카드를 숨긴다. `browser_pointer` 좌표 클릭 시 해당 위치에 흰색 리플이 표시되며, 엘리먼트 참조를 사용하는 `browser_act`는 리플을 표시하지 않는다. 1초 이상 지속된 실행이 끝나면 상태 표시에 `✓`와 최종 응답의 첫 문장이 나타나고 4초 뒤 카드가 접힌다. 세션에 열린 탭이 없으면 카드는 숨겨진다. 이미지를 탭하면 브라우저 시트가 열리며, 여기에는 라이브 탭 제목, 서버에서 쿼리와 프래그먼트를 제거한 URL, 사용자의 실제 브라우저로 여는 `Open in your browser` 링크, 실시간 읽기 전용 이미지, 스트리밍 중인 탭에 `Live`가 표시되는 `Tabs in this conversation` 목록이 포함된다. 시트 내 이미지를 탭(`Zoom browser preview`)하면 프레임을 계속 수신하는 100~500% 확대 가능한 전체 화면 읽기 전용 뷰어가 열린다. 뷰어를 닫으면 시트로, 시트를 닫으면 카드로 돌아간다. 브라우저 컨트롤러 연결이 끊기면(503 `browser_disconnected`) 실시간 이미지는 유지된 채 시트에 연결 해제 상태가 표시된다. API에 해당 엔드포인트가 없으므로 내비게이션·탭 전환·탭 닫기 UI는 제공하지 않는다. 사람이 직접 조작(takeover)하는 동안에는 화면을 비우고 일시정지했다가 끝난 뒤 자동으로 재개된다. `secret_input` 값이 전달된 적이 있는 탭(이동 후 포함)과 비공개 세션은 절대 노출하지 않는다. 세션이 멈추면 열려 있던 미리보기도 종료된다. | 구현 (`web/src/components/BrowserPreview.test.tsx`, `crates/opensided/src/tests/preview.rs`, `crates/openside-e2e/tests/web_pip.rs`) |
| takeover 화면(screencast) | `GET /v1/tickets/{id}/takeover`(WebSocket). 브라우저는 헤더를 못 넣으므로 `Sec-WebSocket-Protocol: openside.v1, openside.bearer.<token>` | `TicketCard` 의 Open remote screen → `TakeoverView`: 프레임 표시, 포인터·휠·키·붙여넣기를 1440x900 좌표로 전달, withheld·input_error·ended 안내, Done/Reject | 구현 (`web/src/components/TakeoverView.test.tsx`, E2E `web_ui_secret_input_and_takeover_screen_resume_the_agent`) |
| 이미지 줌·첨부 업로드 | `POST /v1/sessions/{id}/messages` `attachments[{name,data_base64}]`, `GET .../artifacts/{path}` | `Composer` Attach(썸네일 칩, 첨부만 단독 전송 가능). 데몬과 같은 한도(첨부 4개, 합계 2 MiB, 본문 UTF-8 1 MiB)를 파일을 읽기 전에 검사하여 넘치면 오류를 표시한다. 타임라인 첨부 이미지 클릭 시 `ImageViewer` 실행: 전체 화면, 버튼·`+`/`-`/`0` 키·핀치 조작을 통한 100~500% 확대, 드래그 이동(pan), Esc로 닫기. | 구현 (`web/src/components/{Composer,Timeline}.test.tsx`, E2E 첫 번째 시나리오) |
| 로그 재생만으로 화면 복원 | `GET /v1/sessions/{id}/events` | `reduceSession` 이 서브에이전트·파일·첨부까지 이벤트만으로 만든다 | 구현 (`web/src/hooks/useSessionView.test.tsx`, E2E 새로고침 비교) |
| 페어링(QR) 및 웹 푸시 | 상단 "페어링·푸시 API" 표 참고 | `/pair#<code>`: 1회용 코드를 교환하여 기기 토큰 저장 후 주소창에서 코드를 제거하고 앱 진입(만료·사용 완료·잘못된 코드 오류 안내), `/settings` Notifications: 사용자가 Enable notifications를 눌렀을 때만 권한 요청(페이지 로드 시점 요청 금지), 이후 데몬의 VAPID 키로 서비스 워커를 통해 구독 생성 및 등록. Turn off notifications 클릭 시 구독 해제. 홈 화면에 추가되지 않은 iPhone/iPad 브라우저 탭에서는 버튼 대신 Add to Home Screen 안내 표시. 화면 포커스 변경 시 및 30초 주기로 `POST /v1/push/presence`를 호출해 상태 보고 | 구현 (`web/src/App.manage.test.tsx`, `web/src/pages/SettingsPage.test.tsx`, E2E `paired_web_device_manages_devices_vault_audit_and_settings`). iOS 홈 화면 앱·Android·데스크톱 환경의 실제 푸시 수신은 실기기 및 푸시 서비스가 필요하므로 본 테스트에서는 다루지 않는다. |
| 공유 대상(Web Share Target) | `POST /share-target`(서비스 워커가 처리, 데몬 API 아님) | 공유받은 텍스트와 첨부 이미지가 미리 채워진 새 대화 입력창 | 구현 (`web/src/sw.test.ts`, `web/src/pwa/launch.test.ts`, `web/src/components/Composer.offline.test.tsx`, E2E `web_pwa`) |
| PWA 바로가기·앱 배지·햅틱 | `GET /v1/tickets?status=pending` | 매니페스트 바로가기 `New chat`/`Needs you`, 대기 티켓 수가 표시되는 앱 아이콘 배지, Android 티켓 승인·반려 햅틱 피드백 | 구현 (`web/src/pwa/pwa.test.tsx`, `web/src/pwa/launch.test.ts`, `web/src/sw.test.ts`, E2E `web_pwa`). 실제 기기의 아이콘 배지와 진동은 실기기가 필요하므로 본 테스트에서는 다루지 않는다. |
| 오프라인 보관함(outbox) | `POST /v1/sessions`, `POST /v1/sessions/{id}/messages`(`message_id` 기반 멱등성 보장) | 입력창 상단의 'Send now'·'Discard queued messages' 안내, 네트워크 복구 시 자동 전송 | 구현 (`web/src/pwa/outbox.test.ts`, `web/src/components/Composer.offline.test.tsx`, E2E `web_pwa`) |
| 제스처·설치 안내 | — | 왼쪽 가장자리 스와이프로 드로어 열기, 드로어 좌측 스와이프 및 바텀시트 하향 드래그로 닫기, Android 설치 배너 및 iOS `Add to Home Screen` 일회성 안내 | 구현 (`web/src/ui/Dialog.test.tsx`, `web/src/pwa/pwa.test.tsx`, E2E `web_pwa`) |
| 모든 기능은 피처 플래그나 유료 티어 제한 없이 동작 | — | Workspace 다이얼로그 내 각 섹션 | 구현 (`web/src/App.manage.test.tsx`, `crates/opensided/src/tests/remote.rs::no_route_tool_or_setting_is_behind_a_feature_flag_or_paywall`) |

아티팩트 목록, 시각화 자료, 트랜스크립트 내보내기는 배타적 `after_seq`를 사용하여 모든 이벤트 페이지를 빠짐없이 조회한다. 이를 통해 처음 512건을 넘어선 이벤트까지 모두 포함하며, 트랜스크립트 최종 이벤트 시퀀스가 세션의 마지막 이벤트까지 온전히 이어지도록 보존한다.

## 6. UI 전송 계층 및 프리뷰 격리

웹 프론트엔드는 전송 모드에 관계없이 동일한 `client.ts` 인터페이스를 통해 데몬 API와 통신한다. 릴레이 연결 시에는 `api/relay` 모듈이 Noise 핸드셰이크, 크레딧 기반 다중화(mux), HTTP/1.1 청크 파서, SSE 및 WebSocket 중계 역할을 대신 처리하므로, UI 레이어에서는 기존의 엔드포인트 규격과 스코프 권한, 로컬 소켓 전용 보호 기능을 그대로 유지한다.

- **스토리지 및 세션 초기화**: 기기 인증 토큰과 전송 방식 관련 공개 메타데이터는 브라우저의 `localStorage`에 보관된다. `401 Unauthorized` 응답을 받으면 저장된 토큰과 메타데이터를 즉시 비운다. 이때 릴레이 오리진에서는 직접 토큰을 입력할 수 없으므로 `openside pair --via relay`를 실행하라는 안내 화면을 띄워 새 QR 코드를 통한 재페어링을 유도한다. 직접 접속 오리진에서는 기존의 토큰 입력 폼을 그대로 제공한다.
- **오리진별 프리뷰 처리**: 데몬 직접 접속이나 Tailscale 환경에서는 서명된 원래의 HTTP URL과 데몬 응답 CSP 헤더를 그대로 사용해 iframe을 렌더링한다. 반면 릴레이 환경(`client.relay === true`)에서는 터널을 통해 서명된 바이트를 가져온 뒤, HTML·SVG·텍스트 콘텐츠를 엄격한 메타 CSP(`default-src 'none'; script-src 'none'; style-src 'unsafe-inline'; img-src data:; font-src data:; media-src data:; form-action 'none'; base-uri 'none'`)가 주입된 HTML로 감싸 임시 Blob URL(`blob:`)로 변환해 화면에 렌더링한다. 해당 Blob URL은 컴포넌트가 언마운트되거나 소스가 변경되는 즉시 해제(`URL.revokeObjectURL`)된다. 바이너리 PDF는 원본 바이트와 MIME 타입을 보존한 채 메타 태그 주입 없이 빈 샌드박스 iframe 및 상속된 셸 CSP 정책에 따라 보존되며, 브라우저 고유의 PDF 바이너리 렌더링 동작은 별도로 검증하지 않았다.
- **샌드박스 및 셸 CSP 제어**: 프리뷰 iframe은 빈 샌드박스 속성(`sandbox=""`)과 `referrerpolicy="no-referrer"`를 적용한다. 모든 셸 페이지는 스크립트 실행 제어로 `script-src 'self'`를 적용해 애플리케이션 자체 스크립트만 실행하고 인라인 스크립트를 차단하며, iframe 로딩을 위해 `frame-src 'self' blob:`을 공통 허용한다. 직접 접속 환경과 달리 릴레이 환경의 Blob 프리뷰는 셸의 스타일 제약을 상속받으므로, 릴레이 서빙 셸에 한해 차트 및 인라인 CSS 렌더링을 위한 `style-src 'self' 'unsafe-inline'`을 허용한다. 프리뷰 내부의 스크립트 실행은 메타 CSP(`script-src 'none'`)로 원천 차단된다.
- **자동화 검증 범위**: 자동화 테스트는 사설 로컬 릴레이 환경에서 실제 스타일 계산값(`rgb(47, 111, 237)`), 픽셀 렌더링, 프리뷰 스크립트에 의한 DOM 변조 및 네트워크 프로브 차단 여부, 그리고 데몬/Tailscale 서빙 셸의 엄격한 `style-src 'self'` 유지 여부를 검증 항목으로 다룬다(Safari 및 iOS 환경은 별도 검증 대상에 포함되지 않는다).

```sh
pnpm -C web build
flock --close /tmp/openside-heavy.lock nice -n 10 cargo test -j 4 -p openside-e2e --test remote_relay -- --test-threads=2
flock --close /tmp/openside-heavy.lock nice -n 10 cargo test -j 4 -p opensided remote_transport -- --test-threads=2
flock --close /tmp/openside-heavy.lock nice -n 10 cargo test -j 4 -p openside-relay -- --test-threads=2
pnpm -C web test src/api/relay src/api/preview.test.ts src/pages/PairPage.test.tsx src/components/VisualFrame.test.tsx src/App.test.tsx
```

## 7. 검증
- `pnpm -C web lint && pnpm -C web typecheck && pnpm -C web test && pnpm -C web build`
- vitest: `web/src/api/{sse,client,uiApiMap}.test.ts`, `web/src/state/session.test.ts`, `web/src/hooks/useSessionView.test.tsx`, `web/src/sw.test.ts`(서비스 워커의 공유 대상·배지 동작), `web/src/pwa/{outbox,launch,pwa}.test.ts(x)`, `web/src/ui/Dialog.test.tsx`(바텀시트·드로어 제스처), `web/src/App.test.tsx`, `web/src/App.manage.test.tsx`, `web/src/components/{Sidebar,AppearanceSettings,TicketCard,QueueList,SubagentPanel,Composer,Composer.offline,Timeline,FilesTray,TakeoverView,BrowserPreview}.test.tsx`(`Composer.offline`은 오프라인 큐 적재와 공유 초안 적용), `web/src/pages/{NewChat,Skills,Routines,Memory,Accounts,Vault,Devices,Settings,Config}Page.test.tsx`
- 시각 QA 스크린샷: `tools/qa/web-screens.sh [out-dir]` (헤드리스 Chromium으로 모든 웹 화면을 라이트·다크 테마 및 390×844·1440×900 해상도 PNG 파일로 저장한다. 테스트 코드는 `crates/openside-e2e/tests/web_screens.rs`에 있고 `#[ignore]` 처리되어 스크립트로만 실행한다).
- PWA E2E: `cargo test -j 4 -p openside-e2e --test web_pwa -- --test-threads=2`. 프로덕션 번들을 제공하는 실제 데몬을 상대로 390×844 해상도의 터치 모바일 환경을 에뮬레이트하는 Headless Chromium 환경에서 검증한다. 매니페스트 및 `theme-color` 메타 태그의 적절성, 공유 시트에서 `/share-target`으로 제목·본문·URL·이미지를 POST했을 때 새 대화 입력창에 초안과 첨부파일이 유지되는지 여부, 해당 내용을 전송했을 때 공유 제목을 이름으로 한 대화방이 생성되고 이미지가 저장되는지 여부, 2 MiB 초과 이미지를 규격에 맞게 축소하는지 여부, 네트워크 단절 시 메시지가 IndexedDB에 대기열로 적재된 뒤 복구 시 단 하나의 대화방만 생성되어 메시지가 정상 전달되는지 여부, 설치 배너 노출 여부, 화면 좌측 가장자리 터치 스와이프로 드로어가 열리는지 여부를 포괄한다. `web/dist` 대신 자체 빌드 결과물을 쓰려면 `OPENSIDE_PWA_DIST` 환경변수를 설정한다. 번들이나 Chromium이 없으면 사유를 출력하고 테스트를 건너뛴다.
- UI-API 맵 재생성: `pnpm -C web gen:ui-map` (`uiApiMap.test.ts`가 최신 상태 여부를 검증한다).
- 데몬: `cargo test -j 8 -p opensided web::`
- E2E: `cargo test -j 8 -p openside-e2e --test web`. 실제 데몬과 스크립트 사이드카를 띄우고, 헤드리스 Chromium 으로 토큰 입력 → 세션 생성 → 메시지 전송 → `ask_user` 카드에서 답 선택 → 세션이 답을 받아 끝나는 것까지 확인한다. `web/dist` 나 Chromium 이 없으면 이유를 출력하고 건너뛴다.
- 세션 화면 E2E: `cargo test -j 8 -p openside-e2e --test web_ui`. 실제 데몬·스크립트 사이드카·헤드리스 Chromium 으로 확인한다.
  - `web_ui_subagent_view_and_approval_resume_same_call_and_reload_rebuilds_screen`: 이미지 첨부와 `/rep` → 선택기에서 고른 스킬 칩 → 전송하면 그 스킬이 모델보다 먼저 로드됨 → 서브에이전트가 목록에 나타나 승인 대기 → 카드에서 Approve → 같은 `child-write` 호출이 실행되고 부모까지 끝남. 답은 말풍선 하나, SVG·HTML·정적 차트 visual 은 스크립트 없는 `img` 로 실제로 그려지고(스크린샷 픽셀) Source 토글은 원문을 보인다. 새로고침 뒤 화면 텍스트와 `lastSeq` 가 같다.
  - `pp22_phone_viewport_approval_resumes_the_same_task`: 390x844 모바일 에뮬레이션에서 승인 카드가 화면 폭 안에 들어오고, Approve 후 같은 `write` 호출이 한 번만 재개된다.
  - `web_ui_secret_input_and_takeover_screen_resume_the_agent`: secret_input 을 UI 로 넣으면 `filled` 로 재개되고 canary 가 이벤트·기록·DOM 에 없다. takeover 화면이 프레임을 받고, UI 의 클릭·타이핑이 탭에 전달되며, Done 후 같은 호출이 재개된다.
- 페어링·푸시: `cargo test -j 8 -p opensided tests::remote`, `cargo test -j 8 -p openside-push`, `cargo test -j 8 -p openside-e2e --test remote --test push_browser`. `push_browser` 는 데몬이 127.0.0.1 가짜 푸시 서비스로 보낸 암호문을 복호화한 바이트를 DevTools `ServiceWorker.deliverPushMessage` 로 실제 `sw.js` 에 넣어, 알림 제목과 deep link 를 확인한다. Chromium 이 없으면 이유를 출력하고 건너뛴다.
- 관리 기능 E2E 테스트: `cargo test -j 8 -p openside-e2e --test web_admin`. 실제 데몬과 임시 볼트를 띄운 뒤 TCP 상의 헤드리스 Chromium이 `POST /v1/pairing`으로 발급된 `/pair#<code>` 링크에 접속하여 로그인한다(사용된 코드는 즉시 소진). 탐색은 사이드바의 Workspace 링크와 Workspace 다이얼로그 타일을 거치며, Devices 화면에서는 현재 기기를 확인하고 Vault에서는 항목 정책 변경 후 생성된 `policy_update` 감사 로그를 조회하며 Configuration 폼에서는 `tickets.confirm_ttl_secs`를 저장(config.toml에 반영)한 다음, JSON 탭에서 `policy.mode`를 수정할 때 로컬 소켓 전용 거부 응답을 받는지 검증한다. `web/dist` 빌드 결과물이나 Chromium이 없으면 사유와 함께 건너뛴다.

## 8. 로컬 원격 접속 설정

### 소유자 브라우저 및 로컬 권한

저장소 실행 도구는 `tools/openside-browser`다(`openside` CLI와 같은 위치에 설치). 이 도구는 전용 로컬 소켓을 통해 `openside --json remote local-access`를 실행한 뒤, **0700 디렉터리**(전용 `XDG_RUNTIME_DIR` 또는 macOS의 전용 임시 디렉터리) 내부의 단발성 **0600 HTML 파일**을 통해 브라우저에 루프백 프래그먼트 URL을 전달한다. 브라우저 argv에는 `file://` 경로만 포함되며 소유자 자격 증명은 절대 들어가지 않는다. meta-refresh로 UI에 리디렉션되며, 기존 브라우저 창에서 새 탭을 여는 경우를 포함해 30초 후 자격 증명이 담긴 파일과 디렉터리를 제한 시간 내에 정리·삭제한다. 최초 실행 시 `read`, `steer`, `approve`, `human_step`, `admin` 권한을 가진 `person_device`를 프로비저닝하며, admin 권한이 없는 기존 `openside-browser` 기기는 부트스트랩 성공 후 교체된다. 소유자 브라우저를 부트스트랩할 때는 QR 제안을 사용하지 않는다.

`GET /v1/remote/local-access`는 소켓 전용 엔드포인트다. 호출할 때마다 새로운 소유자 토큰을 생성하고 SQLite에 저장된 기존 관리형 부트스트랩 기기를 원자적으로 교체한다. 데몬이 재시작되더라도 표시 이름이 아니라 별도로 영속화된 부트스트랩 식별자를 통해 해당 기기를 식별한다. 새 부트스트랩 결과가 반환되기 전에 기존 스트림/폴링과 푸시 구독을 해지·정리하며 교체 내역을 감사 로그에 남긴다. 동일한 이름을 사용하더라도 CLI에서 의도적으로 발급한 기기는 건드리지 않는다. 기존 브라우저 로그인은 새 부트스트랩을 설치하면서 admin 권한을 얻으며, 표시가 없는 구형 기기는 단지 이름이 같다는 이유만으로 해지되지 않는다.

응답에는 기기 ID, 부팅별 로컬 시크릿, 그리고 `local_token`, `local_device`, `local_secret`이 담긴 실행 URL 프래그먼트가 포함된다. 웹은 즉시 프래그먼트를 제거하고 증명 값을 `sessionStorage`에만 보관하며, 로그아웃하거나 자격 증명이 거부되거나 ID가 변경되면 이를 삭제한다. 새 부트스트랩은 새 토큰을 저장한 뒤 새 증명을 설치한다. 이 증명은 QR, 감사 기록, localStorage, 원격 상태에 절대 기록되지 않는다. 데몬 재시작 후에는 `openside-browser`를 다시 열어야 하며, 다른 로컬 브라우저에서 열 때는 요청 시마다 토큰이 갱신되므로 `openside remote local-access --json` 출력 전문이나 실행 링크를 직접 붙여넣어야 한다. 해당 출력이나 링크는 절대 외부에 공유해서는 안 된다.

루프백이라고 해서 로컬 접속이 증명되는 것은 **아니다**. Tailscale Serve와 cloudflared 모두 원격 트래픽을 루프백 리스너로 프록시하기 때문이다. 자격 증명 발급 및 전송 방식 변경에는 전용 소켓이 필요하거나, 실제 루프백 TCP 피어, 문자 그대로의 루프백 Host, 프록시 오리진 헤더 부재, 소켓에서 발급된 로컬 기원 `person_device` admin 토큰, 그리고 현재의 `X-Openside-Local-Secret` 조건을 **모두** 만족해야 한다. 릴레이 리스너는 서버가 제어하는 오리진 표식을 추가하며 절대 로컬 권한을 얻지 못한다. Forwarded/Via, `X-Forwarded-*`, `CF-*`, `Tailscale-*` 헤더는 로컬 권한을 거부할 수만 있을 뿐 부여할 수는 없다. 임의의 프록시에는 전역적으로 신뢰할 수 있는 오리진 표식이 없으므로, 표식이 없는 루프백 요청이더라도 부팅별 시크릿이 필수다. 기존 토큰과 QR 교환 토큰은 admin이더라도 로컬 기원이 없으며, 요청을 통해 저장된 kind를 변경할 수 없다.

`POST /v1/pairing`은 180초 단조 만료 시간, 해시된 인메모리 코드, 단발성 교환, 코드를 제외한 감사 메타데이터 유지 방식을 그대로 유지한다. 요청 스코프는 발급자의 허용 범위 내에 있어야 한다(`scope_ceiling_exceeded`). 로컬 운영자가 제안 단계에서 `person_device` 또는 `agent_client`를 지정하며, 교환 시 kind를 재정의할 수 없다. admin이 아닌 호출자에게는 `forbidden`이 반환되고, 로컬 증명이 원격이거나 누락되었거나 만료된 경우에는 `local_access_required`가 반환된다. 기기 목록 조회 및 해지 기능은 변경되지 않으며, 위조 가능한 프록시 헤더로부터 전송 귀속을 추론하지 않는다.

### 실시간 전송 API

| 엔드포인트 | 요청 | 동작 |
| --- | --- | --- |
| `GET /v1/remote/status` | 선택적 local-secret 헤더 | 런타임 모드, 공개 URL, Tailscale 설치/Serve 상태, cloudflared 상태/URL/재시작 횟수/오류, 릴레이 등록/E2EE 상태, 영속화된 설정, `pairing_allowed`, `configure_allowed`, `restart_required`. 토큰 값은 제외. |
| `PUT /v1/remote/tailscale` | `{enabled}` | openside의 HTTPS 루트 Serve 핸들러만 활성화/비활성화한다. 다른 애플리케이션의 핸들러를 덮어쓰는 것은 거부하며(`tailscale_serve_conflict`), Serve reset은 절대 실행하지 않는다. |
| `PUT /v1/remote/cloudflare` | `{enabled,tunnel:"quick"\|"named",hostname,token_file}` | 기존 감독 프로세스를 실시간으로 시작/중지/재구성한다. 동일한 활성화 설정을 반복 호출해도 프로세스와 빠른 URL을 유지한다. 바이너리 설정은 파일 전용으로 유지된다. |
| `PUT /v1/remote/relay` | `{enabled,url,token_file}` | 기존 아웃바운드 릴레이 클라이언트를 실시간으로 등록/등록 해제한다. 변경 없이 활성화된 링크는 유지된다. 등록 자격 증명이 누락된 경우 안전한 설명과 함께 `failed`로 표시된다. |

모든 변경 요청에는 admin 및 앞서 언급한 로컬 권한이 필요하다. 기존 설정 키와 원자적 전용 설정 작성기는 구성/모델 업데이트와 동일한 쓰기 잠금 아래에서 재사용된다. 터널 및 릴레이 등록 토큰 값은 설정된 로컬 파일이나 환경 변수에만 유지되며 JSON이나 argv에는 절대 포함되지 않는다. TCP 리스너가 실행 중이지 않으면 직접 전송 설정은 영속화되지만 `restart_required`가 true가 되며, 재시작 시 리스너가 생성된다. 유효성 검사 오류가 발생해도 기존 런타임 상태는 변경되지 않는다. 실시간 적용된 원격 설정 때문에 일반 Configuration 문서에 재시작이 필요하다고 잘못 표시되지 않는다.

릴레이 `e2ee:true`는 Noise가 릴레이를 거치는 데몬과 기기 간 트래픽을 암호화함을 의미한다. 그렇다고 해서 릴레이가 제공하는 웹 코드가 운영자로부터 독립적이 되는 것은 **아니다**. 사용자는 해당 코드를 제공하는 주체를 반드시 신뢰해야 한다.

Tailscale 관리는 [현재 Serve CLI](https://tailscale.com/docs/reference/tailscale-cli/serve) 방식을 따른다. 즉, 핸들러 등록에는 `serve --bg --https=443 --set-path=/ <target>` 명령을 사용하고, 해당 핸들러만 삭제할 때는 동일한 경로 플래그에 `off`를 붙여 실행한다.

### Quick Tunnel 이벤트 전달

[Cloudflare Quick Tunnel은 SSE를 지원하지 않으며](https://developers.cloudflare.com/tunnel/get-started/quick-tunnels/#limitations), 실제 일회용 Quick Tunnel 프로브 테스트에서도 버퍼링된 이벤트 데이터가 업스트림이 닫힌 뒤에야 도착하는 현상이 확인되었다. 일반 SSE 라우트는 즉시 `: ok` 주석을 전송하므로, 웹 리더는 버퍼링 발생을 감지하고 유한 JSON 롱 폴링으로 전환할 수 있다.

`GET /v1/sessions/{id}/events/poll?after=<cursor>&wait=<seconds>&subagents=<boolean>` 요청에는 `read` 권한이 필요하다. 기본값은 `after=0`, `wait=20`, `subagents=true`이며 대기 시간은 0~25초로 제한된다. 응답 형식은 `{events,last_seq}`로, 순서가 보장된 최대 512개의 저장된 원본 이벤트다. `last_seq`는 아직 전달되지 않은 최신 시퀀스가 아니라, 필터링된 서브에이전트 이벤트를 포함하며 해당 위치 이후부터 다시 재생하기 위한 배타적(exclusive) 리플레이 커서다. 읽기 전 구독 방식을 통해 재연결 시에도 누락과 순서 역전이 방지되며, 권한 해지 시 대기 중인 호출이 즉시 깨어난다. 미래 시점 커서에는 `invalid_event_id`가 반환된다. 폴링 사이에 세션 삭제가 발생하더라도 최종 삭제 tombstone이 재생되며, 이후의 소진된 폴링은 404를 반환한다.

집중 백엔드 검증 내역: `crates/opensided/src/tests/remote_setup.rs`는 로컬/프록시/릴레이 권한, 스코프 상한, 고정 kind, 만료 및 단발성 사용, 가짜 cloudflared + 일회용 릴레이의 실시간 시작/중지/멱등성, 리스너 누락 시 재시작 보고, 실제 전용 소켓→루프백 페어링 스모크 테스트, 빈틈없는 이벤트 페이지/필터링/실시간 깨우기/해지/삭제, 부트스트랩 재사용/해지/부팅 간 로테이션을 검증한다. 이러한 픽스처는 실행 중인 사용자 데몬이나 Tailscale 구성을 전혀 변경하지 않는다.
