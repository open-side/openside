# 원격 UI (웹/PWA)

상태: **부분 구현**. 관련: F-1, E-4, PP-6, PP-8, PP-16, PP-22. 코드: `web/`(React 19 + TypeScript + Vite), `crates/opensided/src/{ui,session_meta}.rs`(정적 서빙·세션 메타데이터), `crates/openside-proto/src/session_meta.rs`(API 타입), `crates/openside-e2e/tests/web.rs`(헤드리스 E2E).

## 1. 구조
- UI는 데몬 HTTP API만 쓴다(E-4). 모든 요청은 `web/src/api/client.ts` 를 거치고, 타입은 OpenAPI 스냅샷(`docs/specs/openapi.json`)에서 `pnpm -C web gen:api` 로 만든 `web/src/api/schema.ts` 를 쓴다(PP-8). 스냅샷은 `cargo run -q -p openside-proto --example openapi > docs/specs/openapi.json` 로 갱신하고, 생성 문서와 다르면 `openside-proto` 테스트가 실패한다.
- 라이브 스트리밍은 `GET /v1/sessions/{id}/events` SSE 를 `fetch` + `ReadableStream` 으로 읽는다. EventSource 는 `Authorization` 헤더를 보낼 수 없어서 쓰지 않는다. 재연결할 때 `Last-Event-ID` 로 이어 받고, 이미 받은 `seq` 이하는 버린다. 401·403·404 는 재시도하지 않는다.
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
| `GET /v1/sessions?q=&archived=` | read | `archived` 기본값 false. `q` 는 제목과 첫 메시지를 대소문자 없이 찾는다. 고정한 세션이 먼저 나온다 |
| `PATCH /v1/sessions/{id}` | steer | `{title?, pinned?, archived?}`. 바뀐 필드만 `session.updated` 이벤트로 남긴다. 빈 요청은 400 `invalid_value`, 빈 제목이나 200자 넘는 제목은 400 `invalid_title` |
| `GET /v1/settings` | read | 기본 모델·대체 모델·정책 모드(모델 선택용). 자격증명은 없다 |
| `GET`·`PUT /v1/settings/config` | admin | `[credentials]`를 제외한 모든 설정 키를 JSON으로 조회하고 수정한다(수정 사항은 데몬 재시작 후 반영). 응답에는 설정 JSON Schema(`schema`), 페어링된 기기가 수정할 수 있는 키 목록(`remote_editable`, 끝이 `.`이면 해당 접두사 하위 전체), 호출자가 페어링 기기인지 여부(`remote`)가 함께 포함되며 설정 화면은 이를 기반으로 폼을 구성한다. TCP 연결에서 그 외 키를 수정하려고 하면 403 `local_socket_required`로 거부되고 `settings/refused_remote`로 감사 로그에 기록된다. 자세한 내용은 [`docs/CONFIGURATION.md`](../CONFIGURATION.md)를 참고한다 |

### 페어링·푸시 API
| 메서드·경로 | 권한 | 설명 |
| --- | --- | --- |
| `POST /v1/pairing` | admin, 로컬 소켓만 | `{name, scopes}` → `{code, path, url?, scopes, expires_at}`. 코드는 3분, 1회용. `openside pair` 가 QR 로 보여 준다 |
| `POST /v1/pairing/exchange` | 없음(코드가 자격) | `{code}` → `{device_id, token, scopes}`(201). 만료 410 `pairing_expired`, 사용했거나 없는 코드 404 `pairing_not_found` |
| `GET /v1/devices`, `DELETE /v1/devices/{id}` | admin | 기기 목록(이름·범위)과 즉시 폐기. 폐기하면 다음 요청이 401 이고 열린 SSE 가 닫힌다 |
| `GET /v1/push/vapid` | read | VAPID 공개키(`applicationServerKey`) |
| `GET`·`POST /v1/push/subscriptions`, `DELETE /v1/push/subscriptions/{id}` | read | 이 기기의 구독만 보고 고친다. `POST` 는 `PushSubscription.toJSON()` 그대로(`{endpoint, keys:{p256dh, auth}}`), https 또는 loopback http 엔드포인트만 |
| `POST /v1/push/presence` | read | `{subscription_id, focused}`. focused 면 60초 동안 그 구독에 푸시하지 않는다 |
| `GET /v1/browser/tabs` | read | 비공개 세션이 아니며 에이전트가 명시적으로 소유한 탭만 나열한다. 반환 항목은 CDP `tab_id`, 인증 정보·쿼리·프래그먼트를 제외한 URL, 제목, 소유권 정보, 소유 세션 ID다. 일반 사용자 탭, 외부 탭, 소유자를 알 수 없는 탭, 비공개 탭은 목록에서 제외된다. 브라우저가 아직 실행되지 않은 상태라면 브라우저를 띄우지 않고 곧바로 200 `{"tabs":[]}`를 반환한다. 브라우저를 띄웠으나 컨트롤러 연결이 끊어진 상태라면 503 `browser_disconnected`를 반환한다. 이 API가 브라우저를 직접 실행하거나 재연결하지는 않는다 |

알림 페이로드는 `{kind:"ticket"|"settled", session_id, ticket_id?, title, body, url}` 이고 `web/public/sw.js` 가 `url`(같은 origin 경로만)을 알림 데이터로 담아 클릭 시 그 창을 열거나 포커스한다. 노출·TLS·서비스 등록은 [`docs/REMOTE.md`](../REMOTE.md).

## 4. PWA
- `web/public/manifest.webmanifest`, 아이콘 `web/public/icons/icon.svg`·`icon-192.png`·`icon-512.png`(any)과 `icon-maskable-192.png`·`icon-maskable-512.png`(maskable).
- `web/public/sw.js` 는 오프라인 셸과 Web Push 를 담당한다. `/v1/*` 는 가로채지도 캐시하지도 않는다. 페이지 이동은 네트워크 우선이고, 오프라인이면 캐시된 셸을 보여 준다. 해시 자산은 캐시 우선이다. `push` 이벤트는 알림을 띄우고 `notificationclick` 은 알림의 deep link 를 연다. 서비스 워커는 프로덕션 빌드에서만 등록한다.

## 5. F-1 체크리스트
| 기능 | API | UI 위치 | 상태 |
| --- | --- | --- | --- |
| 토큰 입력·로그아웃 | `GET /v1/health` | `TokenEntry`, Settings 다이얼로그의 Sign out 버튼 | 구현 |
| 대화 목록 | `GET /v1/sessions` | `Sidebar`(데스크톱 사이드바, 820px 이하는 Conversations 드로어) | 구현 |
| 검색 | `GET /v1/sessions?q=` | 사이드바 검색창 | 구현 |
| 고정 | `PATCH /v1/sessions/{id}` `{pinned}` | 대화 Options 메뉴 Pin/Unpin | 구현 |
| 보관 | `PATCH /v1/sessions/{id}` `{archived}`, `?archived=true` | 대화 Options 메뉴 Archive/Unarchive, Archived 탭 | 구현 |
| 새 대화·제목 변경 | `POST /v1/sessions`, `PATCH /v1/sessions/{id}` `{title}` | `/` 새 대화 화면(첫 메시지로 대화를 만들고 제목은 첫 줄), 대화 Options 메뉴 Rename | 구현 |
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
| 서브에이전트 보기 | `subagent.*` 이벤트, `POST /v1/sessions/{id}/subagents/{sid}/steer`·`/cancel` | `SubagentPanel`: 상태, 진행 이벤트 목록, Steer·Cancel. 승인 카드의 칩에는 서브에이전트 task(미확인 시 id 앞 8자리)를 표시하고, 전체 id는 `data-subagent-id`에 유지. | 구현 (`web/src/components/SubagentPanel.test.tsx`, E2E `web_ui_subagent_view_and_approval_resume_same_call_and_reload_rebuilds_screen`) |
| 입력창 스킬 멘션 칩 | `GET /v1/skills?q=&session=&enabled=true` | `Composer`: `/` 를 치면 후보 목록(↑↓·Enter·Tab), 멘션한 스킬은 칩으로 표시하고 칩에서 지울 수 있다. 멘션 인식과 칩 삭제는 사이드카와 같은 경계 규칙(공백·괄호·구두점)을 쓴다 | 구현 (`web/src/components/Composer.test.tsx`, E2E `web_ui_subagent_view_and_approval_resume_same_call_and_reload_rebuilds_screen`: 실제 데몬 스킬 → 선택기 → 칩 → 전송 → `skill` 로드) |
| visual 미리보기·소스 보기 | `GET /v1/sessions/{id}/visuals` 의 `preview_url` | `Timeline` 의 `VisualFrame`: 빈 `sandbox` iframe(스크립트 없음, opaque origin), Source/Preview 토글. 미리보기 URL 은 9분마다 다시 받는다. 스크립트로 그리는 차트는 실행하지 않는다(정적 HTML/SVG 차트만) | 구현 (`web/src/components/Timeline.test.tsx`, E2E 위와 같음: SVG·HTML·정적 막대 차트를 프레임 밖 스크린샷 픽셀로 확인) |
| 파일 목록·미리보기 | `GET /v1/sessions/{id}/artifacts`, `.../artifacts/{path}`(토큰), `.../preview` | `ArtifactsPanel`: 경로·출처·크기, 선택한 파일을 sandbox iframe 으로 미리보기, 텍스트는 Source(앞 200,000바이트까지, 잘렸으면 전체 크기와 Download 안내), Download | 구현 (`web/src/components/ArtifactsPanel.test.tsx`) |
| 스킬 | `/v1/skills*` | `/skills`: 사용자 스킬 목록 및 로드 실패 폴더와 사유 표시, 스킬 생성, 설명·본문 편집, 활성화·비활성화, 삭제 | 구현 (`web/src/pages/SkillsPage.test.tsx`) |
| 루틴 | `/v1/routines*` | `/routines`: 주기 및 다음 실행 시각 목록 표시, 생성(타임존 지원 크론·간격 반복·단발 실행, 새 대화 또는 기존 대화에서 실행), 일시정지·재개, 즉시 실행, 실행 이력 확인, 삭제, 반복 요청에서 제안된 루틴 바로 생성 | 구현 (`web/src/pages/RoutinesPage.test.tsx`) |
| 메모리 | `/v1/memory*` | `/memory`: 검색 모드 전환(키워드 전용 / 시맨틱), 종류별 필터 검색, 태그 및 보존 기간 지정 생성, 수정, 삭제, 버전 이력 확인, 시크릿 거부 사유 안내 | 구현 (`web/src/pages/MemoryPage.test.tsx`) |
| 계정 및 사용량 | `GET /v1/accounts` | `/accounts`: 자격증명 소스별 마스킹된 계정, 모델, 사용량 주기(백분율 및 리셋 시각), 레이트 리밋 차단 상태 표시, 사용 불가 소스 및 사유 안내 | 구현 (`web/src/pages/AccountsPage.test.tsx`) |
| 볼트 | `GET /v1/vault`, `POST /v1/vault/setup`·`unlock`·`lock`, `GET`·`POST /v1/vault/items`, `PATCH`·`DELETE /v1/vault/items/{id}`, `GET /v1/vault/audit` | `/vault`: 상태 카드(passphrase 또는 키링 설정, 잠금 해제, 잠금), 로그인 또는 인증자 추가(`otpauth://` 링크나 설정 키 입력, 값은 데몬으로만 전송되며 화면에 다시 표시되지 않음), 확인 대화상자를 거치는 삭제, 민감 정보가 제외된 항목 메타데이터, 항목별 사용 정책 설정, 최신순 감사 로그(항목/세션/시간 필터 지원). `admin` 스코프가 없는 기기에는 권한 획득 방법 안내. 모바일 화면 너비 지원 | 구현 완료(`web/src/pages/VaultPage.test.tsx`, E2E `paired_phone_adds_an_authenticator_to_the_vault`, `paired_web_device_manages_devices_vault_audit_and_settings`) |
| Settings 및 Workspace 다이얼로그 | `GET /v1/health` | `/settings`: 현재 화면 위에 Settings 다이얼로그 오버레이(모바일은 바텀 시트): Workspace 진입점, Connection(Daemon 상태 및 API version 표시, 열려 있는 동안 `GET /v1/health`로 5초마다 재확인), Notifications, Appearance, Sign out. `/workspace`는 Skills, Routines, Memory, Accounts, Vault, Devices, Configuration 타일로 구성된 Workspace 다이얼로그를 띄움. 각 섹션은 전용 라우트(`/skills`, `/routines`, `/memory`, `/accounts`, `/vault`, `/devices`, `/config`)를 제공하며, 해당 라우트로 직접 접근하면 새 채팅 위에 다이얼로그를 열고 Close 시 원래 화면으로 복귀. 삭제 및 기기 폐기 시 확인 다이얼로그 표시 | 구현 (`web/src/App.manage.test.tsx`, `web/src/pages/SettingsPage.test.tsx`) |
| 설정 | `GET`·`PUT /v1/settings/config` | `/config`(Workspace → Configuration): 데몬 스키마 기반 동적 폼 생성(전체 키 대상, 페어링된 기기에서는 로컬 전용 키를 읽기 전용으로 표시), 문서 전체를 보여주는 JSON 탭, 재시작 안내 표시 | 구현 (`web/src/pages/ConfigPage.test.tsx`, E2E 위와 같음) |
| 기기 | `GET /v1/devices`, `DELETE /v1/devices/{id}` | `/devices`: 페어링된 기기 목록 및 스코프 표시, 현재 기기 표시, 확인 다이얼로그를 거친 폐기, 새 기기 페어링 안내 | 구현 (`web/src/pages/DevicesPage.test.tsx`, E2E 위와 같음) |
| 브라우저 PiP 미리보기(JPEG)·브라우저 시트 | `GET /v1/sessions/{id}/browser/preview` (WebSocket, `browse` 또는 이를 포함하는 `steer` 스코프 필요. takeover 티켓 엔드포인트 아님), `GET /v1/browser/tabs` (read) | 세션 화면의 `Browser preview` 토글: takeover 티켓 없이도 구석에 읽기 전용 라이브 미리보기 카드를 띄운다(에이전트 활성 탭 추적). 카드 제목 표시줄을 드래그하거나 방향키로 옮길 수 있고, 위치는 이 기기에 기억된다. `Minimize`는 제목 표시줄만 남기고 접는다. 라이브 미리보기 카드의 제목에는 현재 스트리밍 중인 탭의 제목이 표시된다. 탭 ID로 원장 API(`GET /v1/browser/tabs`)를 조회해 가져오며, 페이지가 화면에 보일 때 2초마다 주기적으로 확인하고 현재 세션이 소유한 탭만 반영한다. `Expand` 버튼을 누르거나 이미지를 탭하면 브라우저 시트가 열린다. 시트에서는 라이브 탭 제목과 서버에서 쿼리 및 프래그먼트를 미리 제거한 URL, 해당 URL을 사용자 브라우저에서 직접 여는 `Open in your browser` 링크, 실시간 읽기 전용 이미지, 그리고 스트리밍 중인 탭에 `Live`가 표시되는 `Tabs in this conversation` 목록을 제공한다. 이미지를 탭(`Zoom browser preview`)하면 100~500% 배율로 확대 가능한 전체 화면 읽기 전용 뷰어가 열리며, 이 상태에서도 프레임을 계속 수신한다. 뷰어를 닫으면 시트로, 시트를 닫으면 카드로 돌아간다. 브라우저 컨트롤러 연결이 끊기면(503 `browser_disconnected`) 실시간 이미지는 유지된 채 시트에 연결 해제 상태가 표시된다. API에 관련 엔드포인트가 없으므로 내비게이션, 탭 전환, 탭 닫기 조작 UI는 제공하지 않는다. 사람이 직접 조작(takeover)하는 동안에는 화면을 비우고 일시정지했다가 끝난 뒤 자동으로 재개한다. `secret_input` 값이 입력된 적이 있는 탭(다른 페이지로 이동한 뒤에도 유지)과 비공개 세션은 절대 노출하지 않는다. 세션이 멈추면 열려 있던 미리보기가 닫히며, 직후 다시 시작되더라도 `Browser preview`를 다시 켜야 한다. | 구현 (`web/src/components/BrowserPreview.test.tsx`, `crates/opensided/src/tests/preview.rs`, `crates/openside-e2e/tests/web_pip.rs`) |
| takeover 화면(screencast) | `GET /v1/tickets/{id}/takeover`(WebSocket). 브라우저는 헤더를 못 넣으므로 `Sec-WebSocket-Protocol: openside.v1, openside.bearer.<token>` | `TicketCard` 의 Open remote screen → `TakeoverView`: 프레임 표시, 포인터·휠·키·붙여넣기를 1440x900 좌표로 전달, withheld·input_error·ended 안내, Done/Reject | 구현 (`web/src/components/TakeoverView.test.tsx`, E2E `web_ui_secret_input_and_takeover_screen_resume_the_agent`) |
| 이미지 줌·첨부 업로드 | `POST /v1/sessions/{id}/messages` `attachments[{name,data_base64}]`, `GET .../artifacts/{path}` | `Composer` Attach(썸네일 칩, 첨부만 단독 전송 가능). 데몬과 같은 한도(첨부 4개, 합계 2 MiB, 본문 UTF-8 1 MiB)를 파일을 읽기 전에 검사하여 넘치면 오류를 표시한다. 타임라인 첨부 이미지 클릭 시 `ImageViewer` 실행: 전체 화면, 버튼·`+`/`-`/`0` 키·핀치 조작을 통한 100~500% 확대, 드래그 이동(pan), Esc로 닫기. | 구현 (`web/src/components/{Composer,Timeline}.test.tsx`, E2E 첫 번째 시나리오) |
| 로그 재생만으로 화면 복원 | `GET /v1/sessions/{id}/events` | `reduceSession` 이 서브에이전트·파일·첨부까지 이벤트만으로 만든다 | 구현 (`web/src/hooks/useSessionView.test.tsx`, E2E 새로고침 비교) |
| 페어링(QR) 및 웹 푸시 | 상단 "페어링·푸시 API" 표 참고 | `/pair#<code>`: 1회용 코드를 교환하여 기기 토큰 저장 후 주소창에서 코드를 제거하고 앱 진입(만료·사용 완료·잘못된 코드 오류 안내), `/settings` Notifications: 사용자가 Enable notifications를 눌렀을 때만 권한 요청(페이지 로드 시점 요청 금지), 이후 데몬의 VAPID 키로 서비스 워커를 통해 구독 생성 및 등록. Turn off notifications 클릭 시 구독 해제. 홈 화면에 추가되지 않은 iPhone/iPad 브라우저 탭에서는 버튼 대신 Add to Home Screen 안내 표시. 화면 포커스 변경 시 및 30초 주기로 `POST /v1/push/presence`를 호출해 상태 보고 | 구현 (`web/src/App.manage.test.tsx`, `web/src/pages/SettingsPage.test.tsx`, E2E `paired_web_device_manages_devices_vault_audit_and_settings`). iOS 홈 화면 앱·Android·데스크톱 환경의 실제 푸시 수신은 실기기 및 푸시 서비스가 필요하므로 본 테스트에서는 다루지 않는다. |
| 모든 기능은 피처 플래그나 유료 티어 제한 없이 동작 | — | Workspace 다이얼로그 내 각 섹션 | 구현 (`web/src/App.manage.test.tsx`, `crates/opensided/src/tests/remote.rs::no_route_tool_or_setting_is_behind_a_feature_flag_or_paywall`) |

아티팩트 목록, 시각화 자료, 트랜스크립트 내보내기는 배타적 `after_seq`를 사용하여 모든 이벤트 페이지를 빠짐없이 조회한다. 이를 통해 처음 512건을 넘어선 이벤트까지 모두 포함하며, 트랜스크립트 최종 이벤트 시퀀스가 세션의 마지막 이벤트까지 온전히 이어지도록 보존한다.

## 6. 검증
- `pnpm -C web lint && pnpm -C web typecheck && pnpm -C web test && pnpm -C web build`
- vitest: `web/src/api/{sse,client,uiApiMap}.test.ts`, `web/src/state/session.test.ts`, `web/src/hooks/useSessionView.test.tsx`, `web/src/sw.test.ts`, `web/src/App.test.tsx`, `web/src/App.manage.test.tsx`, `web/src/components/{Sidebar,AppearanceSettings,TicketCard,QueueList,SubagentPanel,Composer,Timeline,ArtifactsPanel,TakeoverView,BrowserPreview}.test.tsx`, `web/src/pages/{NewChat,Skills,Routines,Memory,Accounts,Vault,Devices,Settings,Config}Page.test.tsx`
- 시각 QA 스크린샷: `tools/qa/web-screens.sh [out-dir]` (헤드리스 Chromium으로 모든 웹 화면을 라이트·다크 테마 및 390×844·1440×900 해상도 PNG 파일로 저장한다. 테스트 코드는 `crates/openside-e2e/tests/web_screens.rs`에 있고 `#[ignore]` 처리되어 스크립트로만 실행한다).
- UI-API 맵 재생성: `pnpm -C web gen:ui-map` (`uiApiMap.test.ts`가 최신 상태 여부를 검증한다).
- 데몬: `cargo test -j 8 -p opensided web::`
- E2E: `cargo test -j 8 -p openside-e2e --test web`. 실제 데몬과 스크립트 사이드카를 띄우고, 헤드리스 Chromium 으로 토큰 입력 → 세션 생성 → 메시지 전송 → `ask_user` 카드에서 답 선택 → 세션이 답을 받아 끝나는 것까지 확인한다. `web/dist` 나 Chromium 이 없으면 이유를 출력하고 건너뛴다.
- 세션 화면 E2E: `cargo test -j 8 -p openside-e2e --test web_ui`. 실제 데몬·스크립트 사이드카·헤드리스 Chromium 으로 확인한다.
  - `web_ui_subagent_view_and_approval_resume_same_call_and_reload_rebuilds_screen`: 이미지 첨부와 `/rep` → 선택기에서 고른 스킬 칩 → 전송하면 그 스킬이 모델보다 먼저 로드됨 → 서브에이전트가 목록에 나타나 승인 대기 → 카드에서 Approve → 같은 `child-write` 호출이 실행되고 부모까지 끝남. 답은 말풍선 하나, SVG·HTML·정적 차트 visual 은 빈 sandbox iframe 에서 실제로 그려지고(스크린샷 픽셀) Source 토글은 원문을 보인다. 새로고침 뒤 화면 텍스트와 `lastSeq` 가 같다.
  - `pp22_phone_viewport_approval_resumes_the_same_task`: 390x844 모바일 에뮬레이션에서 승인 카드가 화면 폭 안에 들어오고, Approve 후 같은 `write` 호출이 한 번만 재개된다.
  - `web_ui_secret_input_and_takeover_screen_resume_the_agent`: secret_input 을 UI 로 넣으면 `filled` 로 재개되고 canary 가 이벤트·기록·DOM 에 없다. takeover 화면이 프레임을 받고, UI 의 클릭·타이핑이 탭에 전달되며, Done 후 같은 호출이 재개된다.
- 페어링·푸시: `cargo test -j 8 -p opensided tests::remote`, `cargo test -j 8 -p openside-push`, `cargo test -j 8 -p openside-e2e --test remote --test push_browser`. `push_browser` 는 데몬이 127.0.0.1 가짜 푸시 서비스로 보낸 암호문을 복호화한 바이트를 DevTools `ServiceWorker.deliverPushMessage` 로 실제 `sw.js` 에 넣어, 알림 제목과 deep link 를 확인한다. Chromium 이 없으면 이유를 출력하고 건너뛴다.
- 관리 기능 E2E 테스트: `cargo test -j 8 -p openside-e2e --test web_admin`. 실제 데몬과 임시 볼트를 띄운 뒤 TCP 상의 헤드리스 Chromium이 `POST /v1/pairing`으로 발급된 `/pair#<code>` 링크에 접속하여 로그인한다(사용된 코드는 즉시 소진). 탐색은 사이드바의 Workspace 링크와 Workspace 다이얼로그 타일을 거치며, Devices 화면에서는 현재 기기를 확인하고 Vault에서는 항목 정책 변경 후 생성된 `policy_update` 감사 로그를 조회하며 Configuration 폼에서는 `tickets.confirm_ttl_secs`를 저장(config.toml에 반영)한 다음, JSON 탭에서 `policy.mode`를 수정할 때 로컬 소켓 전용 거부 응답을 받는지 검증한다. `web/dist` 빌드 결과물이나 Chromium이 없으면 사유와 함께 건너뛴다.
