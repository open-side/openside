# Human step (deferred approval) — 인증·보안 벽 처리 설계

작성: 2026-10-11, 구현 후 갱신. 상태: **구현됨**. 관련: PP-11, PP-22, C-1, C-8, D-4, B-8. 코드: `crates/opensided/src/{tickets,api,host,takeover}.rs`, `crates/opensided/src/tools/{human,browser}.rs`, `crates/openside-browser/src/screencast.rs`, `crates/openside-proto/src/{api,takeover}.rs`. 툴 쪽 상세는 `docs/09` §6, 사이드카 재개 경로는 `docs/specs/sidecar-protocol.md` §6~7.
`unverified` = 확인하지 못한 가정.

## 1. 문제
SMS 코드, 이메일 코드, 기기 푸시 승인(Apple "허용"), CAPTCHA, passkey, 본인확인은 **에이전트가 우회하면 안 되는 벽**이다.
에이전트는 이 과정을 사람에게 넘겨야 한다. 문제는 그 사람이 자리에 없을 때다.

사람 입력을 도구 호출 블록(질문·승인 `Prompt`)으로 받던 초기 모델에는 한계가 있었다.
1. 몇 분~몇 시간 걸리는 사람 개입 동안 에이전트 턴/프로세스를 계속 붙잡아 둔다.
2. 사람이 준 **코드(비밀)** 가 모델 컨텍스트/transcript 에 들어간다.
3. "사람이 직접 화면에서 해야 하는 일"(CAPTCHA, passkey, 푸시 승인)을 표현하지 못한다.
4. 푸시 알림 → 폰에서 처리 → 자동 재개 흐름이 정의돼 있지 않다.

현재 구현은 `Prompt` 리소스를 없애고 **티켓**으로 통일했다(D1): `ask_user` 는 `info` 티켓, 정책 `Ask` 는 `confirm` 티켓이다.

## 2. 핵심 아이디어: 티켓 = 보류된 tool call
에이전트는 벽을 만나면 도구 `request_human_step`(또는 `ask_user`)을 호출한다. 데몬은 **티켓을 만들고 호출을 보류한다**. 사람이 해결하면 보류됐던 **같은 tool call(같은 `tool_call_id`)** 이 이어서 처리된다.

```
agent ── request_human_step(kind, summary, tab_id?, origin?, field_ref?, resume_when?) ──▶ daemon(tool_call reverse RPC)
daemon: 티켓 생성(영속) + ticket.opened 이벤트 → 사이드카에 outcome deferred
sidecar: CallDeferred / ApprovalRequired → 런이 DeferredToolRequests 로 종료 (프로세스는 재기동에도 안전, 상태는 state_snapshot)
human (폰/웹/CLI): resolve / done / reject
daemon: 해결 → ticket_resolved → 사이드카가 같은 tool call 의 결과를 채워 새 런으로 재개
        만료/거부 → 같은 경로로 결과 전달(expired/rejected), 에이전트는 대안 또는 중단
```
- **90초 블록 fast path 는 없다**(D2). 사람 단계는 항상 보류이고 호출 시간 제한을 쓰지 않는다. 대기 중인 상태는 데몬의 티켓·세션 상태(`waiting_human`)이며 프로세스 재시작·네트워크 단절에도 남는다(이벤트 소싱, PP-24).
- 두 종류의 보류(`TicketDeferral`): `approval` — 정책 `Ask` 의 `confirm`. 승인되면 원 호출을 같은 `tool_call_id` 와 `approval_ticket` 으로 **다시 실행**하고, 거부·만료는 `ToolDenied(reason)`. `external` — 사람의 작업 결과가 곧 그 tool call 의 결과(`request_human_step`, `ask_user`).
- 사이드카 쪽 매핑: `approval` → `ApprovalRequired(metadata={ticket_id})`, `external` → `CallDeferred(metadata={ticket_id})`. 보류가 모두 해결되면 `DeferredToolResults` 로 재개한다(재개는 최대 1회).
- 티켓이 열려 있는 동안 데몬은 큐의 새 메시지를 사이드카로 보내지 않는다(D4). 세션 stop 은 열린 티켓을 `rejected(reason:"stopped")` 로 닫는다.

## 3. 종류(kind)와 처리 방식
| kind | 예 | 사람이 하는 일 | 값의 흐름 | 완료 판정 |
| --- | --- | --- | --- | --- |
| `confirm` | 결제/게시/전송 승인 (C-8, D-4) | 승인/거부 (`resolve` 의 `true`\|`false`\|`"approve"`\|`"reject"`) | `{"approved":bool}` 만 데몬에 남고 모델로는 원 호출의 실행 결과 또는 거부 사유만 간다 | 응답 |
| `secret_input` | SMS/이메일 인증코드, 본인확인 번호 | 코드 입력 (`resolve` 의 문자열 값) | **모델·사이드카로 안 간다.** 데몬이 노드 핸들로 고정된 입력란에 직접 입력 | 입력 성공 |
| `device_approval` | Apple/Google "다른 기기에서 허용", 앱 푸시 승인 | 자기 기기에서 승인(openside 는 관여 안 함) | 값 없음 | `resume_when` 조건 충족 또는 `done` |
| `takeover` | CAPTCHA, passkey/보안키, 기타 | 원격 화면(JPEG 프레임)에서 직접 조작 | 입력은 탭으로만 간다. 모델에는 끝난 뒤의 페이지 요약만 간다 | `done` |
| `info` | 에이전트의 일반 질문 | 텍스트 답 (`options` 가 있으면 그 `value` 중 하나) | `{"answer": …}` 로 모델에 감(비밀 아님) | 응답 |

`request_human_step` 인자: `kind`, `summary`, `instructions?`, `tab_id?`, `origin?`, `field_ref?`, `resume_when?`, `options?[{value,label}]`. `ask_user` 인자: `question`, `options?[string]`. 두 툴 모두 tier `read`, 정책 capability 없음(항상 Allow).

### secret_input (모델 비노출) — Vault 원칙과 동일
- 호출 인자: `request_human_step{kind:"secret_input", summary, tab_id, origin:"https://…", field_ref:"e41"}`. 셋(`tab_id`, `origin`, `field_ref`) 모두 필수이고 `origin` 은 자격증명·경로·쿼리·프래그먼트가 없는 HTTP(S) origin 이어야 한다.
- **티켓을 열 때** `field_ref` 를 해당 탭의 노드 핸들로 고정한다(탭 소유권·origin 일치 확인). 이후 페이지가 바뀌어도 ref 문자열이 아니라 이 핸들로만 입력한다.
- 사람의 응답 값(`resolve`)은 데몬 메모리에서만 다룬다(`Zeroizing`). 데몬이 현재 탭 origin 이 티켓의 `origin` 과 **정확히 같은지** 다시 검사한 뒤(불일치 `origin_mismatch`) redactor 를 먼저 등록하고, 그 문서를 비밀 문서로 표시한 다음 노드 핸들 기반 `insert_text_node` 로 입력하고 zeroize 한다. 이벤트·DB·감사 로그에는 값도 길이도 남지 않는다.
- 모델이 받는 결과: `{"status":"filled"}`. 코드 값은 어떤 도구 결과에도 없다. 제출(버튼 클릭)은 에이전트가 이어서 한다(`auto_submit` 옵션은 구현하지 않았다).
- 값을 채운 문서에서는 `browser_screenshot` 이 거부된다(`screenshot_unavailable_after_secret_fill`, 힌트 `navigate or take a snapshot instead`). 비밀 문서 표시는 `browser-state.json` 에 저장돼 데몬 재시작 뒤에도 유지된다. 재시작 전에는 redactor 가 스냅샷의 값을 가린다. 데몬 재시작 뒤에는 메모리의 redaction 필터가 없으므로, 비밀 문서로 기록된 문서의 `browser_snapshot`(전체·diff)을 `snapshot_unavailable_after_secret_fill` 로 거부하고 내비게이션 후 다시 허용한다(테스트 `recovered_secret_document_denies_snapshot_until_navigation`). 비밀 문서 표시는 값 입력 전에 디스크에 먼저 기록한다.
- `resolve` 오류 코드: `409 origin_mismatch`(티켓은 pending 유지), `409 secret_input_failed`(노드·입력 실패), `503 secret_input_unavailable`(브라우저 없음), 입력 싱크가 없으면 `501 secret_input_unavailable`, 값이 문자열이 아니면 `invalid_value`. 입력 전에 값 없는 입력 의도(티켓 id·작업 id)를 먼저 기록하고, 이전 시도가 커밋되지 않은 채 남아 있으면 다시 넣지 않고 `409 secret_input_indeterminate` 로 거절한다(티켓은 pending 유지; 사람이 reject 하거나 에이전트가 새 단계를 연다). 입력은 페이지와 분리된 격리 월드(`Page.createIsolatedWorld`)에서 노드에 묶인 한 번의 `Runtime.callFunctionOn`(최상위 문서·연결 상태·origin·편집 가능 요소 확인 후 격리 월드의 value setter + `input`/`change`)으로 한다. 페이지가 프로토타입 setter·`Object.getOwnPropertyDescriptor`·`dispatchEvent` 를 바꿔도 값을 가로채지 못한다(테스트 `secret_insertion_ignores_page_world_setter_and_builtin_hooks`). 비밀 문서 표시는 탭마다 최근 32개 문서를 보관하고, 티켓의 문서가 탭의 현재 문서일 때만 추가하므로 낡은 티켓이 새 문서의 표시를 덮어쓰지 못한다(`stale_secret_ticket_cannot_displace_newer_document_taint`). 입력은, 커서 위치 추가가 아니라 값 교체다.
- 같은 대상(`tab_id`+`origin`+`field_ref`)에 대한 `secret_input` 요청은 세션당 3회로 제한한다(`secret input target request limit reached`).

### request_otp — 인증 코드 한 가지 도구

웹 페이지에서 인증 번호나 일회용 코드 입력을 요구하면(OTP 앱, SMS, 이메일, 백업 코드 및 여러 칸으로 나뉜 입력란 포함) 에이전트는 `request_otp(tab_id, field_ref, channel?, summary?)`를 호출한다. 이는 시스템 프롬프트와 도구 설명에 모두 명시되어 있다. `channel`은 `authenticator`, `sms`, `email`, `backup_code`, `unknown` 중 하나를 지정하며 기본값은 `unknown`이다. 대상 origin은 모델이 임의로 지정하지 않고 항상 탭에서 직접 가져온다. 이때 적용되는 정책 작업은 해당 origin에 대한 `Browse`와 `VaultUse`다.

탭 origin에 등록된 TOTP 볼트 항목이 정확히 하나이고 `channel`이 `authenticator` 또는 `unknown`이면, 데몬이 볼트 콜백 내부에서 현재 코드를 생성해 즉시 입력한다. 티켓은 발행되지 않으며 모델에는 `{"status":"filled"}`만 반환된다. 감사 로그에는 `otp` 액션으로 기록된다. 해당 origin에 등록된 TOTP 항목(계정)이 여러 개이면 어느 것도 선택하지 않는다. 이때는 티켓을 열어 사용자가 대상 계정의 코드를 직접 입력하도록 유도하며, 다른 인증기 등록 옵션은 제공하지 않는다.

그 밖의 경우에는 `otp: {channel, can_register}`를 담은 `secret_input` 티켓을 연다. `can_register`는 볼트가 설정되어 있고, `channel`이 `authenticator` 또는 `unknown`이며, 해당 origin에 등록된 계정이 여러 개가 아닐 때만 `true`가 된다.

사용자는 다음 두 가지 방식 중 하나로 응답한다.
- 코드 문자열: 티켓에 고정된(pinned) 필드에 입력한다. 코드를 여러 칸으로 나누어 입력하는 동작은 `request_otp` 및 `register_authenticator` 티켓에서만 처리한다. 티켓이 열리면 데몬은 대상 필드 뒤에 이어지는 한 글자 입력란 그룹(`maxlength=1` 또는 공통 `autocomplete=one-time-code`)을 고정하고, 각 칸에 값을 쓰기 전에 그룹 상태를 다시 확인한다. 일반 `secret_input` 티켓은 정확히 하나의 필드에만 값을 입력한다.
- `can_register`가 `true`일 때 TOTP 등록: `{"totp": "<otpauth:// URI 또는 base32 설정 키>"}`를 전송한다. 인증 요소를 영구적으로 추가하는 작업이므로, 이 응답을 처리하려면 `human_step` 외에 `approve` 스코프가 추가로 필요하다. 두 스코프 중 하나만 가진 기기는 403 `forbidden`을 수신하며 티켓은 대기(pending) 상태를 유지한다. 데몬은 먼저 유효성 검증을 거친다(400 `invalid_totp`이면 티켓이 대기 상태로 유지되고, 볼트가 없으면 503 `vault_unavailable`을 반환한다). 직접 입력한 설정 키를 저장하기 전에 데몬은 브라우저 게이트를 획득하고 티켓 대상을 다시 검증한다. 탭에 열려 있는 takeover가 없고, 같은 세션이 여전히 탭을 소유하고 있으며, origin이 바뀌지 않았고, 고정된 필드가 여전히 동일한 문서 안에 있어야 한다. 검증이 하나라도 실패하면 409를 반환하고 아무것도 저장하지 않으며, 티켓은 대기(pending) 상태로 남는다. 설정 키는 티켓의 origin에 정확히 바인딩된 새로운 TOTP 전용 항목 `otp-<host>[-<port>]`로 저장된다. 이 항목은 기존 항목과 절대 병합되지 않으며 기존 항목을 대체하지도 않는다. 해당 origin을 허용하는 기존 항목에 이미 TOTP가 등록되어 있으면 409 `otp_already_registered`로 응답을 거부하며, 항목 정책이 `never`이면 403 `vault_denied`를 반환한다. 감사 로그에는 승인한 기기를 `device:<id>` 또는 `local`로 지정하여 `otp_register`(`allowed` 후 `registered`)를 기록한다. 이후 현재 코드를 입력하며 최종 결과로 `{"status":"filled","registered":true}`를 반환한다. 인증 요소는 저장되었으나 코드를 입력하지 못한 경우(페이지가 이동했거나 완료 감사 기록을 작성하지 못한 경우), 티켓은 여전히 종료 처리되며 `{"status":"registered","filled":false}`를 반환한다. 이후 에이전트가 `request_otp`를 다시 호출하면 볼트에서 코드를 가져와 입력한다. 동일 origin에 대한 이후 호출은 사용자 확인 없이 코드를 자동으로 채운다.

사용자가 응답하는 경로와 처리 방식은 다음과 같다.
- 웹 UI: 인증 코드 요청이 발생해도 다른 화면으로 이동하지 않는다. 요청이 시작된 대화 화면에서는 입력 폼이 본문 안에 바로 나타나며, 설정을 포함한 다른 화면에 있을 때는 작은 플로팅 카드로 표시된다. 푸시 알림(`/sessions/<id>?ticket=<ticket>`)을 눌렀을 때만 전체 화면 코드 시트가 열리고, 제출을 마치면 원래 대화로 돌아간다. 입력 폼에는 `autocomplete=one-time-code` 속성을 가진 입력창 하나만 둔다. 등록이 가능한 환경에서는 "Save this site's authenticator" 접이식 섹션에 마스킹된 설정 키를 입력받는다. "Don't ask again for this site" 체크박스가 켜져 있으면(기본값) `{"totp": …}` 형태로 전송하고, 끄면 페이지에서 직접 현재 코드를 계산해 코드만 보낸다. 모든 입력값은 요청을 전송하기 전에 비운다.
- 브라우저 확장: 어느 대화에서 들어온 코드 요청이든 Ask 사이드 패널에 동일한 코드 입력 폼을 표시한다. Chrome에서는 사용자 조작 없이 사이드 패널을 열 수 없으므로, 툴바 배지에 대기 중인 요청 수를 띄우고 요청마다 알림을 한 번씩 보낸다. 알림을 클릭하면 사이드 패널이 열린다. 어떤 화면도 스스로 포커스를 가져가지 않는다.
- CLI: `openside ticket resolve <id>`처럼 값을 전달하지 않고 실행하면 입력 내용이 화면에 가려지는 비공개 프롬프트로 코드를 묻는다(터미널 환경에서만 동작). `openside ticket resolve <id> -`는 표준 입력(stdin)에서 코드를 읽어 들인다. `--totp` 옵션을 붙이면 동일한 두 가지 방식으로 설정 키를 전달할 수 있다. 그 밖의 다른 형식은 입력을 거부한다.
- 챗봇: 시크릿 값을 일절 수신하지 않으며 인증기 등록도 승인하지 않는다. 카드에는 `{remote.public_url}/sessions/<session>` 링크(또는 웹 UI 안내)와 CLI 명령어만 표시하고, 등록 티켓의 버튼 입력은 거부한다. 이는 기본 제공 챗 클라이언트에 적용되는 규칙이며, 서버 자체는 `approve` 스코프를 가진 모든 토큰을 정상 수용한다.
- MCP: `ticket_resolve` 도구는 여전히 `secret_input` 티켓 처리를 거부한다.
- SMS 전달: `POST /v1/otp`로 수신된 코드는 해당 origin을 대상으로 열린 일반 `secret_input` 티켓과 동일하게 이 티켓을 자동으로 채운다.

비어 있지 않은 모든 시크릿 값은 길이에 상관없이 출력 결과에 포함될 때마다 부분 문자열 단위로 마스킹된다. 데몬이 자체 생성하는 값(ID, 타임스탬프, 열거형 태그, 상태)은 티켓 및 이벤트의 지정된 스키마 위치에만 보존된다. 티켓의 대상 문자열(`tab_id`, `origin`, `field_ref`)은 데몬이 티켓을 열 때 실제 브라우저와 대조하여 확인을 마친 부분만 원문 그대로 유지한다. 데몬이 고정한 `field_ref`가 있는 대상은 세 항목을 모두 유지한다. `field_ref`가 없는 대상은 현재 세션이 해당 탭을 소유하고 있을 때만 `tab_id`를 유지하고, 탭의 현재 origin과 일치할 때만 `origin`을 유지한다. 브라우저가 실행 중이지 않거나 검증에 실패하면 다른 일반 텍스트와 마찬가지로 마스킹한다. 반면 응답 옵션처럼 모델이 생성한 텍스트는 일반 텍스트와 동일하게 마스킹을 거친다. 시크릿이 입력된 후에는 해당 문서의 `browser_snapshot`에서도 모든 텍스트 필드 값이 `<redacted>`로 가려진다. 이러한 오염(taint) 상태는 URL이 아니라 문서에 귀속된다. 따라서 `history.pushState`나 해시 변경으로 URL이 바뀌더라도 오염 상태가 유지되며, 데몬을 재시작해도 마찬가지로 유지된다.

### register_authenticator — 인증기 등록

사이트의 2단계 인증 설정 페이지에서 에이전트는 `register_authenticator(tab_id, field_ref?)`를 호출한다. 데몬은 설정 키를 다음 순서로 직접 읽어 들인다. 먼저 페이지 내의 `otpauth://` 링크(격리된 월드에서 읽음), 탭을 전면으로 가져오지 않고 캡처한 스크린샷에서 디코딩한 QR 코드, 그리고 화면에 표시된 base32 텍스트 키 순이다. 키가 먼저 발견된 소스를 기준으로 채택하며, 숨겨진 요소를 포함해 페이지 내 어디에든 존재하는 링크는 QR 코드나 텍스트보다 우선한다.

다른 작업에 앞서, 추출된 모든 후보 키와 이를 파싱한 각 표현형은 모든 출력에서 즉시 마스킹되며, 해당 탭은 스크린샷과 스크립트 실행이 거부되도록 오염(taint) 처리된다. 페이지에 서로 다른 키가 둘 이상 표시되는 경우에도 동일하게 처리되며 `setup_key_ambiguous`로 요청을 거부한다. 키를 찾지 못하면 `setup_key_not_found`를 반환하고, 볼트 항목에 이미 인증기가 등록된 origin이면 `otp_already_registered`를 반환한다.

설정 키는 데몬 메모리에만 임시 보관된다. 키 본문은 제외하고 `otp_register {origin, issuer, account}` 정보만 담은 `confirm` 티켓을 열어 사용자의 승인을 요청하며, 승인에는 `approve` 스코프가 필요하다. 사용자가 승인하면 데몬은 브라우저 게이트(browser gate)를 획득하고, 저장 직전에 탭의 소유 세션이 동일한지, 사용자가 직접 조작(takeover) 중이지 않은지, 탭에 키를 읽어 들였던 동일 문서가 그대로 표시되어 있는지, origin이 변경되지 않았는지를 차례로 검증한다. 이 중 하나라도 실패하면 409를 반환하며 키를 보존한 채 티켓 대기(pending) 상태를 유지한다. 검증을 모두 통과하면 `request_otp`와 동일한 방식으로 키를 저장하고, `field_ref`가 지정되어 있다면 확인 코드를 입력한다. 결과로는 `{"status":"registered","filled":true|false}`를 반환하며, 페이지 URL 이동 등으로 고정된 필드가 무효화(stale)되었을 때는 `filled`가 false가 된다.

거절(declining) 시에는 `{"approved":false,"status":"declined"}`를 반환하고 키를 즉시 폐기한다. 키는 명시적 거절(reject), 만료(expiry), 세션 중지 시에도 폐기되며, 티켓이 열리지 않은 채 24시간이 지나면 다음 등록 시점에 정리(prune)된다. 데몬 재시작 후 승인을 시도하면 `{"status":"setup_key_lost"}`를 반환한다. 키에 적용된 마스킹 필터는 데몬이 종료될 때까지 메모리에 유지되므로, 이후 키가 다시 출력되더라도 가려진 상태를 유지한다. 키를 저장하기 전에 데몬은 새로 발급한 작업 ID(operation id)가 담긴 입력 의도(fill intent)를 기록하며, 볼트는 새 항목을 암호화하여 기록할 때 이 작업 ID를 함께 저장한다(기기 외부로 나가지 않으며 동기화 시에도 내보내지 않는다). 이후 티켓 커밋에 실패하거나 데몬이 종료되더라도, 뒤따르는 모든 종료 경로(응답 반환, 명시적 `reject`, 만료, 세션 중지, 서브에이전트 취소)에서는 정확히 해당 작업 ID로 저장된 인증 수단이 볼트에 존재하는지 확인한다. 인증 수단이 있으면 티켓은 `declined`, `rejected`, `expired`, `setup_key_lost` 대신 `{"status":"registered","filled":false}`로 종료된다. 볼트 확인 결과 해당 작업으로 저장된 내역이 전혀 없다면 일반적인 응답이나 거절로 처리하며, 다른 주체가 동일한 origin에 등록한 인증 수단은 여기에 포함하지 않는다. 볼트가 잠겨 있거나 사용할 수 없는 경우, 또는 입력 의도를 읽을 수 없어 확인이 불가능하다면 티켓은 `{"status":"registration_indeterminate"}`로 종료된다. 등록 기능을 제공하는 `request_otp` 티켓에도 동일한 규칙을 적용한다.

락(lock)은 항상 티켓 게이트 → 브라우저 게이트 → 볼트 순서로 획득하며, 볼트 락을 쥔 상태에서 브라우저 게이트를 대기하는 일은 결코 발생하지 않는다. 탭을 전면으로 가져오지 않으며, 코드 입력을 위해 takeover를 열지도 않는다.

알려진 한계: QR 코드는 가장 큰 정사각형 이미지를 뷰포트로 스크롤했을 때 화면에 보여야만 인식할 수 있다. 에이전트가 등록 전에 촬영한 스냅샷에는 텍스트 설정 키가 이미 포함되어 있을 수 있다. 아울러 에이전트 프로필 내 Chromium 세션 히스토리가 뒤로 가기/앞으로 가기 복원을 위해 폼의 POST 바디(코드 포함)를 보관할 수 있으며, openside는 이를 별도로 삭제하지 않는다.

### device_approval — 가장 쉬운 벽
Apple/Google 의 "기기에서 허용" 은 openside 가 할 일이 없다. 사람은 자기 기기에서 누르고, 에이전트는 **조건 대기**만 한다.
- `resume_when` 은 필수이고 다음 중 정확히 하나다: `{url_not_contains:"…"}` | `{selector_visible:"…"}` | `{text_visible:"…"}`(값은 비어 있으면 안 됨) | `{manual:true}`. 자동 판정이면 `tab_id` 도 필요하다.
- 데몬 `Controller::poll` 이 **2초 간격**으로 열린 `device_approval` 티켓의 탭을 평가(`location.href`, `querySelector` 가시성, `innerText`)하다 조건이 충족되면 사람 없이 `{"status":"done"}` 으로 해결하고 재개한다. `{manual:true}` 는 `done` 호출만 기다린다. 이 폴링은 세션 소유 탭에서만 평가한다.
- 같은 tool call 이 이어지는지는 실제 데몬·사이드카·Chromium E2E 로 확인한다: 3초 뒤 문구가 바뀌는 페이지에서 아무도 `done` 을 부르지 않아도 원래 `request_human_step` 호출이 `{"status":"done"}` 을 받고 런이 끝난다(`crates/openside-e2e/tests/takeover.rs::hs4_device_approval_condition_resumes_same_call_without_a_human`).
- 번호 일치 확인(Apple 이 보여주는 2자리 숫자 등)이 있으면 에이전트가 페이지 스냅샷에서 읽어 `instructions` 에 넣어 알림에 보이게 한다.

### takeover — 원격으로 직접 하기
- 티켓: `request_human_step{kind:"takeover", summary, tab_id}`(TTL `takeover_ttl_secs`, 기본 1800초). `done`·`reject`·만료로 닫히고 `resolve` 로는 못 닫는다(`400 use_done`).
- 스트림: `GET /v1/tickets/{id}/takeover` 를 WebSocket 으로 업그레이드한다. 스코프 `human_step`. 메시지는 양방향 JSON 텍스트다(스키마 `TakeoverServerMessage`, `TakeoverInput`).
  - 데몬 → 클라이언트: `frame{data(base64 JPEG), width, height}`, `withheld{code}`, `input_error{code}`, `ended{reason: done|rejected|expired|idle|revoked|tab_unavailable}`. 프레임은 CDP `Page.startScreencast`(JPEG 품질 60, 최대 1440x900)로 만들고 최대 10fps 다. 확인 응답(ack)을 늦춰 Chromium 이 다음 프레임을 만들지 않게 하고, 읽지 못한 프레임은 가장 최근 1장만 남긴다(무한 버퍼 없음). 쓰기는 별도 작업이 맡는다. 송신 대기열은 8칸이고 그중 4칸은 `input_error`·`ended` 몫으로 남겨 둔다. 클라이언트가 밀리면 프레임을 버리고, 한 번의 쓰기가 5초 안에 끝나지 않으면 연결을 끊는다. 읽지 않는 클라이언트도 컨트롤러 자리와 캡처를 붙잡지 못한다.
  - 클라이언트 → 데몬: `mouse{event:down|up|move, x, y, button?, click_count?, modifiers?}`, `wheel{x, y, delta_x, delta_y}`, `key{event:down|up, key(≤32바이트), code?(≤32바이트), text?(≤16바이트), key_code?, modifiers?}`, `text{text}`(최대 4096바이트). 메시지 하나는 최대 32KiB. 좌표는 뷰포트 CSS 픽셀(0≤x<1440, 0≤y<900)이고 벗어나면 `input_error{out_of_bounds}`. 입력은 CDP `Input.dispatchMouseEvent`/`dispatchKeyEvent`/`insertText` 로 **티켓의 탭에만** 보낸다. 페이지로 보낸 단축키(Ctrl+W/T/L/Tab, F12)는 브라우저 UI 에 닿지 않는다(실브라우저 테스트로 확인). 스트림 동안 그 탭은 포커스 에뮬레이션을 켠다.
  - 인증: `Authorization: Bearer` 헤더. 브라우저는 WebSocket 에 헤더를 넣을 수 없으므로 웹 UI 는 `Sec-WebSocket-Protocol: openside.v1, openside.bearer.<token>` 으로 토큰을 보낸다. 데몬은 이 값을 세 조건이 모두 맞을 때만 자격 증명으로 쓴다. ① `Authorization` 헤더가 아예 없다. 헤더가 있으면 형식이 틀려도 그 헤더만 본다. ② 경로가 `api.rs` 의 `WEBSOCKET_ROUTES` 에 있는 GET 이다. ③ 진짜 핸드셰이크다(`Connection: upgrade`, `Upgrade: websocket`, `Sec-WebSocket-Version: 13`, 16바이트 base64 `Sec-WebSocket-Key`). 토큰 문자는 `[A-Za-z0-9._~-]` 만 허용하고, 응답에는 `openside.v1` 만 돌려준다.
  - Origin 정책: 스킴은 비교하지 않고 호스트만 같은지 본다. `Origin` 의 호스트·포트가 `Host` 와 같아야 하고, 스킴은 `http`·`https` 둘 다 받는다. TLS 를 끝내는 리버스 프록시 뒤에서는 데몬이 `http` 로 요청을 받지만 브라우저는 `https` origin 을 보내기 때문이다.
  - 거부: 토큰 없음 `401`, 스코프 부족 `403 forbidden`, `Origin` 헤더가 있는데 `Host` 와 다르면 `403 origin_forbidden`(리버스 프록시는 `Host` 를 그대로 넘겨야 한다), 업그레이드 요청이 아니면 `426 websocket_required`, 이미 다른 컨트롤러가 붙어 있으면 `409 takeover_in_use`(티켓당 1개), 탭이 없거나 세션 소유가 아니면 `409 tab_unavailable`, `tab_id` 없는 티켓은 `409 needs_desktop`(데스크톱에서 처리 후 `done`).
  - 종료: 티켓이 해결·거부·만료되면 `ended` 를 보내고 닫는다. 받아들여진 입력 없이 300초가 지나면 `idle` 로 닫는다(잘못된 입력은 활동으로 치지 않고, 다시 붙을 수 있다). 컨트롤러 기기가 폐기되면 즉시 `revoked`. 입력은 그 기기의 폐기 게이트를 읽기로 잡은 채 폐기 여부를 다시 보고 실행되므로, `DELETE /v1/devices/{id}` 는 이미 받아들인 입력이 끝난 뒤에야 돌아오고 그 뒤로는 어떤 입력도 실행되지 않는다. idle 시한과 상태 확인은 입력 처리보다 먼저 검사하므로, 거부되는 입력을 쉬지 않고 보내도 시한을 넘겨 버틸 수 없다. 어떤 이유로 끝나든 먼저 입력 접수를 닫고 캡처를 멈춰 컨트롤러 자리를 비운 뒤에 `ended`·close 를 보낸다(최대 1초 시도).
- 에이전트 쓰기 락(결정: 대기 대신 **즉시 거부**): takeover 티켓을 여는 호출은 그 탭에서 이미 실행 중인 에이전트 브라우저 작업(브라우저 게이트를 잡은 호출)이 끝날 때까지 기다린 다음, 탭을 예약하고 티켓을 게시한다. 예약 시점부터 티켓이 끝날 때까지 그 탭을 대상으로 하는 `browser_*`·`vault_*` 호출은 읽기(`browser_snapshot`, `browser_screenshot`)까지 실행하지 않고 `takeover_active` 로 거부된다. 만료 시각이 지나도 만료 처리(1초 주기)가 끝날 때까지는 잠긴다. `tab_id` 없는 `browser_repl` 도 거부한다. `browser_tabs`·`browser_open_tab` 은 허용한다. 공개 `POST /v1/sessions/{id}/repl`(외부 에이전트·MCP)도 같은 호스트 파이프라인이라 똑같이 거부된다. 대기를 고르지 않은 이유: 사람 단계는 몇 분~몇 시간 걸리고 대기 중인 호출이 턴을 붙잡는다.
- 끝나는 순서: 모든 종료 경로(`done`, `reject`, 만료, 세션 stop, 서브에이전트 취소)는 티켓 게이트 안에서, 상태를 바꾸기 **전에** 다음을 한다. ① 입력 접수를 닫는다(이미 접수된 입력은 끝까지 실행된다). ② 캡처를 멈춘다. ③ 컨트롤러 연결 여부와 상관없이 그 탭의 스냅샷 캡처와 요소 ref 를 모두 폐기한다. 그다음 상태를 게시해 에이전트에게 탭을 돌려주고 원래 호출을 재개한다. 따라서 종료 뒤에는 사람 입력이 실행되지 않는다.
- 종료 후 재개: `done` 이면 원래 `request_human_step` 호출이 `{"status":"done","tab_id","page_changed":true,"url","title","note"}` 를 받는다(redaction 적용). 이전 `snapshot_id` 를 `since` 로 줘도 diff 가 아니라 전체 캡처가 온다. 이전 `eN` ref 는 같은 노드가 남아 있어도 stale 이고, 다음 스냅샷은 새 번호를 매긴다.
- 비밀: Vault·`secret_input` 으로 값을 넣은 적이 있는 탭은 이후 모든 프레임을 `withheld{screenshot_unavailable_after_secret_fill}` 로 바꿔 보낸다. 다른 문서로 이동한 뒤에도 마찬가지다. 버퍼에 있던 JPEG 가 어느 문서를 그린 것인지 알 수 없고, 보는 기기가 비밀 정책이 믿은 기기가 아닐 수 있기 때문이다. 입력은 계속 된다. 데몬은 받은 입력 메시지를 이벤트·DB·감사·오류 메시지에 남기지 않는다(오류는 고정 코드만). 로거를 설치하지 않으므로 의존성의 trace 로그(WebSocket 수신 내용 포함)도 어디에도 기록되지 않는다. 사람이 입력한 결과가 페이지에 남으면 이후 스냅샷에는 당연히 보인다.
- `takeover.*` 이벤트는 없다. 연결 상태는 클라이언트가 소켓으로 안다.
- CAPTCHA 해결 서비스 연동, 자동 풀이는 **하지 않는다**(우회 금지 정책). passkey/보안키는 사람 기기에서만 가능하므로 `device_approval` 로 처리한다.

### Read-only session browser preview (WS-5)

- `GET /v1/sessions/{id}/browser/preview` upgrades to a WebSocket without a human-step ticket. It requires `browse` (`steer` implies `browse`), not `read`: JPEG pixels cannot be redacted like text events. The same-origin and browser bearer-subprotocol rules above apply; only `openside.v1` is echoed.
- `PreviewServerMessage` is server-only: `frame{data(base64 JPEG),width,height,tab_id}`, `withheld{code,tab_id}`, `paused{reason:"takeover",tab_id}`, `no_tab`, and `ended{reason:revoked|session_stopped|session_deleted}`. Text and binary client messages are ignored, with a 1 KiB message/frame limit; this route never forwards input.
- Preview follows the most recent successful browser tool's owned tab, or the sole live owned tab if no current tab is known. It does not launch a browser. JPEG quality is 40 and encoded dimensions are at most 720x450. Chromium has a bounded startup allowance of at most three frames (older binaries ignore `maxFramesInFlight:1`); thereafter one capture credit is released per 500 ms, including replaced frames, enforcing sustained capture at most 2 fps. Each viewer's delivery is at most 2 fps, including startup. Takeover retains its eager replacement ACKs for low latency. A session shares one latest-frame feed among at most eight viewers; additional viewers receive `429 too_many_viewers`.
- A takeover reservation stops the preview capture before human capture starts. The preview clears its image and reports `paused` while the ticket is pending, then automatically resumes. Secret-tainted tabs report `withheld{screenshot_unavailable_after_secret_fill}`, including after navigation. Before a secret is inserted or takeover begins, capture is stopped and its generation is invalidated. The writer's final synchronous admission rechecks that generation, device revocation and the stream's latched lifetime. Queued stale frames are discarded. At most one frame admitted **before** a takeover/taint/revocation transition may finish sending afterward, within the five-second write deadline; no frame captured after that transition is sent while the exclusion holds. Private sessions refuse the upgrade with `409 private_session`.
- Each viewer has an eight-message bounded writer queue and reserves four slots for control messages. Revocation closes only that device's sockets. Committed stop/deletion latches an `ended` reason for existing viewers, even if the session immediately resumes; a new viewer can attach to the resumed session. Handshake registration replays terminal events since its initial state read to close the registration race. The last viewer leaving stops capture. Final messages and the close handshake are best-effort within one second, after capture resources are released.
- The session screen's Browser preview toggle opens a read-only corner PiP; Expand/Minimize changes its size without a new connection. Private sessions hide the toggle. The shared `BrowserFrame` renderer is also used by `TakeoverView`.
- Evidence: `crates/openside-browser/src/screencast_tests.rs` checks deterministic CDP credits and real animated Chromium frame/ACK counts; `crates/opensided/src/tests/preview.rs` includes stop/immediate-resume, explicit no-frame windows with visible secrets and takeover input, foreign-owned tabs, and before/after writer-admission barriers; `web/src/components/BrowserPreview.test.tsx` and `crates/openside-e2e/tests/web_pip.rs::web_pip_updates_jpeg_pixels_when_the_agent_navigates_without_a_takeover_ticket` cover the displayed image.

## 4. 자동화 가능한 보조 경로 (옵트인)
1. **TOTP**: Vault 에 시드가 있으면 사람 없이 `vault_otp` 가 처리한다(C-2). 이건 human step 이 아니다. **구현됨.**
2. **SMS 코드 자동 전달**: **구현됨.** `otp_ingest` 범위 토큰으로 `POST /v1/otp {sender, text}` 를 부른다. 발신자는 `[otp] senders` 와 정확히 일치해야 하고(아니면 403 `sender_not_allowed`, 감사 기록), `[otp.sites]` 의 origin 별 정규식으로 코드를 뽑아 그 origin 의 대기 중인 `secret_input` 티켓만 채운다. 티켓이 아직 없으면 `ttl_secs`(기본 300초) 동안 메모리에서 기다렸다가 맞는 티켓이 열리면 채운다. 코드는 디스크·이벤트·감사에 남지 않는다. 설정은 [`docs/CONFIGURATION.md`](../CONFIGURATION.md) `[otp]`, 사용법은 [`docs/REMOTE.md`](../REMOTE.md) §4. iOS 단축어·Android 문자 전달 앱의 자동 실행 가능 여부는 `unverified`.
3. **이메일 코드**: IMAP 읽기는 계정 자격증명을 추가로 맡기는 일이라 기본 제공하지 않는다. 사용자가 원하면 별도 옵트인 모듈. **미구현.**

## 5. API/프로토콜 (구현)
- 별도 `Ticket` 리소스(수명이 다름: 시간 단위 영속).
  `Ticket { id, session_id, tool_call_id, deferral(approval|external), kind, summary, instructions?, options[{value,label}], origin{subagent_id?}, tool?{name, args(redacted)}, target?{tab_id?, origin?, field_ref?}, resume_when?, created_at, expires_at, status: pending|resolved|rejected|expired|needs_desktop, resolution? }`. 저장 전에 모든 문자열이 redaction 을 거치고 `resolution` 에 비밀 값은 없다. `origin.subagent_id` 는 서브에이전트가 만든 티켓의 소유자를 표시하는 필드이며 서브에이전트 어댑터가 없는 지금은 항상 비어 있다(BLOCKED-1).
- 엔드포인트: `GET /v1/tickets[?status=pending|…]`, `GET /v1/tickets/{id}`, `POST /v1/tickets/{id}/resolve`(`confirm`/`info`/`secret_input` 값), `POST /v1/tickets/{id}/done`(`device_approval`·`takeover`; 다른 kind 는 `400 invalid_ticket_kind`), `POST /v1/tickets/{id}/reject`, `GET /v1/tickets/{id}/takeover`(WebSocket, §3). pending 이 아니거나 만료된 티켓은 `409`.
- 이벤트: `ticket.opened`, `ticket.resolved`, `ticket.rejected`, `ticket.expired`. 비밀 값은 이벤트에 포함하지 않는다. `takeover.*` 이벤트는 없다.
- 능력 토큰 스코프: `read`(조회만), `steer`, `approve`(`confirm`·`info` 티켓), `human-step`(`secret_input`·`device_approval`·`takeover` 티켓), `admin`(둘 다 가능, 기기 페어링). `resolve`/`reject` 는 티켓 kind 에 맞는 스코프가 없으면 `403 forbidden`.
- **승인 재실행 보호.** `confirm` 티켓이 승인되면 같은 `tool_call_id` 의 호출을 `approval_ticket` 과 함께 한 번만 재실행할 수 있다. 티켓은 인자와 정책 action 의 정규화 해시에 묶여 있어서, 다른 인자로 쓰면 `approval_args_mismatch`, 이미 쓴 승인을 다시 쓰면 `approval_already_used`, 그 외 같은 세션·툴·`tool_call_id` 가 아니면 `Approval ticket does not authorize this tool call` 로 `denied` 된다(`host.rs`). 승인은 `Deny` 판정을 뒤집지 못한다.
- 에이전트 연동: 호스트 툴 `request_human_step`·`ask_user` 는 데몬이 실행하는 툴이고(`docs/09`), 사이드카에는 reverse RPC 프록시로만 있다. 보류/재개 프로토콜은 `docs/specs/sidecar-protocol.md` §6~7. 티켓이 pending 인 세션은 목록·상세에서 `waiting_human` 으로 파생돼 `settled` 와 구분된다.
- 알림: 티켓이 열리면 데몬이 페어링한 기기의 Web Push 구독으로 알림을 보내고, 누르면 `/sessions/{session}?ticket={id}` 로 열린다([`docs/REMOTE.md`](../REMOTE.md) §3). 채널 봇(A-13)의 deep link 는 채널 어댑터 쪽 기능이다.

## 6. 안전 규칙
1. 에이전트는 인증 벽을 **우회하지 않는다**: 자동 CAPTCHA 풀이, 코드 추측/무차별 대입, 다른 경로 로그인 시도 금지. 현재는 에이전트에게 그런 풀이 도구를 주지 않는 것으로 지킨다(툴은 `docs/09` §3 의 15개뿐). 정책 엔진 `Deny` 규칙화는 아직 없다.
2. 티켓 기본 만료(`[tickets]`, 초 단위, 최소 1, 환경변수 오버라이드 없음): `confirm_ttl_secs=600`(만료=거부), `secret_input_ttl_secs=300`, `device_approval_ttl_secs=1800`, `info_ttl_secs=86400`, `takeover_ttl_secs=1800`. 만료되면 에이전트에 `{"status":"expired"}`(approval 은 `ToolDenied("expired")`)를 전달해 계속 대기하지 않게 한다. 만료는 데몬이 1초 간격으로 처리한다.
3. 페이지가 요구한 사람 개입이 **피싱**일 수 있다. `secret_input` 은 티켓을 열 때 노드를 고정하고 입력 직전 origin 을 정확 일치로 다시 검사한다. UI 는 어느 사이트/어떤 입력란인지 `target` 으로 항상 표시해야 한다(클라이언트 책임). 페이지 내용을 그대로 `instructions` 로 믿지 않고 에이전트가 쓴 요약과 데몬이 확인한 origin 을 함께 보여준다.
4. 한 번 해결된 티켓은 재사용할 수 없다(pending 이 아니면 `409`). 같은 입력란에 대한 반복 요청은 3회로 제한한다.
5. takeover 중에는 그 탭에 대한 에이전트 브라우저 호출을 거부하고(`takeover_active`), 끝나면 그 탭의 스냅샷 캡처를 버린다(§3).
6. 감사: `<data_dir>/audit.jsonl` 에 `tool:"ticket"`, `decision:"resolved"|"rejected"|"expired"` 와 세션·`tool_call_id`·시각이 남는다(값 제외). 누가/어느 기기/어느 origin 은 현재 감사 레코드에 없다.

## 7. 사이트별 현실 (확인 필요 사항 포함)
- **Apple ID(iCloud) 로그인**: 신뢰 기기 승인 + 6자리 코드 + 자동화 탐지가 엄격. 기본 정책은 "Apple 로그인은 항상 `takeover`/`device_approval`, 세션 저장 후 재사용". 우회 시도 안 함. 세션 쿠키 만료 주기는 `unverified`.
- **한국 본인확인(통신사 PASS/문자 인증)**: 앱 인증(PASS)은 `device_approval`, 문자 인증은 `secret_input`. 휴대폰 번호 입력까지는 에이전트가 하되 번호는 Vault 필드로.
- **세션 재사용**: 한 번 사람이 통과한 로그인 상태는 `storageState` 로 저장해(Vault 정책 하에) 다음 task 에서 벽을 다시 만나지 않게 하는 게 가장 효과가 큰 완화책이다. 현재 구현은 없고 후속 과제다.

## 8. 구현 현황
| 항목 | 상태 |
| --- | --- |
| `Ticket` 스키마·이벤트·영속, 만료·재시작 복구 | 구현. 재시작 후 pending 티켓 유지, 해결 시 정확히 1회 재개, 기한이 지난 티켓은 복구 시 만료 처리 |
| 데몬 엔드포인트와 `request_human_step`/`ask_user` 툴 | 구현. pending → 데몬 재시작 → resolve → 재개 E2E |
| `secret_input` | 구현. canary 코드가 이벤트·DB·감사·런타임에 0건, origin 불일치 거부 |
| `device_approval` | 구현. 2초 폴링 자동 재개 |
| `takeover` | 구현. 프레임 WebSocket·입력 전달·에이전트 쓰기 락·종료 후 캡처 폐기. 웹 UI 의 takeover 화면(`TakeoverView`, 읽기 전용 PiP 포함)은 `openside.bearer.<token>` 서브프로토콜로 붙는다(E2E `crates/openside-e2e/tests/web_ui.rs::web_ui_secret_input_and_takeover_screen_resume_the_agent`) |
| 열린 티켓이 참조하는 탭의 브라우저 | 데몬이 종료할 때 pending 티켓의 탭이 있으면 Chromium 을 종료하지 않고 분리(detach)하고, 기동 시 `browser-state.json`(데이터 디렉터리, 관리 중인 `browser-profile` 프로필만 허용)로 다시 붙어 탭 소유권·노드 핸들·비밀 문서 표시를 복원한다. 붙을 수 없으면 상태 파일을 지운다. pending 티켓도 에이전트 소유 탭도 없으면 재부착하지 않고 브라우저를 종료한다. 재시작 후 브라우저 정책의 세부는 사용자 결정 대기 |
| Web Push 알림, `otp-ingest` | 구현. `crates/opensided/src/tests/remote.rs`(`push_reaches_unfocused_subscriptions_encrypted_with_deep_links_and_drops_gone_ones`, `otp_ingest_checks_scope_and_sender_fills_matching_secret_input_and_expires_codes`), E2E `crates/openside-e2e/tests/remote.rs` |
| 서브에이전트가 만든 티켓 | 프로토콜 필드만(`origin.subagent_id`, `tool_call.subagent`), 미사용(BLOCKED-1) |

테스트(`crates/opensided/src/tests*`): `restart_keeps_pending_tickets_and_resolution_resumes_once_hs1`, `restart_reconciles_inflight_once_and_expires_overdue_tickets`, `pending_secret_survives_daemon_restart_and_dead_endpoint_relaunches`, `secret_input_canary_never_reaches_events_database_audit_or_runtime`, `secret_input_without_sink_is_501_and_origin_mismatch_stays_pending`, `ticket_lifecycle_and_scope_by_kind_hs7`, `expiry_emits_event_and_delivers_external_expired_result`, `takeover_without_tab_returns_needs_desktop_and_done_can_finish_it`, `takeover::takeover_streams_frames_forwards_input_locks_agent_and_resumes_with_fresh_page_hs6`, `takeover::takeover_stream_ends_on_ticket_expiry_and_on_token_revocation`, `takeover::takeover_publication_waits_for_the_agent_operation_already_running`, `takeover::queued_input_never_lands_after_done_reject_or_revocation`, `takeover::a_tab_with_secret_fill_history_never_streams_frames_even_after_navigation`, `takeover::a_controller_that_stops_reading_is_dropped_and_the_ticket_can_be_taken_again`, `takeover::revocation_returns_only_after_admitted_input_and_nothing_runs_after_it`, `takeover::one_takeover_opener_ending_keeps_another_openers_reservation`, `takeover::rejected_input_flood_cannot_hold_the_controller_past_its_idle_limit`, `pending_tickets_hold_queue_and_stop_rejects_them`, `browser_tools_sensitive_expiry_vault_secret_device_and_readonly`. E2E: `crates/openside-e2e/tests/takeover.rs`(HS-4, HS-6). 브라우저: `crates/openside-browser/tests/screencast.rs`. takeover·HS-4 테스트 말고는 이 문서를 고치면서 실행하지 않았다(`unverified`).

## 9. 수용 기준(acceptance.csv 에 추가할 항목)
- HS-1 세션 재시작 후에도 pending 티켓 보존, 해결 시 정확히 1회 재개.
- HS-2 `secret_input` 값이 어떤 이벤트/로그/transcript/스냅샷/스크린샷에도 0건(canary).
- HS-3 origin 불일치 시 secret_input 거부.
- HS-4 `device_approval` 조건 충족 시 사람 개입 없이 자동 재개.
- HS-5 만료 시 에이전트에 통지, 무한 대기 없음.
- HS-6 takeover 중 에이전트 쓰기 정지, 종료 후 재개.
- HS-7 read 토큰은 티켓 값 제출 불가(403).

## 10. 런타임 결정: Pydantic AI (확정)
이 설계의 티켓 모델은 Pydantic AI 의 보류 도구(deferred tools)와 같은 구조라서 에이전트 런타임을 Pydantic AI 사이드카로 확정했다(2026-10-11, `docs/08`). 스파이크 S-8(steer와 보류 경계), S-9(harness 성숙도), S-15(보류 도구 재개 검증)와 `docs/specs/sidecar-protocol.md` 가 근거다. 요지:
- 보류된 tool call 이 히스토리에 미해결로 남고 같은 call id 로 결과를 채워 재개한다. 모델이 "pending 결과 + 나중에 온 사용자 메시지" 를 이어서 이해해야 하는 프롬프트 의존이 없다.
- 대기 중에는 런이 끝나고 상태는 직렬화된 대화(`state_snapshot`)와 데몬의 티켓으로 남는다. 그래서 사이드카가 재시작돼도 `pending_deferred` 로 복구한다.
- omp `follow_up` 재개 방식(pending 반환 + 재개 메시지 템플릿)과 S-7 전환 스파이크는 omp 런타임 제거로 필요 없어졌다.
- 서브에이전트가 만든 티켓의 소유권·재개는 아직 어댑터가 없다(BLOCKED-1, `docs/08` §C).
