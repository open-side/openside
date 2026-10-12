# 09. 툴 설계: Rust 데몬이 실행하는 툴과 Pydantic AI 사이드카 프록시

작성: 2026-10-11, 구현 반영. 근거 코드: `crates/opensided/src/tools/{human,files,shell,browser,vault}.rs`, `crates/opensided/src/{host,tickets}.rs`, `crates/openside-policy/src/lib.rs`, `agent/src/openside_agent/tools/__init__.py`. 프로토콜은 `docs/specs/sidecar-protocol.md`, 사람 단계(티켓)는 `docs/specs/human-step.md`, 하네스 결정은 `docs/08`.
표기: 검증 = 코드·테스트로 확인, `unverified` = 확인하지 못함.

## 1. 구조
- **툴은 데몬만 실행하고 노출 게이트로 고정한다**(ADR-017). 사이드카의 Pydantic AI `Tool` 은 데몬으로 가는 얇은 프록시다. 입력 JSON Schema 의 원본은 데몬 툴의 `schemars` 출력(`ToolSpec.parameters`)이며, 사이드카는 `Tool.from_schema` 로 그대로 등록하고 Python 쪽 검증은 하지 않는다. 인자 검증은 데몬이 한다(`deny_unknown_fields`, 위반 시 `Arguments do not match the tool schema`).
- `ToolSpec = {name, description, parameters, approval: read|write|exec|sensitive, deferrable}`. 사이드카가 보는 툴은 `open.tools` 로 받은 명세뿐이다. 사이드카 자체의 파일·셸·브라우저 툴은 없고, harness capability 도 툴을 노출하는 것은 연결하지 않았다(`docs/08` §E). 사이드카는 모델 요청 직전에 선언된 함수 툴 이름이 기대 집합과 다르면 `ToolGateError`, 데몬은 `ready.tools` 가 `open.tools` 와 다르면 세션 시작을 거부한다(`tool_gate`).
- 시크릿(Vault 값, `secret_input` 값), 정책 판정, 감사, redaction 은 Python 프로세스를 지나지 않는다. 사이드카가 받는 것은 redaction 이 끝난 `for_model` 뿐이다.
- 호출 결과(`tool_result.outcome`)는 사이드카에서 이렇게 바뀐다: `done` → 툴 반환값, `deferred/approval` → `ApprovalRequired(metadata={ticket_id})`, `deferred/external` → `CallDeferred(metadata={ticket_id})`, `denied`/`error` → `ModelRetry`(이유+대안 문장, 기본 3회). 재개는 같은 `tool_call_id` 로 `DeferredToolResults`(`docs/specs/sidecar-protocol.md` §6~7).

## 2. 호출 파이프라인 (`host.rs`)
1. 툴 이름 조회: 모르는 툴 → `denied` `Unknown tool`. 이 세션에서 노출되지 않은 툴(모드·조건, §4) → `denied` `Tool is not exposed in this session`.
2. 인자 JSON 파싱, `browser_*`/`vault_*` 는 브라우저 gate 를 잡아 정책 판정과 실행을 직렬화한다.
3. 툴의 `actions()` 가 인자를 검증하고 정책 대상 `Action{capability, target}` 목록을 만든다(경로 canonicalize, 탭 소유권, origin 조회). 실패는 `denied` + `Correct the arguments using the tool schema`.
4. 정책: 각 action 을 `Policy::decide` 하고 가장 강한 효과(`Allow < Ask < Deny`)를 쓴다. `Deny` → `denied` `Policy denied this action; approval cannot override Deny`.
5. 승인 참조(`tool_call.approval_ticket`)가 있으면 그 티켓이 같은 세션·같은 `tool_call_id`·같은 툴 이름의 승인 완료(`approved:true`, `confirm`, approval deferral) 상태이고, 인자와 정책 action 의 정규화 해시가 티켓을 열 때의 값과 같을 때만 **한 번** 통과한다(`claim_approval`). 다른 인자는 `denied` `approval_args_mismatch`, 이미 쓴 승인은 `approval_already_used`, 그 밖에는 `Approval ticket does not authorize this tool call`. 참조가 없는데 효과가 `Ask` 면 `confirm` 티켓(approval 보류, redaction 된 인자 표시)을 열고 `deferred` 로 반환한다.
6. 통과하면 툴 `call`. `Done{for_model, for_app}` 는 redaction 후 `for_model` 에 §5 의 32 KiB 제한을 적용한다. `Defer` 는 티켓(external 보류)을 연다. `Denied`/`Failed` 의 이유·메시지도 redaction 한다.
7. 모든 호출은 `tool.started`(redaction 된 인자) → `tool.ended`(결과, `is_error`) 이벤트와 감사 JSONL 한 줄(§5)을 남긴다.

## 3. 툴 표
tier 는 `ToolSpec.approval`, 정책 capability 는 `actions()` 가 만드는 `Action` 이다. 인자의 `?` 는 선택값이다.

| 툴 | 인자 | tier / deferrable | 정책 capability (target) | 노출 조건 |
| --- | --- | --- | --- | --- |
| `request_human_step` | `kind`(`confirm`\|`secret_input`\|`device_approval`\|`info`\|`takeover`), `summary`, `instructions?`, `tab_id?`, `origin?`, `field_ref?`, `resume_when?`, `options?[{value,label}]` | read / true | 없음(항상 Allow; 대신 인자 검증, §6) | 항상 |
| `ask_user` | `question`, `options?[string]` | read / true | 없음 | 항상 |
| `read` | `path`, `offset?`, `limit?` | read / false | `FsRead` (canonical 경로) | 항상 |
| `write` | `path`, `content` | write / false | `FsWrite` (canonical 경로, 새 파일은 부모가 있어야 함) | Guard·Full |
| `edit` | `path`, `old_text`, `new_text` | write / false | `FsWrite` (canonical 경로) | Guard·Full |
| `bash` | `command`, `cwd?`, `timeout_ms?` | exec / false | `Exec` (canonical cwd, 기본값은 세션 workspace) | Guard·Full, unix, 그리고 `sandbox.required=false` 이거나 bwrap 이 있을 때(§7) |
| `browser_open_tab` | `url` | read / false | `Browse` (URL origin) | 브라우저 사용 가능 |
| `browser_tabs` | 없음 | read / false | 없음 | 브라우저 사용 가능 |
| `browser_snapshot` | `tab_id`, `since?`, `snapshot_id?`, `ref?`, `from_line?`, `from_char?` | read / false | `Browse` (탭 현재 origin) | 브라우저 사용 가능 |
| `browser_navigate` | `tab_id`, `url` | read / false | `Browse` (이동할 URL origin; 탭 소유권 확인) | 브라우저 사용 가능 |
| `browser_act` | `tab_id`, `target`(`eN`), `action`(`click`\|`type`\|`press`\|`select`\|`hover`\|`focus`), `text?`, `submit?`, `key?`, `values?` | write / true | `Browse` (탭 현재 origin) + 키워드 일치 시 `Sensitive` (§4) | Guard·Full, 브라우저 사용 가능 |
| `browser_pointer` | `tab_id`, `action`(`click`\|`double_click`\|`type`\|`scroll`), `x`, `y`(1440x900 뷰포트 CSS 픽셀), `text?`, `delta_x?`, `delta_y?` | write / true | `Browse` (탭 현재 origin, 좌표에 걸린 요소의 frame origin 이 다르면 그 origin 도) + 그 요소·조상의 이름/role 이 키워드에 걸리거나 들여다볼 수 없는 cross-process iframe 이면 그 요소의 origin 에 `Sensitive` (§4) | Guard·Full, 브라우저 사용 가능 |
| `browser_screenshot` | `tab_id` | read / false | `Browse` (탭 현재 origin) | 브라우저 사용 가능 |
| `browser_repl` | `code`, `tab_id?`, `approvals?` | write / true | 스니펫 안에서 단계별로 판정한다. 최상위 문서 요청(링크·리다이렉트 포함)과 읽기·조작마다 `Browse`, 실제로 입력을 받는 대상이 키워드에 맞으면 `Sensitive`. 승인 항목은 동작 하나에 한 번 쓰인다(`docs/specs/browser-repl.md`) | 브라우저 사용 가능 |
| `browser_close_tab` | `tab_id` | read / false | `Browse` (탭 현재 origin) | 브라우저 사용 가능 |
| `vault_login` | `tab_id`, `site`, `username_ref`, `password_ref`, `submit_ref?` | write / true | `VaultUse` (탭 현재 origin) + `submit_ref` 가 키워드 일치 시 `Sensitive` | Guard·Full, 브라우저 사용 가능 + Vault 잠금 해제됨 |
| `vault_otp` | `tab_id`, `site`, `field_ref` | write / false | `VaultUse` (탭 현재 origin) | Guard·Full, 브라우저 사용 가능 + Vault 잠금 해제됨 |
| `vault_passkey` | `tab_id`, `site`, `action`(`register`\|`login`), `submit_ref` | write / false | `VaultUse` (탭 현재 origin) + `submit_ref` 가 키워드 일치 시 `Sensitive` | Guard·Full, 브라우저 사용 가능 + Vault 잠금 해제됨 |

- 위 표의 툴이 데몬 `Registry` 의 브라우저·파일·휴먼 툴 전부다. 사이드카에는 노출 조건을 통과한 것만 `open.tools` 로 보낸다.
- "브라우저 사용 가능" = 브라우저 실행 파일이 발견됐고(`browser.executable` → `OPENSIDE_BROWSER` → `browser.bundle_dir` 의 `chrome`/`chromium` → PATH 의 `chromium`·`google-chrome` 계열 순) 브라우저 컨트롤러가 멈춘 상태가 아님.
- "Vault 잠금 해제됨" = 데몬 시작 시 또는 이후 `openside vault unlock`/`POST /v1/vault/unlock` 호출을 통해 데몬이 `[vault].path`(기본값 `<data_dir>/vault.db`)의 볼트를 `[vault].unlock` 방식으로 잠금 해제한 상태. 기본값인 `auto` 환경에서는 키링 볼트가 데몬 시작 시 키링을 통해 바로 열린다. passphrase 볼트는 데몬 시작 시 `[vault].passphrase_cmd`(argv 형태, 셸 미사용, 출력 ≤ 64 KiB, 제한 시간 20초)가 설정되어 있어야만 자동으로 열린다. 볼트 파일이 없거나 잠겨 있는 동안에는 볼트 툴이 숨김 처리되며, 사유는 `GET /v1/vault`에서 확인할 수 있다. 볼트 감사 로그는 데이터 디렉터리 내 `vault-audit.jsonl`에 기록된다. 포맷, 정책 및 API 규격은 [`docs/specs/vault.md`](specs/vault.md)를 참고한다.
- Vault 가 거절하면 `vault_*` 는 `denied` 를 돌려준다. 이유는 `vault_locked`, `vault_unknown_site`, `vault_policy_never`, `vault_platform_locked`, `vault_origin_mismatch`, `vault_rp_mismatch`, `vault_no_passkey` 중 하나다.
- `vault_passkey` 는 호출 하나로 WebAuthn 의식 전체를 처리한다. 탭에 CDP 가상 인증기(ctap2, resident key, UV)를 붙이고, `login` 이면 Vault 의 개인 키를 넣은 뒤 `submit_ref` 를 누르고, 자격 증명이 생기거나(`register`) 서명 카운터가 오를 때(`login`)까지 최대 15초 기다린 다음 인증기를 지운다. 결과는 `{"status":"registered"|"signed_in"|"failed"}` 뿐이다. WebAuthn CDP 명령은 `browser_repl` 에서 쓸 수 없다.
- `deferrable` 은 명세 필드다. 정책 `Ask` 는 이 값과 무관하게 모든 툴에 적용된다(`host.rs`).
- 브라우저 툴의 `eN` 은 탭 수명 동안 단조 증가하는 ref 이고, 내비게이션 뒤의 옛 ref 는 `StaleReference` 로 거부된다. 탭은 세션 소유이며(`opened_by=agent` + 소유 세션 일치) 다른 세션·사용자 탭에는 접근할 수 없다. 탭 포커스를 가져오는 `Target.activateTarget`/`bringToFront` 는 쓰지 않는다.
- 사용자 탭은 읽기 전용이다(B-7 ①). 에이전트는 자기 세션이 연 탭만 닫고 이동하고 조작할 수 있다. 사용자 탭과 다른 세션의 탭에 대한 `browser_snapshot`·`browser_navigate`·`browser_act`·`browser_screenshot`·`browser_close_tab`·`browser_repl` 은 `denied` 다. `browser_tabs` 에는 자기 세션의 탭만 나온다.
- 잠든 탭(discarded/unloaded)에는 attach 하지 않는다(B-7 ③). 컨트롤러는 CDP 호출 전에 `BrowserError::TabAsleep` 을 돌려주고, 도구는 `denied` reason `tab_asleep: …` 을 돌려준다. attach 전체 제한 시간은 12초다. `browser_tabs` 의 `asleep` 이 `true` 인 탭은 다른 탭을 쓰거나 새 탭을 연다. 패치 브라우저에서는 2초 주기 점검이 gateway 의 탭 상태(`asleep`/`live`)를 장부에 옮긴다. 스톡 Chromium 은 CDP 로 discard 상태를 알려주지 않으므로 장부 표시만 쓴다.
- `browser_tabs` 항목의 `group` 은 패치 브라우저 gateway 가 알려주는 탭 그룹 제목이다(B-7 ④). 에이전트가 만든 탭은 MCP·CLI 의 `POST /v1/sessions/{id}/repl` 로 연 탭을 포함해 모두 `"Agent Tabs"` 그룹에 들어간다. 스톡 Chromium 은 CDP 로 탭 그룹을 볼 수 없으므로 `group` 이 `null` 이고, 세션별 소유권 장부가 그룹 역할을 한다.
- 브라우저가 강제 종료되면(PP-23 ②) 데몬은 다음 브라우저 호출이나 2초 주기 점검에서 같은 관리 프로필로 Chromium 을 다시 띄운다. 죽은 탭의 소유 세션에는 `tab.closed` 이벤트와, 실행 중이면 steer 알림이 간다. 그 탭 ID 를 쓰는 호출(`browser_repl` 포함)은 한 번 `browser_restarted: …` 로 거부되고, 그 뒤로는 일반 소유권 거부가 된다. 다시 띄운 브라우저가 30초 안에 또 죽으면 1초부터 두 배씩, 최대 30초까지 기다렸다가 다시 띄우며, 그동안의 호출은 `browser_restarting: …` 으로 거부된다.
- incognito 세션(B-10)은 공유 프로필 대신 세션 전용 Chromium 을 쓴다. 프로필은 `<runtime_dir>/incognito-browsers/<session>/profile`(0700)이고 장부는 저장하지 않는다. 세션 삭제, 데몬 종료, 크래시 뒤 다음 데몬 시작 때 프로세스를 끝내고 디렉터리를 지운다. 시작 시 정리는 incognito 루트 바로 아래의 실제 디렉터리만 다루고, `endpoint.json` 이 그 디렉터리의 `profile` 을 가리킬 때만 브라우저를 끈다. 심볼릭 링크는 따라가지 않고 링크만 지운다. `vault_*` 호출과 `secret_input` 사람 단계는 정책 판정 전에 `denied` reason `vault_incognito` 로 거부된다. 팝업·다운로드·탭 닫힘은 공유 브라우저와 같이 steer 알림으로 전달되고, device approval 사람 단계는 incognito 탭에도 걸린다. incognito 브라우저가 죽으면 탭은 `tab.closed`·`browser_restarted` 로 알리고, 다음 호출은 빈 새 프로필로 시작한다.
- 아직 없는 것: 지연 노출(`tool_search`), `web_fetch`, `memory_*`/`routine_*`, 서브에이전트(BLOCKED-1). 상시 노출 툴 정의의 토큰 측정(구 S-13)은 시작하지 않았다. REPL 엔진 선택(S-12)은 `docs/spikes/S-12-js-engine.md`, ADR-021에 있다.
- `skill`(읽기 등급, 항상 노출, 인자 `{name, resource?}`)은 사용자 라이브러리와 작업공간 `.openside/skills/` 의 스킬 지침을 읽는다. 세션을 열 때 활성 스킬 목록이 시스템 프롬프트에 들어가고, 메시지의 `/이름` 은 사이드카가 이 도구로 미리 불러온다. [`docs/specs/skills.md`](specs/skills.md).

## 4. 정책과 노출
정책 엔진(`openside-policy`)은 `(Policy, Action) → Decision` 순수 함수다. 현재 데몬은 규칙 layer 없이 **모드 기본값**만 넣는다(`approved_roots` = 세션 workspace).

| capability | `read-only` | `guard`(기본) | `full` |
| --- | --- | --- | --- |
| `FsRead` | Allow | workspace 안 Allow, 밖 Ask | Allow |
| `FsWrite`, `Exec` | Deny | workspace 안 Allow, 밖 Ask | Allow |
| `Browse`, `Fetch` | Allow | Allow | Allow |
| `VaultUse` | Deny | Allow | Allow |
| `Sensitive` | Deny | Ask | Ask |

- `Sensitive` 는 자동 허용되지 않는다(규칙으로는 `Deny` 로만 조일 수 있다). `Deny` 는 승인으로도 뒤집히지 않는다. 설정 키는 `[policy] mode = "read-only" | "guard" | "full"`.
- **노출은 모드로도 줄인다.** `read-only` 에서는 tier 가 `read` 가 아닌 툴(`write`, `edit`, `bash`, `browser_act`, `vault_login`, `vault_otp`, `vault_passkey`)을 `open.tools` 에서 뺀다. 숨은 툴을 부르면 §2-1 의 `denied`. E2E: `read_only_gate_hides_mutations_and_unknown_write_retries_without_touching_file`.
- **`Sensitive` 키워드 휴리스틱.** `browser_act` 는 대상 노드의 접근성 이름(name)과 role 에, `vault_login` 은 `submit_ref` 노드에 대해 아래 키워드가 부분 문자열(ASCII 대소문자 무시)로 있으면 `Sensitive` action 을 추가한다: `pay`, `buy`, `purchase`, `checkout`, `place order`, `submit payment`, `결제`, `구매`, `송금`, `delete`, `삭제`. 의도 판별이 아니라 보수적 키워드 일치이므로 오탐도 확인을 요구하고, **일치하지 않는다는 사실이 안전을 뜻하지 않는다.** 일치하면 `guard`/`full` 은 `Ask`(`confirm` 티켓, 만료 시 원 호출이 거부됨), `read-only` 는 `Deny` 다. E2E: `sensitive_payment_ask_expires_and_denies_original_call_without_clicking`.
- 승인된 `browser_act` 도 실행 직전에 탭의 현재 origin 이 승인된 `Browse` 대상과 같은지 다시 확인하고, 다르면 `Page origin changed; retry after a snapshot` 로 거부한다. `vault_*` 도 `VaultUse` 대상 origin 과 현재 origin 이 같아야 한다(`Page origin changed; take a fresh snapshot`).
- **좌표 입력(B-4).** `browser_pointer` 는 스냅샷 ref 로 가리킬 수 없는 대상(캔버스로 그린 버튼 등)에 쓴다. 좌표는 `browser_screenshot` 과 같은 1440x900 뷰포트 기준이고, 뷰포트 밖이면 거부한다. 정책을 묻기 전에 좌표를 hit-test 한다(`DOM.getNodeForLocation`, `ignorePointerEventsNone:false` 라서 `pointer-events:none` 덮개가 아니라 실제로 입력을 받을 요소가 잡힌다 → 접근성 트리). 그 요소와 조상 최대 6단계의 role·이름에 위 `Sensitive` 키워드 규칙을 적용한다. 좌표가 iframe 위인데 그 frame 의 origin 을 알 수 없으면 부모 origin 으로 대신하지 않고 거부한다. 페이지 스크립트는 입력 이벤트 사이에도 돌기 때문에, 실행할 때는 mouseMoved 뒤의 모든 press·release·wheel·텍스트 입력 직전에 hit-test 를 다시 한다. 문서·요소·origin·라벨 중 하나라도 승인 때와 다르면 거기서 멈추고 `the page changed under that point; take a new screenshot` 로 거부한다. `type` 은 클릭 뒤 포커스가 hit-test 한 그 요소에 있을 때만 텍스트를 넣는다. Enter·submit 은 없다. 암묵적 폼 제출은 hit-test 가 보지 못한 제출 버튼에 닿기 때문이고, 제출은 `browser_act` 로 한다. 입력한 텍스트는 이벤트·감사에 `text_length` 로만 남는다(모델 대화 기록에는 툴 호출 인자로 남는다). 모델에는 `{url, navigated, hit:{role,name}}` 를 준다. 테스트: `crates/opensided/src/tests/browser_snap.rs::browser_pointer_clicks_canvas_from_screenshot_and_gates_sensitive_targets`, `…::browser_pointer_rechecks_every_event_ignores_covers_and_types_only_into_the_hit_element`.
- **브라우저 이벤트 steer(B-2 ②).** 에이전트 탭에서 팝업이 열리거나, 다운로드가 시작·종료되거나, 탭이 닫히면(페이지가 닫았든 사용자가 닫았든) 그 탭을 가진 세션에 `[browser event] …` 문장을 steer 로 넣는다. 실행 중일 때만 넣으므로 다음 모델 요청 직전에 사용자 메시지로 들어가고, 쉬는 세션을 깨우지는 않는다(이벤트 자체는 SSE 에 그대로 남는다). 문장에는 탭 id, 다운로드 guid, origin 만 들어가고 경로·파일 이름·대화상자 문구는 넣지 않는다. 페이지가 정한 문자열이 사용자 역할 메시지로 모델에 들어가는 것을 막기 위해서다. E2E: `crates/openside-e2e/tests/browser_events.rs::popup_download_and_closed_tab_reach_the_running_agent_as_steer`.

## 5. 출력, redaction, 감사
- **모델용 vs 앱용.** `ToolExec::Done{for_model, for_app}`: 모델(사이드카)에 가는 값은 `for_model` 뿐이다. `for_app` 가 있으면 `tool.ended` 이벤트의 `result` 가 `for_model` 대신 `for_app` 이 된다. 지금 `for_app` 을 쓰는 툴은 `browser_screenshot` 하나다(모델: `{"saved":true,"width":W,"height":H}`, 앱: `{"path": <PNG 경로>}`; PNG 는 데이터 디렉터리의 `artifacts/<session>/<tool_call_id>.png`, 디렉터리 0700, 파일 0600·새로 만들기·심볼릭 링크 거부).
- **출력 제한 32 KiB.** `for_model` JSON 직렬화가 `OUTPUT_LIMIT = 32 KiB` 를 넘으면 `{"truncated":true,"preview":"…","how_to_get_more":"Output truncated. Use read offset/limit or narrow the tool request to retrieve more."}` 로 바뀐다(이스케이프된 JSON 바이트 기준으로 자름). 툴 자체 제한도 있다: `read` 는 기본 200줄·최대 2000줄에 페이지당 약 32 KiB 를 넘기면 `more:true` 와 `next_offset` 을 주고, `bash` 는 stdout·stderr 를 각각 32 KiB 까지만 담고 `[Output truncated; narrow the command to retrieve more.]` 를 붙인다.
- **스냅샷 id, diff, 경고(B-3).** 캡처마다 `snapshot_id` 가 붙는다. 온전한 결과는 첫 줄이 `[snapshot_id <id>]` 인 문자열이다. `since: <id>` 를 주면, 그 id 가 같은 세션·같은 탭·같은 문서이고 호출자에게 트리 전체가 한 번에 전달된 캡처(또는 그런 캡처 위의 diff)일 때만 줄 단위 diff(`[snapshot_id <새 id> since <id>] Diff since …` 다음에 `@@ -s,n +s,n @@` 와 `-`/`+` 줄, 문맥 줄 없음, 이 형식 전용 좌표)를 준다. 그 밖에는 언제나 전체 트리다. 쪽 하나만 받은 캡처, `ref` 로 일부만 받은 캡처, 다른 세션의 캡처, 이동 뒤의 캡처는 diff 기준이 될 수 없다. 그래서 호출자가 받지 않은 기준으로 diff 를 만드는 일은 없다. ref 는 바뀌지 않으므로 diff 를 기준 텍스트에 적용하면 새 전체 트리와 같다. 스냅샷은 자르지 않는다. 직렬화한 결과 전체(경고 포함, 이스케이프 기준)가 30 KiB 를 넘으면 앞부분만 담고 `{"snapshot_id", "snapshot", "warning": {"code":"snapshot_over_budget", "message", "shown_lines", "total_lines", "omitted_lines", "omitted_refs", "next_from_line", "next_from_char", "scopes":[{ref,role,name,lines}]}}` 를 준다. `scopes` 의 이름은 80자에서 자르고 `…` 를 붙인다. 나머지는 같은 `snapshot_id` 에 `from_line` 이나 `ref` 를 주어 읽는다. 이때 새로 캡처하지 않고 그 캡처를 그대로 읽으므로, 그 사이 페이지가 바뀌어도 쪽이 섞이지 않는다. 문서가 바뀌었거나 캡처가 만료되었으면(전체 64개를 넘으면 오래된 것부터 지움, 데몬 재시작 시 모두 지움) 거부한다. 한 줄이 혼자 예산을 넘으면 그 줄을 글자(유니코드 스칼라) 단위로 나눠 보여 주고, `next_from_line` 과 `next_from_char` 를 그대로 `from_line`·`from_char` 로 보내면 같은 캡처에서 이어지는 조각을 준다. 조각을 이어 붙이면 원래 줄과 정확히 같다. 대화 요약(compaction)으로 모델 컨텍스트에서 이전 메시지가 빠지면 사이드카가 `context_compacted` 를 보내고, 데몬은 그 대화(부모와 하위 에이전트 각각)의 epoch 를 올린다. 이전 epoch 에서 받은 캡처나 그 위의 diff 를 `since` 로 보내면 diff 대신 전체 트리를 준다. epoch 는 메모리에만 있어 데몬을 다시 시작하면 캡처와 함께 사라진다. 테스트: `crates/opensided/src/tests/browser_snap.rs`, `crates/opensided/src/tools/browser_snap.rs`, 벤치 `bench/snapshot/README.md`.
- **redaction.** 툴 인자는 이벤트·감사에 쓰기 전에 툴별 `redact_args` 로 허용 필드만 남긴다: `write.content`·`edit.old_text`/`new_text`·`bash.command` 는 `[REDACTED]`, `browser_act.text` 는 `text_length`(길이만), 브라우저 URL 은 사용자정보·쿼리·프래그먼트를 제거(`public_url`), `request_human_step` 은 스키마 필드만, `ask_user` 는 `question`/`options` 만, `vault_*` 는 ref·사이트·탭만. 그 위에 레지스트리 redactor(Vault 값, 사람이 넣은 `secret_input` 값, OTP 코드)가 `for_model`·`for_app`·거부 이유·실패 메시지·이벤트·티켓·감사 레코드의 모든 문자열과 JSON 키에 적용된다. 파일 내용·셸 출력도 `for_model` 이므로 redactor 는 거치지만, 모델이 읽을 수 있는 경로의 비밀을 분류해 막는 기능은 아니다(`unverified`).
- **감사 JSONL.** 데이터 디렉터리의 `audit.jsonl`, 권한 0600(심볼릭 링크 거부, 배타 잠금, 쓸 때마다 fsync). 한 줄은 메타데이터만 담는다: `{"ts","session","tool_call_id","tool","decision","duration_ms"}`. `decision` 은 `allowed`/`deferred`/`denied`/`failed`. 티켓 해결·거부·만료는 `tool:"ticket"`, `decision:"resolved"|"rejected"|"expired"` 로 같은 파일에 남는다. 인자·출력 내용은 기록하지 않는다. 테스트: `file_tools_validate_edit_and_redact_content_in_events_and_audit`, `secret_input_canary_never_reaches_events_database_audit_or_runtime`.

## 6. 티켓(사람 단계) 종류와 TTL
정책 `Ask` 는 `confirm`(approval 보류, 원 호출을 같은 `tool_call_id` 로 다시 실행), `ask_user` 는 `info`, `request_human_step` 은 인자의 `kind` 로 external 보류 티켓을 만든다(`docs/specs/human-step.md`). 만료는 데몬이 1초 간격으로 처리한다.

| kind | 만들어지는 경로 | 사람이 하는 일 | 모델이 받는 값 | 기본 TTL (`[tickets]` 키) |
| --- | --- | --- | --- | --- |
| `confirm` | 정책 `Ask`(`approval` 보류) | 승인/거부 | 승인 시 원 호출이 실행됨. 거부·만료는 `ToolDenied(reason)` | 600초 (`confirm_ttl_secs`) |
| `secret_input` | `request_human_step` | 값 입력(API `resolve`) | `{"status":"filled"}` — 값은 모델·이벤트·DB·감사에 가지 않고 데몬이 지정 입력란에 직접 넣는다 | 300초 (`secret_input_ttl_secs`) |
| `device_approval` | `request_human_step` | 자기 기기에서 승인 후 `done` 또는 자동 재개 | `{"status":"done"}` | 1800초 (`device_approval_ttl_secs`) |
| `info` | `ask_user`, `request_human_step` | 텍스트/선택지 답 | `{"answer": "…"}` | 86400초 (`info_ttl_secs`) |
| `takeover` | `request_human_step`(`tab_id` 필요, 없으면 데스크톱 처리) | 원격 화면(`GET /v1/tickets/{id}/takeover` WebSocket)에서 직접 조작 후 `done` | `{"status":"done","tab_id","page_changed":true,"url","title","note"}` | 1800초 (`takeover_ttl_secs`) |

- TTL 은 초 단위이고 최솟값 1, 환경변수 오버라이드는 없다. 만료된 external 티켓은 `{"status":"expired"}`, 거부는 `{"status":"rejected"}` 로 원 호출의 결과가 된다. 세션 stop 은 열린 티켓을 `rejected(reason:"stopped")` 로 닫는다.
- `secret_input` 은 `tab_id`, `origin`, `field_ref` 가 모두 있어야 하고 `origin` 은 자격증명·경로·쿼리·프래그먼트가 없는 HTTP(S) origin 이어야 한다. 티켓을 열 때 ref 를 탭의 노드 핸들로 고정하고(탭 소유권·origin 일치 검사), 값을 넣을 때 현재 origin 이 다시 일치해야 한다(불일치 `origin_mismatch`). 같은 대상에 대한 요청은 3회로 제한한다. 값을 채운 문서는 비밀 문서로 기록된다.
- `device_approval` 은 `resume_when` 이 필수이고 자동 판정(`url_not_contains`\|`selector_visible`\|`text_visible`)이면 `tab_id` 도 필요하다. `{manual:true}` 는 `done` 호출만 기다린다. 데몬은 2초 간격으로 탭을 보고 조건이 충족되면 사람 없이 재개한다.
- `takeover` 는 탭의 JPEG 프레임(최대 10fps)을 WebSocket 으로 보내고 마우스·휠·키·텍스트 입력을 그 탭에만 전달한다. 티켓이 pending 인 동안 그 탭을 대상으로 하는 `browser_*`·`vault_*` 호출(읽기 포함, `tab_id` 없는 `browser_repl` 포함)은 실행되지 않고 `denied` — `takeover_active` 다. 여는 호출은 그 탭에서 실행 중인 에이전트 작업이 끝난 뒤 게시되고, 끝날 때는 사람 입력을 먼저 멈춘 뒤 탭을 돌려준다. 그 뒤 이전 `snapshot_id` 는 `since`/페이징에 쓸 수 없고 이전 `eN` ref 는 stale 이다. 비밀을 채운 적이 있는 탭은 프레임을 보내지 않는다. 프로토콜·거부 코드·순서는 `docs/specs/human-step.md` §3.
- **스크린샷 거부.** `vault_login`/`vault_otp`/`secret_input` 으로 비밀을 채운 문서에서는 `browser_screenshot` 이 `denied` — `screenshot_unavailable_after_secret_fill`(hint `navigate or take a snapshot instead`). 내비게이션해서 새 문서가 되면 다시 가능하다. 데몬 재시작 뒤에는 같은 비밀 문서의 `browser_snapshot` 도 `snapshot_unavailable_after_secret_fill` 로 거부한다(재시작으로 redaction 필터가 사라지기 때문). `secret_input` 재시도 시 이전 입력 결과를 알 수 없으면 `409 secret_input_indeterminate`.

## 7. `bash` 와 샌드박스
- 실행: unix 에서만 노출. `bash`(없으면 `sh`)를 `-c` 로 실행하고 환경은 비운 뒤 `PATH=/usr/local/bin:/usr/bin:/bin`, `HOME` 만 둔다. stdin 은 닫고 별도 프로세스 그룹으로 실행하며 호출이 끝나면 자식도 정리한다(`kill_on_drop`). `timeout_ms` 기본 30000, 허용 범위 1..120000(벗어나면 `error` 로 `timeout_ms must be between 1 and 120000`). 결과 `for_model` = `{"stdout","stderr","exit_code"}`.
- **bwrap 이 있을 때**(`sandbox.bwrap` 설정 또는 PATH 의 `bwrap`): `bwrap --die-with-parent --new-session --unshare-all --clearenv`, 존재하는 `/usr /bin /sbin /lib /lib64 /etc` 는 읽기 전용 bind, `/proc`·`/dev`·`/tmp`(tmpfs), 세션 workspace 만 읽기/쓰기 bind. `--unshare-all` 이므로 네트워크도 격리된다. `[network] allow` 는 정보용이며 격리를 풀지 않는다(`--share-net` 으로 바꾸지 않음). 샌드박스 bash 는 workspace 가 설정돼 있어야 한다. 래퍼가 샌드박스 설정 직후 준비 마커를 출력하고, 마커가 없거나 bwrap 이 기동에 실패하면 `denied` — `Bubblewrap execution failed; unsandboxed fallback is forbidden`(hint `openside doctor`). 샌드박스 없이 다시 실행하지 않는다.
- **bwrap 이 없을 때**: `sandbox.required = false`(기본)면 `bash` 는 노출되지만 정책 전용 격리로 돈다. 세션을 열 때 stderr 와 `runtime.error{code:"sandbox_unavailable"}`(비치명) 로 `Bubblewrap is unavailable; shell tools use policy-only isolation. Install bubblewrap or set sandbox.required=true.` 를 알린다. `sandbox.required = true` 면 bwrap 이 없을 때 `bash` 를 **숨긴다**(`open.tools` 에서 빠짐).

## 8. Pydantic AI 개념과 openside 의 대응
| Pydantic AI 개념 | openside 에서의 형태 | 상태 |
| --- | --- | --- |
| Function tools + 스키마 | 데몬 `schemars` 스키마를 `Tool.from_schema` 로 등록 | 구현 |
| `ModelRetry` | `denied`/`error` outcome 의 이유·대안 문장 | 구현 |
| Deferred tools(`CallDeferred`, `ApprovalRequired`, `DeferredToolResults`) | 티켓. 같은 call id 로 재개 | 구현 (live: `ask_user` 왕복) |
| `HandleDeferredToolCalls` | 쓰지 않음. 정책 Allow/Ask 는 데몬 파이프라인(§2)이, TOTP 는 `vault_otp` 가 직접 처리 | 해당 없음 |
| Toolsets / `prepare_tools` | 데몬이 모드·조건(§3~4)으로 노출 목록을 정해 `open.tools` 로 전달 | 구현 |
| `ToolReturn`(모델용/앱용) | `for_model` / `for_app` | 구현 (§5) |
| AskUser | 데몬 `ask_user` 툴(`info` 티켓). harness `AskUser` 는 쓰지 않음 | 구현 |
| Guardrails, Warn on cache busts | `PromptInjectionDefender`, `WarnOnCacheBusts` 연결(`docs/08` §E) | 구현 |
| Durable execution | 데몬 이벤트 소싱 + `state_snapshot`(런 종료 지점) | 구현 |
| Workspaces / Bubblewrap | 데몬 `bash` 가 직접 bwrap 실행(§7) | 구현 |
| Tool Search, Code Mode | — | 미구현 |
| Evals / Logfire | — | 미구현 |
