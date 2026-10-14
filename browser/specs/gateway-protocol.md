# 브라우저 ↔ 데몬 게이트웨이 프로토콜 v1

## 1. 상태와 책임

이 문서는 BR-1의 구현 계약이다. 구현은 `browser/src/chrome/browser/openside/agent_gateway.cc`(Linux 전용)와 `crates/openside-browser/src/gateway.rs`에 있다. 실행 검증은 `crates/openside-browser/tests/patched.rs`에 있다. 계약과 다른 점은 §10에 적는다.

- 브라우저: `WebContents` 식별·소유권, 실제 사용자 입력, 포커스, 내부 이벤트의 권위 있는 원본이다.
- `openside-browser`: 소켓 연결·프레이밍·탭 장부 동기화와 기존 CDP 컨트롤러의 연결을 맡는다.
- `openside-proto`: 후속 구현에서 wire 스키마를 생성하는 단일 원본이 된다. D-3에서는 Rust 스키마를 추가하지 않는다.
- `opensided`·`openside-policy`: 세션 권한, 승인, 이벤트 영속화를 맡는다.
- 기존 Stage A CDP 경로는 유지한다. 이 계약을 이미 구현한 경로로 취급하지 않는다.

토큰 인증은 사용자 제스처의 증명이 아니다. 모델이 보낸 `owner`나 사용자 신분 주장은 권한의 근거가 될 수 없다.

## 2. 소켓과 토큰

브라우저가 `AF_UNIX/SOCK_STREAM`으로 수신하고 데몬이 연결한다. 인스턴스당 인증된 제어 연결은 하나다.

When the gateway closes a connection, it releases the single-controller slot before the peer observes EOF. An immediate reconnect after EOF must not be rejected because deferred destruction still holds the old connection.

| 항목 | 계약 |
| --- | --- |
| 소켓 | `<runtime>/openside/browser/<instance_id>/gateway.sock` (`<runtime>`은 `--openside-gateway-runtime-dir` 값이며, 스위치가 없을 때만 `$XDG_RUNTIME_DIR` 사용) |
| 토큰 파일 | `<runtime>/openside/browser/<instance_id>/gateway.token` (`<runtime>`은 위와 같다) |
| 상위 디렉터리 | 심볼릭 링크가 아니며 실행 중인 UID가 소유하고 권한이 정확히 `0700`인 실제 디렉터리여야 한다. 브라우저는 런타임 디렉터리를 `O_DIRECTORY\|O_NOFOLLOW`로 한 번 열고 디스크립터로 검증한 뒤, 그 아래의 모든 항목을 디스크립터 기준으로 생성한다. 런타임 디렉터리의 상위 경로는 신뢰한다. 중간 경로를 바꿔치기하더라도 동일한 검증을 통과하는 디렉터리로만 연결되기 때문이다. |
| 소켓·토큰·discovery(`gateway.json`) 파일 | 실행 UID 소유, `0600` |
| `instance_id` | 인스턴스별 무작위 식별자. 프로필 경로를 넣지 않는다 |
| peer 확인 | 양쪽 모두 `SO_PEERCRED`로 같은 UID인지 확인한다 |

스위치를 지정했더라도 값이 비어 있거나, 절대 경로가 아니거나, 심볼릭 링크이거나, 실행 중인 UID가 소유한 권한 `0700`의 전용 디렉터리가 아니면 게이트웨이는 시작을 거부하며 `XDG_RUNTIME_DIR`로 폴백하지 않는다. 스위치가 없으면 `XDG_RUNTIME_DIR`이 없거나 안전하지 않을 때 게이트웨이 시작을 거부한다. 프로덕션 경로는 `/tmp`나 TCP로 변경되지 않는다. Linux 소켓 경로가 NUL 문자를 제외하고 107바이트를 초과하면 경로를 자르지 않고 시작 단계에서 즉시 거부한다. native-messaging 호스트가 상속받아야 하므로 브라우저 자체의 `XDG_RUNTIME_DIR`은 사용자 설정을 유지한다.

브라우저는 인스턴스마다 CSPRNG 32바이트를 생성한다. 토큰 파일에는 소문자 16진수 64자와 줄바꿈 하나를 쓴다. `umask 077`, 배타적 생성, 일반 파일 조건을 적용한다. 데몬은 symlink를 따라가지 않고 열며 `fstat`으로 파일 종류·UID·정확한 `0600` 권한을 확인한 뒤 읽는다. 다른 파일을 임의로 `chmod`하지 않는다. 비교는 constant-time으로 한다.

Rust client는 인스턴스 디렉터리를 `O_DIRECTORY|O_NOFOLLOW`로 열어 UID·`0700`을 확인한 뒤, 그 descriptor(`/proc/self/fd/<fd>/<name>`)로 discovery·토큰·소켓을 찾는다. 확인한 뒤 경로가 바뀌어도 확인한 디렉터리 밖의 파일을 열지 않는다. 파일은 `O_NOFOLLOW|O_NONBLOCK`으로 열어 FIFO에서 멈추지 않는다.

토큰은 최초 `hello`에만 들어간다. 명령행, URL, 로그, 이벤트, omp 프롬프트·REPL에 넣지 않는다. discovery 메타데이터와 토큰 파일을 분리한다. 정상 종료 시 브라우저가 자기 토큰을 삭제한다. 재시작하면 새 인스턴스·새 토큰을 쓴다. 같은 UID의 악성 프로세스까지 파일 권한으로 막을 수는 없다. D-3 작업에서는 실제 토큰 파일을 읽지 않았다.

## 3. 프레이밍과 handshake

```text
[u32 big-endian payload_byte_length][UTF-8 JSON object]
```

- 길이는 `1..16777216`이다. 할당 전에 검사한다. 줄바꿈으로 프레임을 나누지 않는다.
- 부분 read와 여러 프레임이 합쳐진 read를 처리한다. 프레임 도중 EOF는 `protocol_error`다.
- 잘못된 UTF-8·JSON, 중복 object key, 알 수 없는 `kind`·`type`, 필수 필드 누락·타입 오류를 거부한다.
- 정수 카운터 범위는 `0..9007199254740991`이다. 범위를 넘으면 거부한다.
- 선택한 버전의 알 수 없는 선택 필드는 무시할 수 있다. v1에는 별도 binary stream이 없다.

연결 후 5초 안에 다음 handshake를 끝낸다. 아래 토큰은 예시용 표기다.

```json
{"kind":"hello","protocol":"openside.browser","versions":[1],"instance_id":"instance-id","token":"<redacted>"}
{"kind":"welcome","v":1,"instance_id":"instance-id","capabilities":["tab_lifecycle","focus","hidden_screenshot","popup","dialog","download"]}
```

브라우저는 토큰·peer UID·인스턴스·공통 버전을 확인한다. `welcome`의 capability에는 실제 구현한 기능만 넣는다. 미지원 기능은 `unsupported`다. 성공 응답을 꾸며 내지 않는다. `welcome` 전에는 명령·이벤트를 처리하지 않는다. 실패하면 안전한 범위의 오류만 보내고 연결을 닫는다. 토큰을 오류에 되돌려 보내지 않는다. 인증 없는 CDP로 자동 전환하지 않는다.

## 4. 메시지 envelope

handshake 이후 모든 객체는 `v:1`, `kind`, `type`을 가진다.

| `kind` | 추가 필드 |
| --- | --- |
| `request` | `id`, `session_id`, `subagent_id`, `payload` |
| `result` | 요청의 `id`·`type`, `payload` |
| `error` | `id`, `type`, `code`, `message`, `details` |
| `event` | `event_seq`, `instance_id`, 해당 시 `tab_id`, `session_id`, `subagent_id`, `payload` |

`id`는 비어 있지 않은 opaque string이며 최대 128 UTF-8 바이트다. 같은 연결에서 재사용하지 않는다. 요청의 `session_id`는 유효한 세션이어야 한다. `subagent_id:null`은 세션의 루트 에이전트다. 서브에이전트는 비어 있지 않은 문자열을 쓴다. 루트와 자식, 서로 다른 자식은 별도 소유자다. 데몬은 실행 컨텍스트에서 권한을 확인한다. 종료·미등록 세션 요청은 거부한다.

`payload`는 object다. `error.message`는 페이지 내용·비밀이 없는 고정 문구다. `details`에는 비밀이 아닌 식별자만 넣는다. handshake 오류의 `id`·`type`은 `null`이다. 세션에 귀속되지 않은 사용자·외부 이벤트의 `session_id`·`subagent_id`도 `null`이다.

`event_seq`는 연결마다 1부터 단조 증가한다. 브라우저 UI sequence에서 사건 순서를 정하고 IO sequence에서 같은 순서로 직렬화한다.

## 5. 탭 레코드

| 필드 | 타입·의미 |
| --- | --- |
| `tab_id` | `WebContents` 수명 동안 유지하는 opaque string. 인스턴스 내 재사용 금지 |
| `cdp_target_id` | CDP 식별자 `string|null` |
| `owner` | 현재 제어 주체 `agent|user|external` |
| `session_id` | 생성 세션 `string|null` |
| `subagent_id` | 생성 서브에이전트 `string|null`. `null`은 루트 |
| `label` | 표시용 문자열. 인증 정보가 아니다 |
| `background` | 생성·제어 상태의 bool. 현재 compositor 가시성의 증명이 아니다 |
| `opened_by` | 변경하지 않는 생성 출처 `agent|user|external` |
| `url`, `title` | 문자열. 비신뢰 페이지 데이터다 |
| `state` | `live|asleep|closed` |

탭 strip index를 `tab_id`로 쓰지 않는다. 알 수 없는 탭·복원한 탭은 `external`이다. 사용자 takeover 후에도 생성 세션·서브에이전트·`opened_by`는 남긴다. `owner:user`이면 에이전트 권한은 사라진다. C++의 빈 식별자는 wire의 `null`로 변환한다.

## 6. 요청

| `type` | `payload` | 결과·규칙 |
| --- | --- | --- |
| `tabs.list` | `{}` | `{tabs:[TabRecord],through_event_seq:n}`. UI sequence에서 원자적으로 샘플링한다 |
| `tab.open` | `{url,label,background:true}` | `{tab:TabRecord}`. 인증된 scope로 소유권을 정한다. `tab.opened` 다음에 결과를 보낸다 |
| `tab.close` | `{tab_id}` | `{closed:true}`. 자신의 agent 탭만 닫는다. 실제 종료의 `tab.closed` 다음에 결과를 보낸다 |
| `tab.set_label` | `{tab_id,label}` | 같은 소유 scope만 허용한다. `tab.updated`와 갱신한 `TabRecord`를 보낸다 |
| `focus.request` | `{tab_id,grant_id:null|string}` | `{allowed:bool,reason,owner:user|agent}`. 거절은 정상 판정 결과다 |
| `screenshot.capture` | `{tab_id,format:png}` | `{tab_id,format:png,width:1440,height:900,data_base64}` |
| `dialog.respond` | `{tab_id,dialog_id,accept:bool,prompt_text:string|null}` | 같은 scope와 정책 승인이 필요하다. 사용자 다이얼로그에는 적용하지 않는다 |
| `download.cancel` | `{tab_id,download_id}` | 같은 scope와 정책 승인이 필요하다 |

닫기는 `owner:agent`, `opened_by:agent`, 정확한 세션·서브에이전트 일치, 사용자 제어 상태 아님을 모두 확인한다. `beforeunload`가 막으면 `dialog_pending`과 `dialog.opened`를 보낸다. 닫혔다고 보고하지 않는다. `tab.claim`, wire `focus.grant`, wire `user_gesture` 명령은 없다.

## 7. 이벤트

| `type` | `payload` |
| --- | --- |
| `tab.opened` | `{tab:TabRecord}` |
| `tab.updated` | `{tab:TabRecord,reason:navigation|label|user_takeover}` |
| `tab.asleep` | `{reason:discarded|unloaded}` |
| `tab.closed` | `{reason:agent|user|external|browser_shutdown}` |
| `focus.decided` | `{request_id,allowed,reason,owner}` |
| `focus.changed` | `{owner,session_id,subagent_id,reason:user_gesture|grant_consumed|run_ended|disconnect}` |
| `popup.opened` | `{opener_tab_id,child_tab:TabRecord|null,blocked:bool}` |
| `dialog.opened` | `{dialog_id,dialog_type:alert|confirm|prompt|beforeunload,origin,message_redacted}` |
| `dialog.closed` | `{dialog_id,accepted:bool}` |
| `download.started` | `{download_id,suggested_filename_redacted}` |
| `download.updated` | `{download_id,state:in_progress|complete|cancelled|interrupted,received_bytes,total_bytes:null|integer}` |

페이지가 만든 팝업은 `opened_by:external`, `owner:external`이다. opener의 agent 권한을 상속하지 않는다. native UI를 활성화하지 않는다. 차단한 팝업은 `blocked:true`, `child_tab:null`로 보고한다. 다이얼로그에는 암호·입력값을 싣지 않는다. 다운로드에는 임의 파일 경로를 노출하지 않는다. 완료·취소·중단 상태는 한 번 보고한다.

## 8. 포커스와 숨은 탭 캡처

실제 native UI의 명시적 동작만 일회성 `grant_id`를 발급한다. grant는 인스턴스·탭·세션·서브에이전트·run에 묶이며 5초 후 만료한다. UI sequence에서 검사·소비·활성화를 직렬화한다. 사용자 동작은 grant를 즉시 취소한다. 토큰 보유자가 스스로 grant를 만들 수 없다. 상세 규칙은 [focus-arbiter.md](focus-arbiter.md)를 따른다.

캡처는 같은 허가 scope의 살아 있는 탭에만 허용한다. discarded·unloaded 탭은 거부한다. 뷰포트는 1440x900이다. 숨은 탭은 compositor `CopyFromSurface` 경로를 쓴다. capturer는 RAII로 관리하며 숨김 상태를 유지하고 모든 종료 경로에서 해제한다. 내부 제한 시간은 8초다. 탭 선택·활성화, 창 raise·`Show`를 하지 않는다. surface가 없으면 실패하며 foreground 캡처로 전환하지 않는다.

PNG는 base64 변환 전 최대 8388608바이트다. 응답 전체는 프레임 한도 안에 들어가야 한다. 화면은 민감한 데이터다. 모델 전달·영속화 전에 Vault redaction을 적용한다. 캡처 데이터·비밀을 이벤트 로그에 넣지 않는다.

## 9. 오류·종료·재연결

| 코드 | 의미 |
| --- | --- |
| `auth_failed`, `version_mismatch` | handshake 인증·버전 실패 |
| `protocol_error`, `frame_too_large` | wire 형식·길이 위반. 연결을 닫는다 |
| `unsupported`, `invalid_request` | 기능 미지원·인자 오류 |
| `scope_not_found`, `tab_not_found` | scope·탭 없음 |
| `ownership_denied`, `policy_denied` | 소유권·정책 거부 |
| `focus_denied` | 잘못된 grant 실행. 일반 거절은 `focus.request` 결과다 |
| `tab_asleep`, `surface_unavailable`, `capture_failed` | 캡처 불가 |
| `timeout`, `dialog_pending` | 제한 시간·다이얼로그 대기 |
| `resource_exhausted`, `internal` | 자원 부족·내부 실패 |

요청 타임아웃은 12초다. 캡처 작업은 뷰포트 준비, 서피스 복사, 인코딩을 포함해 8초의 엔드투엔드 기한을 가진다. 이 기한은 강제 중단이 아닌 타이머 작업으로 관리된다. 타이머가 만료되면 캡처는 `timeout`을 반환하고 해당 슬롯은 다시 가용 상태가 된다. 클라이언트의 수신 속도가 브라우저보다 느려 대기 프레임이 16개에 도달하면, 브라우저는 해당 클라이언트의 요청 처리를 중단했다가 대기열이 비워지면 재개한다. 이 일시 중단은 프레임 수만 집계하므로 파이프라인된 소형 응답만 보호한다. 프레임이 대기 중인 상태에서 클라이언트가 5초 동안 아무것도 읽지 않거나, 아웃바운드 큐가 64개 프레임 또는 32MiB를 초과하면 연결이 끊어진다. 이벤트, 이미 실행 중이던 요청의 응답(캡처, 자동완성), 스크린샷과 같은 일부 대형 응답은 이 한도에 도달할 수 있다. 소유권·포커스 이벤트를 조용히 버리지 않는다.

연결 종료 시 미완료 요청은 실패하고 grant를 취소한다. 재연결은 handshake와 `tabs.list`로 복구한다. wire replay·exactly-once는 제공하지 않는다. `tab.open`, 닫기, 다이얼로그 응답을 자동 재전송하지 않는다. snapshot의 `through_event_seq` 뒤 이벤트만 장부에 적용한다.

`openside-browser`는 `opened_by`를 기존 Stage A의 `openedBy:agent|user|external`로 매핑한다. asleep·unloaded 탭에 attach하지 않으며 attach에는 12초 race를 적용한다. `opensided`가 정규화한 `tab.opened|closed|asleep`를 저장한다. 영속 세션의 `seq`·`ts`·`run_id`는 세션 계층에서 붙인다. 재연결마다 초기화되는 `event_seq`를 세션 `seq`로 쓰지 않는다. 귀속되지 않은 사용자·외부 사건을 세션 사건으로 꾸미지 않는다.

## 10. 검증 계약과 설계 출처

검증 항목은 부분·합쳐진 프레임, 잘못된 길이·UTF-8, UID·권한·symlink·토큰·버전 거절, 미지원 capability, 세션·서브에이전트 닫기 경계, 사용자 takeover, 포커스 불변 캡처, 팝업·다이얼로그·다운로드 순서, timeout 해제, 재연결 barrier와 재전송 금지다.

### 현재 구현과의 차이

- `tab.open`·`tab.close`·`dialog.respond`·`download.cancel`은 `unsupported`를 돌려준다. 탭 생성·닫기는 CDP 경로를 쓴다.
- native grant UI가 없다. `focus.request`는 항상 `{allowed:false, reason:"no_user_grant", owner:"user"}`다.
- agent 탭의 활성화 시도(`Target.activateTarget`, `window.focus()`)를 막으면 `focus.decided`를 `request_id:null`로 보낸다.
- `TabRecord`에 `group_title`(탭 그룹 제목 또는 `null`)을 더 넣는다.
- `message_redacted`·`suggested_filename_redacted`는 bool `true`다. 메시지와 파일명은 보내지 않는다.
- `tab.closed`의 `reason`은 `browser_shutdown` 또는 `external`뿐이다. 닫은 주체는 구분하지 않는다.
- `beforeunload`(앱 모달) 다이얼로그와 차단된 팝업은 보고하지 않는다.
- JSON 중복 key는 거절하지 않는다(`base::JSONReader`).
- Rust client는 gateway 모드에서 `Runtime.enable`을 보내지 않는다.
- 브라우저는 debugging endpoint를 열기 전에 discovery를 쓴다. `required=false`(데몬 `browser.gateway = "auto"`)면 endpoint가 준비된 시점에 discovery가 없을 때 기다리지 않고 CDP 전용으로 동작한다. runtime 디렉터리가 없거나 공유돼 있어도 CDP 전용이다. discovery·파일 권한이 잘못됐거나 인증이 실패하면 launch가 실패한다. `required=true`면 discovery가 없을 때도 실패한다. Linux가 아니면 gateway를 쓰지 않는다.
- 받는 프레임은 인증 전 4096바이트, 인증 후 1 MiB까지다(§3의 16 MiB보다 작다). 길이 header만 보고 본문을 받기 전에 `frame_too_large`로 닫는다. 한 번에 256 KiB까지 읽고, 한 task에서 32프레임까지 처리한 뒤 다음 task로 넘긴다. 처리하지 못한 프레임이 남아 있으면 읽기를 멈춘다. 보내는 프레임은 16 MiB까지다.
- 같은 연결에서 `id` 재사용은 최근 1024개 안에서만 거절한다.
- 연결당 동시에 실행할 수 있는 캡처는 최대 2개이며, 이를 초과하면 `resource_exhausted`를 반환한다. 캡처를 시작할 때마다 브라우저는 자체 1440x900 및 1배율 오버라이드를 다시 적용하고 렌더러의 확인 응답을 기다린다. 따라서 다른 DevTools 클라이언트가 남긴 메트릭이 캡처에 반영되지 않는다. 렌더러 메인 스레드가 바쁜 등의 이유로 요청을 거부하거나 3초 안에 응답하지 않으면, 준비되지 않은 페이지를 캡처하는 대신 `surface_unavailable` 오류로 실패 처리한다. 브라우저는 서피스를 복사하기 직전과 응답을 보내기 직전에 탭 소유권을 다시 검사한다. 그사이 탭이 스코프를 벗어났다면 `ownership_denied`를 반환하고 생성한 이미지는 폐기한다. 뷰 크기가 1440x900 DIP가 아닐 때도 `surface_unavailable`로 실패하며, 결과물은 1440x900 크기로 요청한다. PNG 인코딩, base64 변환, 응답 JSON 직렬화는 UI 스레드 밖에서 처리한다. 창 관리자가 창에 어떤 크기를 부여하든 페이지는 에뮬레이션되므로 헤디드 환경에서도 캡처가 정상 작동한다.
- `event_seq`가 2^53-1에 닿으면 `resource_exhausted`로 연결을 닫는다. `tab.asleep` 이벤트와 재연결 barrier는 아직 없다. `Browser::connect`로 다시 붙은 데몬은 gateway 없이 CDP만 쓴다.
- Rust client는 요청마다 launch scope의 `session_id`·`subagent_id`를 그대로 보낸다. 요청 frame은 writer task 하나가 frame 단위로 쓰고, 한 deadline이 대기열·송신·응답을 모두 덮는다. 취소된 요청은 pending에 남지 않는다. 쓰기가 제한 시간 안에 끝나지 않으면 연결을 버린다.
- Rust client는 envelope를 검증한다: `v == 1`, 이벤트는 `event_seq`가 정확히 1씩 늘고 `instance_id`가 맞으며 `type` 문자열·`payload` object, tab·session·subagent 필드는 문자열 또는 `null`. 결과는 문자열 `id`·`type`과 object `payload`. 어긋나거나 `id` 없는 연결 오류가 오면 연결을 닫고 미완료 요청은 `Disconnected`로 끝난다.
- Rust client의 이벤트 버퍼는 256개다. 소비자가 뒤처지면 오래된 이벤트부터 버리고 다음 `next_event`가 `Lagged{missed}`를 돌려준다. 요청·결과는 이벤트 소비와 무관하다. 장부는 `tabs.list`의 `through_event_seq`로 다시 맞춘다.
- 데몬은 브라우저 하나를 모든 세션이 같이 쓰므로 고정 scope `session_id = "opensided"`로 띄운다. 브라우저 쪽 `owner:agent`는 "데몬이 연 탭"을 뜻하고, 세션별 소유권은 CDP 장부(`owner_session_id`)가 정한다.

xplorer 기준 revision은 `64b3ef1c40e899aa9b9d14657514a5a47838f250`이다. 다음 파일을 직접 읽고 설계만 참고했다. 코드는 복사하지 않았다.

- [`src/chrome/browser/agent_gateway/agent_gateway.cc`](https://github.com/daniel-farina/xplorer/blob/64b3ef1c40e899aa9b9d14657514a5a47838f250/src/chrome/browser/agent_gateway/agent_gateway.cc): 토큰·서버·인증, IO→UI, focus·tab route.
- [`browser_api.cc`](https://github.com/daniel-farina/xplorer/blob/64b3ef1c40e899aa9b9d14657514a5a47838f250/src/chrome/browser/agent_gateway/browser_api.cc): 닫기 처리.
- [`tab_ownership.h`](https://github.com/daniel-farina/xplorer/blob/64b3ef1c40e899aa9b9d14657514a5a47838f250/src/chrome/browser/agent_gateway/tab_ownership.h)·[`tab_ownership.cc`](https://github.com/daniel-farina/xplorer/blob/64b3ef1c40e899aa9b9d14657514a5a47838f250/src/chrome/browser/agent_gateway/tab_ownership.cc): `WebContents` 수명 메타데이터.
- [`focus_arbiter.h`](https://github.com/daniel-farina/xplorer/blob/64b3ef1c40e899aa9b9d14657514a5a47838f250/src/chrome/browser/agent_gateway/focus_arbiter.h)·[`focus_arbiter.cc`](https://github.com/daniel-farina/xplorer/blob/64b3ef1c40e899aa9b9d14657514a5a47838f250/src/chrome/browser/agent_gateway/focus_arbiter.cc): 기본 거부·사용자 포커스 복귀.
- [`tab_screenshot.cc`](https://github.com/daniel-farina/xplorer/blob/64b3ef1c40e899aa9b9d14657514a5a47838f250/src/chrome/browser/agent_gateway/tab_screenshot.cc): native view와 숨은 탭 compositor 분리.

우리는 HTTP 대신 unix socket, strip index 대신 안정 ID, 누락한 `X-Agent-Id` 대신 명시적 scope를 쓴다. 기존 탭 claim을 제공하지 않고 닫기 권한을 엄격하게 제한한다.

라이선스는 `unverified`다. 이 revision에는 루트 `LICENSE`가 없다. 파일 헤더는 BSD-style이라고 적고 [`README.md`의 License 절](https://github.com/daniel-farina/xplorer/blob/64b3ef1c40e899aa9b9d14657514a5a47838f250/README.md#license)은 오버레이 조건을 owner-defined로 설명한다. 브리프의 BSD-3 표기를 확정된 코드 이용 허가로 취급하지 않는다. [NOTICE.md](../NOTICE.md)에 차이를 기록한다.

## 11. Authenticated native status and autofill

The Linux gateway advertises `browser_status` and `native_autofill` in addition
to the six original capabilities. The normal v1 authentication and exact launch
scope apply to every method below.
The daemon passes its configured private runtime directory to the browser as
`--openside-gateway-runtime-dir`; the browser's own environment keeps the
user's `XDG_RUNTIME_DIR`, which native-messaging hosts inherit.

| Method | Request payload | Result payload |
| --- | --- | --- |
| `browser.status` | `{}` | `{automation_infobar_count, ux:{vertical_tabs,split_view,bookmarks_side_panel}, ui:{ask:bool,routines:bool}}` |
| `autofill.fields` | `{tab_id,backend_node_id:positive integer}` | `{fields:[{handle,name,origin,backend_node_id,kind}]}` |
| `autofill.fill` | `{tab_id,handle,origin,value}` | `{filled:bool}` |

`browser.status` counts actual native automation infobars on gateway tabs.
Its UX values read the profile's vertical-tabs preference, the real
`SplitViewHorizontal` FeatureList state, and the registered native bookmarks
side-panel entry. UI booleans compare actual loaded TemplateURLService entries
to the exact credential-free integration templates in the managed profile.
Stored template URLs, including pre-existing user keywords, are never returned;
unrelated profiles and incognito do not contribute. An existing vertical-tabs
profile without an explicit value follows the openside build's new default;
explicit user values are preserved. No page content or credential is returned.

Autofill accepts only agent-owned tabs of the exact authenticated session and
subagent launch scope. The daemon's gateway scope is shared `opensided`;
per-session tab permission remains in its `resolve_ref` ownership check.
`autofill.fields` requires the exact non-secret backend node identity and uses
native renderer extraction, not lifted AutofillManager cache provenance. The
receiving main-frame agent verifies that the resolved node's actual document is
its own document and returns its actual DocumentToken and security origin.
No field values are returned. A successful response invalidates previous handles.
Each handle expires after 60 seconds, is consumed once, and binds to its tab,
WeakDocumentPtr, renderer DocumentToken, origin, primary-page generation and
native field ID. Every PrimaryPageChanged advances that generation; navigating
away and restoring a still-live BFCache document cannot revive a handle.
`backend_node_id` is the Blink DOMNodeId underlying FieldRendererId; the daemon
maps its snapshot reference to that exact node rather than an ambiguous label.
At most 256 handles exist per connection.

Before fill, the browser rechecks live main-frame identity, tab ownership, and
exact committed origin against the Vault-authorized origin supplied by the
daemon. Values over 65536 UTF-8 bytes are refused. The daemon's existing browser
operation lock and takeover gate run before Vault calls; native takeover
observation is not added by this patch. The value travels over the authenticated
socket and PasswordAutofillAgent Mojo (`FillFieldForOpenside`), never CDP
Runtime/Input. Immediately at renderer delivery, the receiving agent checks
main-frame identity, the actual node document, DocumentToken and expected origin
again, and refuses hidden/pagehide documents. This closes the asynchronous
document-swap window and rejects same-renderer foreign-frame IDs rather than
trusting browser-lifted metadata. Results/errors/events never echo the value.

The interpretation of P10 is **no CDP or agent/page-executed script carrying the
value into the form**, not a claim that a filled form is invisible to its own
renderer. A page can read its filled fields and submit them; a separate privileged
CDP client could inspect DOM or network traffic after fill. The normal gateway
controller does not enable Runtime or Network events. The daemon continues to
mark/redact secret-bearing documents. Stock Chromium keeps the existing CDP
fallback; a connected gateway that refuses native fill fails closed rather than
silently sending its value through CDP.

