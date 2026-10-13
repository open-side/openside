# 위협 모델

openside 는 한 사용자의 로컬 데몬이다. 아래 표는 막는 위협과 막는 위치, 검증 테스트를 정리한다. 막지 않는 위협은 마지막 절에 따로 적는다.

## 신뢰 경계

1. 모델 출력: 신뢰하지 않는다. 모든 툴 호출은 데몬의 하네스 훅(`host.rs`)을 거친다.
2. 웹 페이지 내용: 신뢰하지 않는다. 프롬프트 인젝션이 들어 있다고 가정한다.
3. 클라이언트: 토큰 스코프만큼만 믿는다. Unix 소켓 호출자는 같은 사용자로 본다.
4. Python 사이드카: 툴은 데몬의 Host 훅을 거쳐서만 실행하고, 비밀은 받지 않는다. 다만 사이드카 프로세스 자체는 샌드박스에서 돌지 않는 한 신뢰하는 코드다. 외부 전송 검사는 그 프로세스 안에서 도는 장치이고 커널 수준 격리가 아니다.

## 위협과 대응

| 위협 | 대응 | 검증 |
|---|---|---|
| 페이지가 모델에게 결제·전송을 지시 | 키워드로 분류한 결제·구매·삭제·전송·게시 행동(`sensitive`)은 항상 승인이 필요하다. 승인 전에는 실행하지 않는다. 주입 문구는 에이전트가 스냅샷으로 읽어 대화에 들어간 뒤에도 같은 결과다. 승인한 호출은 성공한다(대조군) | `openside-e2e/tests/redteam.rs` |
| 페이지가 데이터를 다른 origin 으로 빼내도록 지시 | `allow_origins`/`deny_origins` 를 도구 호출 단계와 브라우저 컨트롤러에서 강제한다. 컨트롤러는 에이전트 탭의 모든 최상위 문서 요청(리다이렉트 매 홉, 링크·폼, 팝업)을 요청이 나가기 전에 판단하고, 소유자를 확인하지 못한 요청은 거부한다. `Ask` 승인은 그 호출이 실행되는 동안에만 유효하다 | `redteam.rs`, `opensided/src/tests/policy.rs::denied_origin_is_unreachable_through_any_agent_navigation`, `::approved_ask_navigation_does_not_outlive_the_approved_call` |
| 모델이 승인 없이 정책을 우회 | 판단은 프롬프트가 아니라 데몬 훅에서 한다. `Deny` 는 승인으로도 풀리지 않는다 | `opensided/src/tests.rs::policy_deny_cannot_be_approved` |
| 비밀 입력 티켓을 연 뒤 대상 origin 이 거부됨 | 값을 채우는 순간 현재 정책으로 다시 판단한다. `Deny` 면 403 `policy_denied`, 필드는 그대로다 | `tests/policy.rs::secret_release_rechecks_live_policy` |
| 정책 변경이 동시에 들어오거나 요청이 끊김 | 저장과 반영을 한 단위로 하나씩 처리한다. 실행 중인 정책, 디스크, 재시작 뒤 정책이 같다 | `tests/policy.rs::concurrent_policy_puts_leave_live_and_disk_identical`, `::aborted_policy_put_still_commits_live_and_disk_together` |
| 워크스페이스 밖 파일 접근(`..`, 심볼릭 링크, 절대 경로) | `guard` 는 승인을 묻는다. 끊긴 링크에 쓰기는 거부한다. `bash` 는 bubblewrap 안에서 실행한다 | `opensided/src/tests/policy.rs::sandbox_escape_suite_never_touches_outside_paths` |
| 스코프가 좁은 토큰으로 권한 상승 | 라우트마다 스코프를 검사한다. 토큰 발급은 `admin` 이고 Unix 소켓으로만 된다. TCP 는 `admin` 토큰도 403 `local_socket_required` 다 | `tests/policy.rs::every_route_rejects_tokens_lacking_its_scope`, `::token_minting_is_socket_only`, `openside-e2e/tests/scopes.rs` |
| 샌드박스 안 클라이언트의 소켓 접근 | 런타임 디렉터리를 샌드박스에 넣지 않으면 소켓에 닿지 않는다. TCP 는 토큰이 필요하다. 검증은 사용자 네임스페이스 안의 클라이언트로 한다(아래 한계 참고) | `scopes.rs` |
| 기본 설치의 외부 통신 | 데몬에는 외부 클라이언트가 없다. 사이드카의 HTTP 전송, 채널 어댑터, 모델 다운로드는 `network.allow` 와 설정한 엔드포인트 밖으로 나가는 요청(리다이렉트 포함)을 거부한다. Chromium 백그라운드 통신은 끄고, 끌 수 없는 GCM 및 컴포넌트 업데이트 요청은 로컬 포트로 돌린다 | `openside-e2e/tests/egress.rs`, `agent/tests/providers/test_egress.py` |
| 텔레메트리 | 전송 코드를 두지 않는다. 알려진 SDK 이름 목록으로 의존성을 검사하고, 네트워크 관찰로 허용 밖 전송이 없음을 확인한다 | `egress.rs::telemetry_has_no_sender_in_rust_or_python_dependencies`, `egress.rs::default_install_contacts_only_loopback_and_allow_listed_peers` |
| 정책 파일 손상 | 시작을 거부한다(fail closed) | `tests/policy.rs::policy_api_validates_persists_and_hot_reloads` |

## 전송 방식별 보안 경계 및 위협 모델

### 릴레이 전송 (자체 호스팅 릴레이)
- **등록 프로토콜 및 식별자 소유권**: 데몬 등록 시 `openside-relay-registration/1\n{id}` 형태의 도메인 분리 챌린지를 적용하여 Noise NK 핸드셰이크를 수행한다. 공개키 해시와 라우팅 ID가 일치해야만 등록이 승인되며, 서버가 재시작되더라도 개인키 증명을 통해 ID 소유권을 유지한다. 프로세스가 살아 있는 동안 각 등록 토큰은 최초 인증된 데몬 ID에 바인딩되어 다른 토큰이나 제3자가 활성 ID를 가로챌 수 없다. 따라서 데몬마다 별도 토큰을 발급해야 한다.
- **리소스 제한(DoS 방어)**: 릴레이 서버는 전역적으로 최대 256개의 데몬 소켓과 1024개의 클라이언트 소켓까지만 수용하며, 데몬당 최대 32대의 클라이언트와 클라이언트당 8MiB의 다중화 버퍼 한도를 적용해 자원 고갈을 방지한다.
- **암호화 범위 및 메타데이터**: 핸드셰이크 완료 후 모든 HTTP 요청 본문, 헤더, Bearer 인증 토큰, SSE 스트림, WebSocket 프레임은 AEAD로 암호화된다. 요청 메타데이터 형식이 올바르지 않으면 연결을 즉시 닫고 거부하며(fail closed), 인증 정보를 평문이나 직접 연결로 폴백하지 않는다. 단, Noise 핸드셰이크 단계의 임시 공개키와 식별자는 암호화되지 않으며, 릴레이 서버는 라우팅 메타데이터(클라이언트 IP, 접속 시간, 패킷 크기, 데몬 ID)를 열람할 수 있다. 패킷 재전송이나 순서 조작은 Noise의 엄격한 카운터 및 AEAD 검증으로 차단된다.
- **웹 UI 코드 배포 위험**: 릴레이 서버가 제공하는 웹 페이지의 JavaScript 코드가 변조된 경우, 페어링 과정에서 URL 프래그먼트에 담긴 `#c=<페어링코드>&k=<데몬공개키>` 값을 읽어 들일 수 있다. 이 코드는 3분의 유효 시간 동안 1회만 교환 가능하지만, 악의적인 스크립트가 사용자가 정상 교환하기 전에 이를 가로채 로컬 CLI가 허용한 스코프 범위 내에서 공격자 소유의 영구 기기 토큰으로 등록할 위험이 있다. URL 프래그먼트는 일반적인 HTTP 요청이나 프록시 로그에는 남지 않지만 해당 페이지에서 실행되는 스크립트에는 그대로 노출된다. 독립 바이너리로 설치되어 페어링 시 고정된 Noise NK 공개키를 자체 검증하는 CLI나 MCP 클라이언트와 달리, 릴레이 PWA는 브라우저가 원격 코드를 내려받아 실행하는 구조다. 따라서 신뢰할 수 있는 자체 호스팅 릴레이를 직접 운영하거나, `openside pair --via tailscale`을 이용해 데몬이나 신뢰할 수 있는 Tailscale 오리진의 PWA를 직접 사용하는 방식을 권장한다. 현재 생성되는 릴레이 페어링 링크는 릴레이 오리진으로 바로 이동하며, 웹 앱 셸과 서비스 워커 업데이트가 네트워크 우선(network-first)으로 동작하므로 PWA를 한 번 설치했다고 해서 실행 코드의 불변성이 보장되지는 않는다(코드 해시 고정이나 사용자 승인 기반 업데이트 체계는 구현되어 있지 않다). 다만 데몬의 정적 개인키(`relay-key`)는 브라우저나 릴레이 서버로 절대 전송되지 않으므로, 이러한 웹 페이지 스크립트 및 URL 프래그먼트 탈취 경로를 통해 데몬 개인키가 직접 노출되지는 않는다.
- **인증 우회 방지**: 페어링되지 않은 연결이나 인증 토큰이 없는 릴레이 웹 요청은 데몬 스코프를 우회할 수 없으며, 모든 API 요청은 기기 토큰 검증을 통과해야 한다.

독립적으로 설치된 CLI와 MCP는 페어링 QR 코드나 링크에 고정(pinning)된 데몬 공개키와 Noise 프로토콜 종단간 암호화(E2EE)로 릴레이 연결을 보호한다. 반면 릴레이가 제공하는 PWA는 릴레이 운영자가 서빙하는 HTML과 JS 코드를 신뢰해야 한다. 따라서 자체 호스팅 릴레이를 운영하거나, 릴레이를 거치지 않고 데몬 또는 Tailscale 오리진에 직접 연결하여 PWA를 불러오는 구성을 권장한다.

사람의 판단과 승인 권한은 프롬프트 지침이 아니라 데몬이 직접 강제하는 규칙이다. 기기와 토큰의 역할(`kind`)은 코드를 교환할 때가 아니라 운영자가 오퍼나 토큰을 발급할 때 결정된다. 데몬 호스트에서 일반 `openside pair`를 실행하면 `person_device` 오퍼가 생성되고(웹 UI 선택지 'Pair a phone/browser'), `openside pair --agent`를 실행하면 `agent_client` 오퍼가 생성된다(웹 UI 선택지 'Connect an agent / CLI'). `openside token create`는 기본적으로 `agent_client` 토큰을 발급하며, 로컬 소켓 전용인 `openside token create --person`을 실행하면 확장 프로그램·웹·모바일 기기용 `person_device` 토큰을 명시적으로 발급한다. `openside connect`는 오퍼에 지정된 역할을 그대로 물려받을 뿐 변경할 수 없으므로, `person_device` 오퍼를 교환하면 그대로 `person_device`가 된다. 로컬 Unix 소켓 자체는 식별자가 `local`이며 `agent_client`로 분류된다. `person_device` 토큰을 에이전트에 넘기면 사람의 승인 권한까지 부여되므로 에이전트에는 절대 `person_device` 토큰을 주어서는 안 된다. 로컬 소켓에 접근할 수 있는 동일 UID 프로세스는 의도적으로 `person_device` 토큰을 발급할 수 있으므로, 이 구분은 OS 샌드박스 경계가 아니라 신뢰받는 운영자의 제어 수단이다.

사람의 판단이나 승인이 필요한 작업은 기존 스코프에 더해 반드시 `person_device` 역할이 있어야만 수행할 수 있다. 여기에는 확인(confirm) 티켓 승인(`true` 또는 `"approve"`), 모든 선택지(`choice`) 티켓 응답, `secret_input`의 `{totp}` 영구 등록, `login`의 `save: true` 저장이 포함된다. `agent_client`나 로컬 소켓(`local`)이 이러한 작업을 시도하면 해당 티켓을 직접 생성하거나 세션을 구동한 주체가 아니더라도 `403 person_approval_required` 오류로 거부된다. 거부된 티켓은 승인 대기(pending) 상태를 그대로 유지하며 페이지 조작이나 Vault 등록·저장도 일절 실행되지 않는다. 반면 확인 티켓 거절(`false` 또는 `"reject"`)과 거절·취소 엔드포인트 호출은 필요한 스코프만 있으면 에이전트에서도 허용된다.

### Cloudflare Tunnel 및 Tailscale Serve
- **제공업체 신뢰 수준**: Tailscale Serve는 로컬 노드에서 직접 TLS를 종단하므로 Tailscale 인프라에 평문이 노출되지 않는다. 반면 Cloudflare Tunnel은 Cloudflare 인프라에서 HTTPS가 종단되므로 평문 프록시 트래픽에 대해 제공업체를 신뢰해야 하며, 제공업체를 상대로 한 E2EE 암호화는 성립하지 않는다.
- **자격 증명 격리**: Cloudflare 자격 증명이나 터널 토큰은 클라이언트 API 엔드포인트에 노출되지 않으며 오직 데몬 내부에서만 사용된다.
- **프로세스 격리 한계**: 토큰 파일은 `0600` 권한으로 관리하고 토큰 환경 변수는 자식 프로세스 인자(argv)에 들어가지 않도록 보호하지만, 동일 시스템 계정 내의 다른 프로세스가 파일이나 환경 변수를 직접 읽는 공격은 보호 범위에 포함되지 않는다.

### 프리뷰 샌드박스
HTML, SVG, 텍스트 프리뷰는 릴레이 연결 시 터널을 통해 바이트를 가져와 엄격한 메타 CSP가 주입된 HTML로 감싼 뒤 불투명한 빈 샌드박스 iframe(`sandbox=""`)에 Blob URL 형태로 격리 렌더링하며, 프리뷰 스크립트 실행과 자동 탐색 및 외부 네트워크 요청을 차단한다. 바이너리 PDF는 원본 바이트와 MIME 타입을 보존한 채 빈 iframe 샌드박스와 셸에서 상속된 CSP 정책의 적용을 받으며, 브라우저의 네이티브 PDF 렌더링 동작 자체는 별도로 검증하지 않았다. 데몬 직접 접속 시 프리뷰는 데몬의 독립된 HTTP 응답 CSP를 직접 적용받지만, 릴레이 연결 시의 Blob 프리뷰는 셸의 스타일 제약을 그대로 상속받는다. 이 때문에 릴레이 서빙 셸에 한해 차트 및 시각 요소를 위한 인라인 CSS(`style-src 'self' 'unsafe-inline'`)를 허용한다. 셸 페이지의 스크립트 권한은 어떤 환경에서도 `script-src 'self'`만 유지되어 인라인 스크립트가 차단되며, `frame-src 'self' blob:`은 iframe 출처 제어에만 국한된다. PWA를 서빙하는 릴레이 운영자는 여전히 신뢰할 수 있는 배포자여야 하며, Safari 및 iOS 환경의 렌더링은 별도로 검증되지 않았다.

## 막지 않는 것

- 같은 사용자 권한의 다른 프로세스. 소켓·데이터 디렉터리 권한(0700/0600)으로만 분리한다. 사용자 네임스페이스 안에서 uid 를 바꾼 프로세스도 커널에서는 같은 uid 다. 그래서 `scopes.rs` 는 "네임스페이스 클라이언트" 검증이지 OS 권한 분리 검증이 아니다. 다른 커널 uid 로 실행하는 검증에는 root 나 별도 계정이 필요해서 자동 테스트에 넣지 않았다.
- 샌드박스에서 돌지 않는 사이드카. 사이드카나 그 의존성이 직접 여는 소켓은 외부 전송 검사로 막지 못한다. 알려진 텔레메트리 SDK 가 의존성에 없다는 것과 네트워크 관찰에서 나간 패킷이 없다는 것까지가 보장이다.
- 페이지 자체의 스크립트가 보내는 요청과 Chromium 의 페이지 로드. 정책은 에이전트의 행동만 판단한다. 사용자가 그 사이트를 연 것과 같다. 페이지 로드는 `network.allow` 를 거치지 않고 origin 목록으로만 제한한다.
- 키워드에 걸리지 않는 위험한 버튼. 민감 행동은 이름·역할 키워드로 분류하므로, "Continue" 같은 이름의 결제 버튼은 `sensitive` 로 보이지 않는다.
- 이미 실행 중인 호출. 정책 변경은 다음 툴 호출부터 적용된다. 실행 중인 REPL 스니펫은 시작할 때의 정책으로 끝까지 돈다. 브라우저 이동 검사만 요청마다 현재 정책을 읽는다.
- 같은 데몬의 다른 세션. `browse` 토큰은 세션이 아니라 데몬 전체에 대한 권한이다.
- 사용자가 승인한 민감 행동의 결과.
- LLM 제공자에게 보내는 대화 내용. 제공자 선택은 사용자 몫이다.

## 공유 Card 경계

모델이 전달한 `present` 데이터는 티켓 권한, Hire 액션, 보안 입력폼, 실행 가능한 HTML을 만들 수 없다. 지시문이나 마크업이 포함되어 있더라도 텍스트는 실행되지 않는 데이터로 취급된다. 진입 단계에서 미확인 블록, 위조된 티켓 참조, 아티팩트 경로로 위장한 원격 URL을 거부한다. 미디어 가져오기는 계속 데몬이 직접 관리하며 정책 검사와 정규화를 거친다. 이미지를 가져올 수 없거나 아이콘 캐시가 비어 있어도 원격 브라우저로 넘어가지 않고 텍스트나 이니셜을 대신 표시한다.

보안 Card는 캡처된 실제 페이지와 새로 발급된 신뢰할 수 있는 티켓을 바탕으로 생성된다. 출처와 문서·프레임 검증, 만료 시간, 직렬화된 전환 게이트를 통해 페이지 이동과 여러 기기의 동시 접근을 방지한다. 결제 양식을 채우더라도 제출 권한까지 부여되지는 않는다. 이후 진행되는 Confirm 단계는 캡처된 실제 대상 작업에 독립적으로 바인딩되며, 승인 영수증 자체만으로는 작업의 성공을 증명하지 못한다.

시크릿 카나리는 events.db, 대화 기록(transcript), 도구 인자, 감사 및 로그 파일, 사이드카 스냅샷, 정착된 웹 상태에 절대 남아 있어서는 안 된다. 카나리는 비공개 픽스처 페이지와 사람과 데몬 사이의 일시적인 입력 및 요청 본문에만 머물러야 하며, 영구 저장소에 기록되어서는 안 된다. 인계(handoff) 포인터는 탐색 정보와 상태만 담고 있으며 다른 스레드의 보안 입력폼을 복제하지 않는다.

## Connections 위협 T1–T6

인증 값은 Chromium 또는 Vault에만 보관한다. 모든 연결 관리 경로는 Admin 권한을 요구하며, 메타데이터 라벨만으로는 절대 접근 권한을 부여하지 않는다.

| ID | 위협 | 완화 방안 | 회귀 검증 |
|---|---|---|---|
| T1 | 악성 페이지가 에이전트에게 다른 사이트의 로그인 정보를 사용하도록 지시함 | 페이지의 식별 레이블이나 chief의 권한이 아니라 호출 에이전트의 연결 승인 여부로 사용을 제한한다. teammate는 기본적으로 확인을 거친다. 사람 전용 Confirm 단계에서 teammate, 대상 사이트, 요청 출처를 식별하며 항상 해당 에이전트의 승인 내역만 영구 저장한다. 단순 탐색과 중요한 쓰기 작업의 승인은 별도 게이트로 분리해 유지한다. | `crates/opensided/src/tests/connection_tools.rs::connection_tools_grant_matrix_once_always_and_requesting_origin`; `crates/openside-e2e/tests/connections.rs::login_teammate_always_fetch_cli_parity_and_revoke_clear_browser_sign_in` |
| T2 | 요청이나 리다이렉트로 인해 브라우저 인증 정보가 다른 오리진으로 전송됨 | 해당 연결의 정확한 프로필 및 계정 공간에 속한 전용 탭이 있어야 한다. 최상위 프레임 사이트는 연결과 일치해야 하고, 요청 대상은 탭의 정확한 출처와 일치해야 한다. 브라우저가 관리하는 자격 증명은 해당 출처 안에 머물며, 리다이렉트나 교차 출처 요청은 `connection_cross_site`와 함께 닫힌 채로 실패한다. | `crates/opensided/src/tests/connection_tools.rs::connection_tools_headless_fetch_approval_canaries_cross_site_and_body_cap` |
| T3 | 응답 본문이나 재사용된 헤더를 통해 인증 자료가 유출됨 | 응답 텍스트는 신뢰할 수 없는 페이지 데이터로 취급해 스트림 크기를 64 KiB로 제한하고, 모델에 전달하기 전에 세션 마스킹 처리를 적용한다. 쿠키가 아닌 재사용 헤더는 용량이 제한된 데몬 전용 메모리 소거 캐시에 보관했다가 마스킹 처리가 등록된 후 브라우저로 직접 전달한다. 요청을 보내기 전에 탭을 오염(taint) 처리해 미리보기와 스크린샷 노출을 차단한다. 감사 기록에는 메서드와 경로만 남기며 쿼리 값, 본문, 헤더 값은 절대 저장하지 않는다. | `crates/opensided/src/tests/connection_tools.rs::connection_tools_headless_fetch_approval_canaries_cross_site_and_body_cap` |
| T4 | 가짜 계정 라벨로 특권 연결을 가장함 | 쓰기 경계에서 원시 식별자를 마스킹하고 계정 HMAC은 외부에 공개하지 않는다. 프로브나 에이전트의 식별 텍스트는 설명을 위한 정보일 뿐 권한 부여 근거가 될 수 없다. 인가는 불투명한 연결 ID와 안정적인 호출 에이전트 ID에 바인딩된다. | `crates/opensided/src/tests/connections.rs::connections_migration_identity_and_canary_storage`; `crates/opensided/src/tests/connection_tools.rs::connection_tools_grant_matrix_once_always_and_requesting_origin` |
| T5 | 진행 중인 작업과 로그아웃 시점이 경합하거나, 닫힌 탭의 사이트 데이터가 권한 철회 후에도 남음 | 브라우저와 연결의 변경 작업을 직렬화하고, 사이트 탭을 닫거나 빈 페이지로 만든 뒤 신뢰할 수 있는 호스트 CDP를 통해 사이트 쿠키와 알려진 모든 사이트 출처의 스토리지를 정리한다. 이후 메타데이터와 승인 내역을 삭제하고 감사 로그를 추가한다. 삭제 후 연결 도구를 호출하면 `connection_not_found`를 반환한다. 공유 프로필에서 로그아웃하면 해당 사이트의 모든 계정에 영향을 주며, 별도 공간은 프로세스를 종료하고 프로필 전체를 삭제한다. 삭제만 수행할 때는 공유 인증 상태는 유지하되 승인 권한만 제거한다. | `crates/opensided/src/tests/connections.rs::connections_revoke_clears_closed_cookie_less_subdomain_frame_storage`; `crates/openside-e2e/tests/connections.rs::login_teammate_always_fetch_cli_parity_and_revoke_clear_browser_sign_in` |
| T6 | REPL의 Network.getCookies 호출로 인증 값이 노출됨 | 명시적인 프레임 URL을 포함하여 허용된 모든 출처에서 반환된 쿠키 값을 마스킹하며, 쿠키 이름과 플래그는 정상적으로 사용할 수 있도록 유지한다. 쿠키 저장소 변경과 광범위한 스토리지 조회 권한은 호스트에만 남겨 둔다. 도구, 이벤트, 로그에는 어떤 쿠키 값도 반환하지 않는다. | `crates/opensided/src/tests/repl.rs::connections_repl_cookie_values_are_redacted_in_results_events_and_logs` |

이 경계가 인증된 사이트 콘텐츠의 민감도를 없애 주거나, 상호 신뢰하는 에이전트 간에 공유된 쿠키 저장소를 격리하거나, 동일한 OS 사용자로 실행 중인 다른 프로세스로부터 Chromium의 비공개 프로필을 보호하지는 않는다. 중대한 작업은 여전히 정책과 사람의 승인에 따라 통제된다.

