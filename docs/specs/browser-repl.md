# browser_repl — Playwright 문법 REPL

세션이 소유한 에이전트 탭에서 Playwright 문법 JavaScript를 실행한다. 엔진 선택 근거는 `docs/spikes/S-12-js-engine.md`와 ADR-021에 있다.

## 진입점

| 경로 | 형태 |
|---|---|
| 에이전트 도구 | `browser_repl` — 인자 `{code, tab_id?, approvals?}`. 브라우저가 있을 때만 노출된다. 승인 등급은 `write`다. |
| API | `POST /v1/sessions/{id}/repl` — 요청 `ReplRequest{code, tab_id?, approval_ticket?}`, 응답 `ReplResponse{tool_call_id, status, result?, ticket_id?, reason?, hint?}`. `status`는 `done`, `deferred`, `denied`, `failed` 중 하나다. 원격 토큰에는 `browse` 범위가 필요하다. `steer`와 `admin` 범위도 이를 포함한다. |
| CLI | `openside repl <session-id> [--tab ID] [--eval CODE \| --interactive]` |

세 경로 모두 같은 도구 호출(`Host::call`)을 거친다. 정책, 티켓, 감사 로그, 인자 마스킹이 에이전트 호출과 같다. 기록되는 인자는 `{tab_id, code_length, approvals}`뿐이다.

탭은 다음 순서로 고른다.
1. `tab_id`를 주면 그 탭을 쓴다. 다른 세션의 탭이면 거부한다.
2. 생략했고 세션이 소유한 에이전트 탭이 하나뿐이면 그 탭을 쓴다.
3. 소유한 탭이 없으면 `about:blank` 탭을 새로 연다.
4. 소유한 탭이 여러 개면 `tab_id`를 요구한다.

`done`일 때 `result`의 형태는 `{return_value, console, error?, tab_id}`다.
- `return_value`: 마지막 식 문장의 값이다. 직렬화한 크기가 16 KiB를 넘으면 `{truncated, preview, bytes}`로 바뀐다.
- `console`: `console.log/info/warn/error/debug` 출력이다. 줄마다 32바이트를 더해 셈하며, 합이 8 KiB를 넘으면 그 뒤 줄은 버리고 잘림 안내 한 줄을 붙인다. 빈 줄도 이 한도에 들어간다.
- `error`: 스크립트가 던진 예외다(`TimeoutError`, `NotSupported`, `Error: strict mode violation` 등). 이 경우에도 그 전까지 실행한 결과는 남는다.

## 실행 모델

- 엔진은 QuickJS(`rquickjs`)다. 호출마다 새 컨텍스트를 만든다. 호출 사이에 전역 변수는 이어지지 않고, 탭 상태(URL, 입력값)만 이어진다.
- 한도: 전체 30초, 동작별 기본 대기 5초(`timeout` 옵션으로 바꿀 수 있다), 힙 32 MiB, 스택 512 KiB. 한도를 넘거나 호출이 취소되면 인터럽트 핸들러가 스크립트를 멈춘다. 프라미스 작업을 끝없이 만드는 루프와 역추적이 폭발하는 정규식도 이 한도에서 멈춘다.
- 호스트 쪽 한도: 페이지 DOM과 접근성 트리는 노드 20만 개, 32 MiB까지만 읽는다. 바이트는 태그 이름·속성 이름과 값·노드 값·텍스트(조상마다 따로 센 합)를 복사하기 전에 센다. DOM, 접근성 트리, `page.content()`, `page.snapshot()`을 요청하기 전에 `Performance.getMetrics`의 노드 수부터 확인하므로, 노드가 너무 많은 페이지는 응답을 받기 전에 거절한다. 동작 결과는 4 MiB까지만 스크립트에 넘긴다(`page.content()` 포함). 어느 한도든 넘으면 그 동작은 `page is too large for the REPL host budget`으로 실패한다. DOM을 읽는 중에도 정지·취소를 1024 노드마다 확인한다. 반환값은 전부 직렬화한 뒤에 크기를 잰다.
- 최상위 `await`를 쓸 수 있다.
- 쓸 수 있는 전역은 `page`, `expect`, `console`뿐이다. 파일·네트워크·프로세스 API는 없다.
- 페이지 조작은 모두 `openside_browser::PAGE_COMMANDS`에 있는 CDP 명령으로만 한다. DOM, Accessibility, Input, Page 탐색, Performance 지표, 포커스 에뮬레이션이 여기에 속한다. 페이지에 스크립트를 주입하지 않는다(`Runtime.*` 없음). 탐색 검사에는 `Fetch` 가로채기를 쓴다(아래 참고).
- 로케이터는 Rust에서 푼다. CSS는 `DOM.querySelectorAll`로, 역할과 접근 가능한 이름은 접근성 트리로 처리한다. 엄격 모드와 자동 대기(연결됨, 보임, 활성, 편집 가능)는 Playwright와 같다.
- 클릭과 키 입력은 `Input.dispatch*`로 보내는 실제 입력 이벤트다.
- 한 실행 안의 동작은 하나씩 차례로 처리한다. `Promise.all`로 여러 동작을 동시에 걸어도 CDP 명령이 섞이지 않는다.

## 스냅샷·ref 공유

`page.snapshot()`은 `browser_snapshot`과 같은 텍스트를 돌려준다. `page.locator('aria-ref=eN')`는 같은 탭의 마지막 스냅샷 ref를 가리킨다. 에이전트가 `browser_snapshot`으로 받은 `eN`도 REPL에서 그대로 쓸 수 있고, 반대 방향도 된다.

## 정책과 승인

### 검사 지점

| 대상 | 검사 |
|---|---|
| 최상위 프레임 문서 요청 | 실행하는 동안 탭에 `Fetch` 가로채기를 켜 둔다. `goto`, `reload`, 뒤로·앞으로뿐 아니라 링크 클릭, 폼 제출, 페이지 스크립트, 리다이렉트가 만든 문서 요청도 서버로 나가기 전에 `Browse` 판정을 받는다. 거부되면 요청을 `Aborted`로 끊으므로 오류 페이지도 남지 않고, 스크립트는 그 자리에서 멈춘다. |
| 페이지 읽기와 조작 | 동작마다 현재 문서의 출처로 `Browse` 판정을 받는다. 자동 대기로 다시 찾을 때도 매번 다시 판정한다. 읽은 값을 돌려주기 전에 문서(`loaderId`)가 그대로인지 확인하고, 바뀌었으면 값을 버린다. `page.url()`과 `waitForLoadState`도 판정을 받는다. |
| 클릭·호버·체크 | 대상 상자가 40 ms 간격으로 두 번 같아야 하고, 그 중심점에서 최상위 프레임이 돌려주는 요소(`DOM.getNodeForLocation`)가 대상 자신이나 그 자손이어야 한다. 덮개 요소나 iframe이 위에 있으면 이벤트를 보내지 않고 기다리다가 시간 초과로 실패한다. 그 뒤 마우스 이벤트(이동, 누르기, 떼기) 하나를 보내기 직전마다 문서가 그대로인지, 그 점의 요소가 여전히 대상이나 그 자손인지 다시 확인하고, 대상의 현재 역할과 이름으로 판정한다. 하나라도 어긋나면 남은 이벤트를 보내지 않고 실패한다. `clickCount`는 1~3만 받고, 범위를 벗어나면 이벤트를 하나도 보내지 않고 실패한다. |
| 키 입력 | 키 이벤트(누름, 뗌) 하나를 보내기 직전마다 그때 포커스를 가진 노드를 다시 찾고, 그 노드의 현재 역할과 이름으로 판정한다. 한 동작 안에서 이미 판정을 통과한 노드가 역할과 이름도 그대로면 다시 승인을 요구하지 않는다. 페이지가 입력 도중 포커스를 옮기거나 이름을 바꾸면 다음 키는 새 상태로 판정한다. 포커스가 최상위 프레임 밖(iframe 등)에 있으면 키를 보내지 않고 실패한다. |
| 판정할 노드 | 대상의 접근성 노드를 브라우저가 돌려주지 않으면 역할과 이름을 알 수 없으므로 입력을 보내지 않고 실패한다. 노드는 있지만 이름이 비어 있으면 이름 없는 대상으로 판정한다. |
| Enter | 폼 입력란에서 누르면 그 폼의 제출로 판정한다. 이름은 폼의 제출 버튼 이름과 `aria-label`·`name`·`id`·`action` 속성을 합친 것이다. `type`·`pressSequentially`의 `\n`, `page.keyboard`도 같다. |
| `selectOption` | 키보드로 지나가는 모든 선택지를 하나씩 `select option` 동작으로 판정한다. 선택지 이름 앞에 select의 이름을 붙인다. 하나라도 걸리면 키를 하나도 보내지 않는다. |

`Sensitive` 판정은 `browser_act`와 같은 키워드 판별(`openside_policy::sensitive_label`)을 쓴다. 대상의 역할과 접근 가능한 이름에 결제·구매·삭제 같은 키워드가 있으면 해당한다. 키워드 목록은 `openside_policy::SENSITIVE_KEYWORDS`에 있다.

### 멈춤과 승인

- 판정이 거부면 그 단계 직전에 멈추고 `denied`(`policy_denied: …`)를 돌려준다.
- `Sensitive` 판정은 항상 실제 출처로 먼저 한다. 그 출처에 `sensitive` 거부 규칙이 있으면 `approvals`를 넘기거나 이미 승인된 티켓으로 다시 불러도 거부한다. 데몬도 `approvals`의 각 항목을 그 출처의 `Sensitive` 동작으로 판정하므로, 티켓을 쓰는 단계에서도 거부된다. 거부가 아닐 때에만 승인된 항목과 맞춰 본다.
- 판정이 승인 필요면 그 단계 직전에 멈추고 `denied`(`approval_required: <설명>`)를 돌려준다. `hint` 끝에 `"approvals": [...]` 값이 붙는다. 각 항목의 `target`은 `<동작> <역할> "<이름>" on <출처> in tab <탭>` 형태로, 동작·대상·출처·탭을 모두 묶는다.
- 멈춘 뒤에는 스크립트가 예외를 잡아도 이어서 실행되지 않는다. 이미 걸려 있던 다른 동작도 CDP 명령을 더 보내지 못한다.
- 같은 `code`·`tab_id`에 그 `approvals`를 더해 다시 부르면 승인 티켓이 열린다. 티켓의 인자에 `approvals`가 그대로 보이고, 승인은 코드·탭·`approvals` 전체에 묶인다. 인자가 하나라도 다르면 `approval_args_mismatch`로 거부한다.
- 승인되면 스니펫 전체를 처음부터 다시 실행한다. 앞 단계의 부작용도 다시 일어나므로 앞 단계는 반복해도 안전하게 써야 한다. 승인된 `sensitive` 항목 하나는 일치하는 동작 한 번에만 쓰인다. 같은 버튼을 두 번 누르려면 항목도 두 개여야 한다.
- 다시 실행하다 새 승인 지점에 닿으면, 이전 항목과 새 항목을 모두 담은 `approvals`로 다시 멈춘다.
- 대기 중인 승인 정보는 데몬 메모리에 두지 않는다. 호출 인자와 영속 티켓에 들어 있으므로 데몬이 재시작해도 그대로 이어진다.
- API는 이 재호출을 직접 해서 `deferred`와 `ticket_id`를 돌려준다. 티켓이 승인되면 `approval_ticket`을 붙여 다시 호출한다. 이때 `approvals`는 티켓에서 가져온다. 다시 실행하다 새 승인 지점에 닿으면 새 티켓으로 다시 `deferred`를 돌려준다. API로 연 티켓은 에이전트 런을 재개하지 않는다.
- CLI는 티켓 처리 명령을 출력하고, 티켓이 끝날 때까지 기다렸다가 다시 실행한다. `deferred`가 다시 오면 같은 과정을 되풀이한다.
- 비밀값을 채운 탭에서는 `repl_unavailable_after_secret_fill`로 거부한다.

### 한계

- 마우스·키 이벤트마다 직전에 다시 확인하지만, 확인과 그 이벤트 하나 사이는 원자적이지 않다. 확인 직후 페이지가 대상을 바꾸면 이벤트 하나는 바뀐 대상에 닿을 수 있다. 다만 그다음 이벤트는 보내지 않는다.
- `Fetch` 가로채기는 네트워크 요청이 생기는 문서 탐색만 본다. bfcache에서 되살린 문서와 `about:`·`data:` URL은 이 검사를 거치지 않는다. 되살린 문서도 다음 동작의 문서 판정은 받는다. (프리렌더는 프로필 설정으로 꺼 두므로 해당하지 않는다. 패스키 의식은 기록이 없는 전용 새 탭에서, 가드를 먼저 세우고 처음 탐색을 시작하므로 이 틈과 무관하다.)
- 새 탭으로 열리는 팝업과 스니펫이 끝난 뒤에 일어나는 탐색은 REPL 규칙이 아니라, 세션이 소유한 탭 전체에 걸리는 브라우저 탐색 정책(세션의 `Browse` 판정)이 막는다. REPL이 실행되는 동안에는 그 정책 위에 REPL 규칙이 더해진다.
- 승인 항목은 동작·역할·이름·출처·탭에 묶이지만 문서(`loaderId`)에는 묶이지 않는다. 다시 실행할 때 같은 탭, 같은 출처에서 역할과 이름이 같은 대상을 만나면 문서가 바뀌었어도 그 승인이 쓰인다.
- 키워드 판별은 휴리스틱이다. 키워드가 없는 이름의 위험한 버튼은 걸러내지 못한다.
- `browser_act`에는 REPL 규칙이 더해지지 않고 브라우저 탐색 정책만 걸린다.
- 노드 수는 미리 확인하지만, 노드 수가 한도 안인 페이지(예: 아주 긴 속성 하나)의 CDP 응답 하나는 받은 뒤에야 크기를 잴 수 있다. 그 응답은 웹소켓 메시지 한도(별도 설정 없음, 라이브러리 기본값) 안에서 메모리에 올라온 다음 위 한도로 거절된다.

## 근사와 미지원

| 항목 | 동작 |
|---|---|
| `innerText`, 텍스트 매칭 | 요소 경계마다 공백을 넣는다. CDP DOM에는 공백뿐인 텍스트 노드가 없기 때문이다. |
| `textContent` | 원문 그대로다. 공백뿐인 텍스트 노드가 있으면 Playwright 결과와 달라진다(아래 표 32번). |
| `isEditable`, `toBeEditable` | 활성 상태와 `readonly`만 본다. 요소가 편집 가능한 종류인지는 보지 않는다. |
| `waitForLoadState` | `load`/`domcontentloaded`는 `Performance.getMetrics`의 DOMContentLoaded 지표로 판단한다. `networkidle`은 그 뒤 500 ms를 더 기다린다. |
| `selectOption` | 포커스 후 Home과 ArrowDown 키로 고른다. |
| 범위 | 최상위 프레임만 다룬다. iframe과 shadow DOM 내부는 다루지 않는다. |
| 미지원 API | `evaluate*`, `$eval`, `$$eval`, `addInitScript`, `addScriptTag`, `addStyleTag`, `exposeFunction/Binding`, `route`, `request`, `context`, `screenshot`, `pdf`, `setContent`, `dispatchEvent`, `setInputFiles`, `$`, `$$`, `elementHandle(s)`, `frameLocator`, `mouse`, `waitForFunction/Response/Request/Event`, `on`, `once`, `innerHTML`, `expect.poll`, `expect.soft`, URL 함수 조건(`waitForURL(fn)`, `toHaveURL(fn)`), XPath, `getByRole({includeHidden})` |

미지원 API를 부르면 `NotSupported: <API> is not supported by the openside REPL (<이유>)`를 던진다. 목록에 없는 멤버도 `undefined`를 돌려주지 않고 같은 오류를 던진다.

## 호환성 스위트

`crates/openside-repl/tests/compat.rs`가 로컬 고정 페이지(`tests/fixtures/`)와 실제 Chromium으로 아래 표를 계산한다. 이 표와 다르면 테스트가 실패한다. 스위트를 도는 동안 기록한 CDP 명령에 페이지 스크립트 명령(`Runtime.*`, `Page.addScriptToEvaluateOnNewDocument` 등)이 하나라도 있어도 실패한다. 통과율은 아래 49개 케이스 안에서의 비율일 뿐이고, Playwright API 전체의 호환 정도를 뜻하지 않는다.

정책 검사는 `crates/openside-repl/tests/policy.rs`(다른 출처로 가는 링크·스크립트·리다이렉트 탐색, 덮개 요소, Enter 제출, 민감한 선택지, 입력 중 포커스 이동, 이벤트 사이에 바뀐 대상 이름, iframe 포커스, 동시 동작과 정지, 콘솔·실행 한도, 호스트 크기 한도)와 `crates/opensided/src/tests/repl.rs`(동작 단위 승인, 인자 변경 거부, 탭 묶음, 두 번의 승인 지점, 출처의 `sensitive` 거부 규칙과 승인, 비밀값 탭 거부)가 확인한다.

<!-- compat:begin -->
| # | 케이스 | Playwright API | 결과 |
|---|---|---|---|
| 1 | `goto-title` | page.goto · page.title | pass |
| 2 | `url` | page.url | pass |
| 3 | `role-heading` | getByRole(name) · textContent | pass |
| 4 | `text-substring` | getByText · count | pass |
| 5 | `testid-innertext` | getByTestId · innerText | pass |
| 6 | `click-to-have-text` | click · expect.toHaveText | pass |
| 7 | `dblclick` | dblclick | pass |
| 8 | `hover` | hover | pass |
| 9 | `visibility` | isVisible (display/visibility) | pass |
| 10 | `auto-wait` | expect.toBeVisible auto-wait | pass |
| 11 | `reload-wait-for-selector` | page.reload · waitForSelector · last | pass |
| 12 | `alt-title` | getByAltText · getByTitle | pass |
| 13 | `press-chord` | locator.press(Shift+A) | pass |
| 14 | `keyboard-type` | focus · page.keyboard.type · inputValue | pass |
| 15 | `wait-for-hidden` | locator.waitFor({state:'hidden'}) | pass |
| 16 | `link-navigation` | click link · expect(page).toHaveURL | pass |
| 17 | `label-fill` | getByLabel · fill · inputValue | pass |
| 18 | `placeholder-to-have-value` | getByPlaceholder · expect.toHaveValue | pass |
| 19 | `fill-replaces` | fill (replace) | pass |
| 20 | `clear` | clear | pass |
| 21 | `press-sequentially` | pressSequentially (textarea) | pass |
| 22 | `check` | check · isChecked | pass |
| 23 | `uncheck-not-checked` | uncheck · expect.not.toBeChecked | pass |
| 24 | `radio` | getByRole(radio) · check · toBeChecked | pass |
| 25 | `select-option` | selectOption · inputValue | pass |
| 26 | `enabled-editable` | toBeDisabled · toBeEnabled · isEditable | pass |
| 27 | `attributes` | getAttribute · expect.toHaveAttribute | pass |
| 28 | `enter-submits` | press(Enter) · waitForURL(glob) | pass |
| 29 | `history` | goBack · goForward | pass |
| 30 | `count-nth-last` | count · nth · last · chained locator | pass |
| 31 | `filter-has-text` | filter({hasText}) · toHaveCount · allInnerTexts | pass |
| 32 | `text-content-whitespace` | allTextContents (whitespace-only text nodes) | fail |
| 33 | `filter-has` | filter({has}) · getByRole(disabled) | pass |
| 34 | `filter-has-not-text` | filter({hasNotText}) | pass |
| 35 | `strict-mode` | strict mode violation | pass |
| 36 | `action-timeout` | click({timeout}) TimeoutError | pass |
| 37 | `chain-text-engine` | 'ul >> text=Apple' | pass |
| 38 | `role-selector` | 'role=button[name="Delete"]' | pass |
| 39 | `css-has-text` | 'li:has-text("Cherry")' | pass |
| 40 | `columnheader` | getByRole(columnheader) · allInnerTexts | pass |
| 41 | `contain-text` | expect.toContainText · not.toHaveText | pass |
| 42 | `aria-ref` | page.snapshot ref → 'aria-ref=eN' | pass |
| 43 | `generic-expect` | expect(value).toBe/toEqual/toContain | pass |
| 44 | `completion-value` | top-level await · waitForTimeout | pass |
| 45 | `content` | page.content | pass |
| 46 | `load-state-title` | waitForLoadState · expect(page).toHaveTitle(regex) | pass |
| 47 | `page-shorthands` | page.fill · page.inputValue · page.click | pass |
| 48 | `role-regex-name` | getByRole({name: RegExp}) | pass |
| 49 | `locator-all` | locator.all · first | pass |

통과율: 48/49 (98.0%)
<!-- compat:end -->

32번은 알려진 차이다. CDP DOM 도메인이 공백뿐인 텍스트 노드를 보내지 않아서, `textContent`가 `"Apple Delete"` 대신 `"AppleDelete"`를 돌려준다.
