# browser_repl — Playwright 문법 REPL

세션이 소유한 에이전트 탭에서 Playwright 문법 JavaScript를 실행한다. 엔진 선택 근거는 `docs/spikes/S-12-js-engine.md`와 ADR-021에 있다.

**페이지 스크립트(ADR-022).** 페이지 내 JavaScript 실행을 허용하며(ADR-022), 관련 구현을 갖추고 있다. 호출자는 아래 "페이지 스크립트" 절에 정리된 `page.evaluate` 등의 API로 페이지 안에서 직접 코드를 실행할 수 있다. 다만 REPL 자체 코드(프렐류드 및 실행 대상 스크립트)는 여전히 QuickJS 안에서만 동작한다.

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
- `error`: 스크립트가 던진 예외다(`TimeoutError`, `NotSupported`, `Error: strict mode violation` 등). 이 경우에도 그 전까지 실행한 결과는 남는다. 엄격 모드를 위반하면 일치하는 요소를 최대 10개까지 태그, 식별 속성, 텍스트가 담긴 짧은 HTML 형태의 미리보기로 나열하며, 요소가 프레임 안에 있으면 프레임 이름도 함께 표시한다.

## 실행 모델

- 엔진은 QuickJS(`rquickjs`)다. 호출마다 새 컨텍스트를 만든다. 호출 사이에 전역 변수는 이어지지 않고, 탭 상태(URL, 입력값)만 이어진다.
- 한도: 전체 30초, 동작별 기본 대기 5초(`timeout` 옵션으로 바꿀 수 있다), 힙 32 MiB, 스택 512 KiB. 한도를 넘거나 호출이 취소되면 인터럽트 핸들러가 스크립트를 멈춘다. 프라미스 작업을 끝없이 만드는 루프와 역추적이 폭발하는 정규식도 이 한도에서 멈춘다.
- 호스트 쪽 한도: 페이지 DOM과 접근성 트리는 노드 20만 개, 32 MiB까지만 읽는다. 바이트는 태그 이름·속성 이름과 값·노드 값·텍스트(조상마다 따로 센 합)를 복사하기 전에 센다. DOM, 접근성 트리, `page.content()`, `page.snapshot()`을 요청하기 전에 `Performance.getMetrics`의 노드 수부터 확인하므로, 노드가 너무 많은 페이지는 응답을 받기 전에 거절한다. 동작 결과는 4 MiB까지만 스크립트에 넘긴다(`page.content()` 포함). 어느 한도든 넘으면 그 동작은 `page is too large for the REPL host budget`으로 실패한다. DOM을 읽는 중에도 정지·취소를 1024 노드마다 확인한다. 반환값은 전부 직렬화한 뒤에 크기를 잰다.
- 최상위 `await`를 쓸 수 있다.
- 전역 객체로는 `page`, `expect`, `console`만 제공한다. QuickJS 자체에는 파일·네트워크·프로세스 관련 API가 없다. 반면 페이지 스크립트는 브라우저 페이지 내부에서 실행되므로, 페이지 출처의 `fetch` 등 웹 페이지가 쓸 수 있는 기능을 그대로 사용할 수 있다.
- 드라이버 동작(로케이터, 액션, 대기)은 `openside_browser::PAGE_COMMANDS`에 정의된 CDP 명령(DOM, Accessibility, Input, Page, Runtime, Performance 메트릭, 포커스 에뮬레이션)으로 전달한다. 또한 페이지 안에서 직접 JavaScript를 실행하는 편이 더 단순하고 안정적인 경우에는 드라이버 내부에서 스크립트를 실행하기도 한다. 내비게이션 검사에는 아래에서 설명하는 `Fetch` 가로채기를 사용한다.
- 로케이터는 Rust에서 푼다. CSS는 `DOM.querySelectorAll`로, 역할과 접근 가능한 이름은 접근성 트리로 처리한다. CSS·텍스트·역할 로케이터는 열린 shadow root를 뚫고 들어가며(각 shadow 트리를 따로 탐색한다), 닫힌 shadow root 안으로는 들어가지 않는다. XPath(`xpath=` 접두사 또는 `/`나 `..`로 시작하는 셀렉터)는 페이지 스크립트가 덮어쓸 수 없도록 격리된 컨텍스트(isolated world) 안에서 현재 범위 요소를 기준 노드로 삼아 평가한다. 페이지 로케이터는 최상위 문서만 탐색하며, 프레임 로케이터를 거쳐야만 프레임 안으로 들어간다. 엄격 모드와 자동 대기(연결됨, 보임, 활성, 편집 가능)는 Playwright와 같다.
- 클릭과 키 입력은 `Input.dispatch*`로 보내는 실제 입력 이벤트다.
- 한 실행 안의 동작은 하나씩 차례로 처리한다. `Promise.all`로 여러 동작을 동시에 걸어도 CDP 명령이 섞이지 않는다.

내부 JavaScript 기본 연산은 `openside_repl::INTERNAL_PRIMITIVES` 단일 카탈로그에서 중앙 관리하며, 데몬 역시 이 카탈로그에 정의된 설정 정보 스크리닝 소스를 가져다 사용한다. 기본 단계 프로토콜 감사(basic-step protocol audit)는 카탈로그에 등록된 정확한 소스 코드, 호출 메서드, 지정된 격리 월드(isolated world), 그리고 `returnByValue` 및 `awaitPromise` 계약이 일치하는 호출만 허용한다. 임의의 호출자 JavaScript나 메인 월드 실행, 초기화 스크립트는 이 감사에서 계속 금지된다. ADR-022에 따른 호출자 스크립트 API는 별도로 테스트하며, 내비게이션 로드 추적은 스크립트 없이 생명주기(lifecycle) 이벤트만으로 처리한다.

| 연산 | 상수 식별자 | 격리 월드 | CDP 메서드 | 반환 및 비동기 계약 |
| --- | --- | --- | --- | --- |
| 설정 정보 스크리닝 | `PROTECT_SETUP_SOURCES` | `openside-fill` | `Runtime.evaluate` | `returnByValue=true` (값 반환) |
| 스크롤 계획 | `PLAN` | `openside-scroll` | `Runtime.callFunctionOn` | `returnByValue=true` (값 반환) |
| 클릭 안정성 샘플링 | `STABLE_BOX` | `openside-pointer` | `Runtime.callFunctionOn` | `returnByValue=true`, `awaitPromise=true` |
| 입력 후 값 확인 | `READ_BACK` | `openside-fill` | `Runtime.callFunctionOn` | `returnByValue=true` (값 반환) |
| XPath 탐색 | `XPATH` | `openside` | `Runtime.callFunctionOn` | `returnByValue=false` 명시적 예외 (엘리먼트 핸들 반환 후 드라이버가 객체 그룹 해제) |

## 스냅샷·ref 공유

`page.snapshot()`은 `browser_snapshot`과 같은 텍스트를 돌려준다. `page.locator('aria-ref=eN')`는 같은 탭의 마지막 스냅샷 ref를 가리킨다. 에이전트가 `browser_snapshot`으로 받은 `eN`도 REPL에서 그대로 쓸 수 있고, 반대 방향도 된다.

스냅샷에는 각 자식 프레임의 내용이 해당 iframe 노드 아래에 포함되며, 프레임 내부 ref도 `aria-ref=eN` 형태로 동작한다. `browser_snapshot`에는 최상위 문서와 출처가 같은 프레임만 들어가고, `page.snapshot()`에는 정책상 허용된 모든 프레임이 들어간다. 정책에서 허용하지 않은 프레임은 내용 없이 iframe 요소만 나타난다.

## 프레임

`page.frameLocator(selector)`, `locator.frameLocator(selector)`, `locator.contentFrame()`은 프레임 로케이터를 반환하며, `locator`, `getBy*`, `frameLocator`, `first`, `last`, `nth`, `owner()`는 Playwright와 똑같이 동작한다. `page.mainFrame()`, 그리고 `page.frame(name)` 또는 `page.frame({ name, url })`(url은 정확히 일치하는 문자열, glob 패턴, RegExp를 받고 조건 함수는 받지 않는다)은 동작을 실행할 때마다 대상을 다시 찾는 프레임 객체를 반환한다. `await page.frames()`는 최상위 프레임부터 모든 프레임 목록을 반환한다. Playwright와 달리 프로미스를 반환하므로 반드시 `await`해야 한다. 프레임 객체는 `name()`, `url()`, `parentFrame()`, `childFrames()`, `title()`, `content()`, `locator`/`getBy*`, `frameLocator`, `evaluate`와 셀렉터를 받는 동작 단축 메서드(`click(selector)` 등)를 지원한다.

사이트 간 iframe은 별도 렌더러 프로세스(OOPIF)에서 실행된다. REPL은 필요할 때 해당 프로세스에 연결해 전용 CDP 세션에서 노드를 읽고 히트 테스트를 수행하며, 클릭이나 키 입력은 여전히 최상위 뷰포트의 요소 위치에 실제 입력으로 보낸다. 어느 프레임 안에 있든 키보드 포커스를 지원한다.

## 페이지 스크립트 (ADR-022)

아래 API는 호출자의 코드를 페이지 안에서 실행한다. 모든 호출은 실제 페이지에 전달되기 전에 정책 검사를 거친다("검사 지점" 참고).

| API | 동작 |
|---|---|
| `page.evaluate(fn, arg?)` | 최상위 프레임의 메인 월드(페이지 자체 스크립트가 실행되는 공간)에서 코드를 실행한다(`Runtime.callFunctionOn`). 함수를 전달하면 `arg`를 인자로 넘겨 호출한다. 문자열을 넘기면 단일 표현식으로 평가하며, 평가 결과가 함수이면 `arg`를 넘겨 호출한다. 반환값이 프로미스이면 이행될 때까지 대기한다. |
| `page.evaluateHandle(fn, arg?)`, `locator.evaluateHandle(fn, arg?)` | `JSHandle`을 반환한다(`evaluate`, `evaluateHandle`, `jsonValue`, `getProperty`, `getProperties`, `asElement`, `dispose` 지원). 핸들은 `dispose()`를 호출하거나 대상 문서가 사라지거나 호출이 끝날 때까지 유효하며, 호출이 종료되면 해당 호출에서 만든 모든 핸들을 자동으로 해제한다. `click` 같은 엘리먼트 핸들 액션은 제공하지 않으므로 로케이터를 사용해야 한다. |
| `page.$eval(selector, fn, arg?)` | 일치하는 첫 번째 엘리먼트를 찾아 별도 대기 없이 실행한다. 일치하는 엘리먼트가 없으면 `failed to find element matching` 오류로 실패한다. |
| `page.$$eval(selector, fn, arg?)` | 일치하는 모든 엘리먼트의 배열을 대상으로 한 번 실행한다. 일치하는 엘리먼트가 없으면 빈 배열을 넘긴다. |
| `locator.evaluate(fn, arg?, { timeout })` | 일반 액션과 마찬가지로 엄격 모드(정확히 하나의 엘리먼트와 일치)로 동작하며, 엘리먼트가 문서에 붙을 때까지 기다린다. |
| `locator.evaluateAll(fn, arg?)` | 일치하는 모든 엘리먼트의 배열을 대상으로 실행한다. 엄격 모드를 적용하지 않으며 대기하지도 않는다. |
| `page.addInitScript(script, arg?)` | 함수(fn), 코드 문자열(string), 스크립트 본문(content) 전달을 지원하며, 파일 경로(path) 전달은 지원하지 않는다. 스크립트를 실제로 실행하기 전에 등록 시점에 허용한 오리진 집합과 대조하여 대상 오리진을 검증한다. 불투명 오리진과 등록 이후 새로 나타난 오리진은 실행하지 않고 건너뛴다. 등록된 스크립트는 해당 browser_repl 호출이 끝나면 자동으로 제거된다. |
| `page.cdp(method, params?)` | 호출 범위를 동일 탭의 페이지 세션 내부로만 엄격히 제한한다. 허용되지 않은 도메인을 요청하면 reserved by the host 오류를 반환하며 거부한다. CDP를 통해 변경된 원시 브라우저 상태는 호출 종료 시 기본적으로 되돌리지 않는다. 핸들 정리는 이번 호출의 객체 그룹(Runtime evaluate/call/run 및 DOM resolve 실행 시 강제 지정하는 objectGroup)에 할당된 핸들로 한정하며, 임의의 모든 원시 핸들이 무조건 해제되는 것은 아니다. 등록된 초기화 스크립트는 호출 종료 시 함께 정리된다. |

page.cdp는 소유한 탭의 페이지 세션에서만 사용할 수 있다. 허용 도메인은 DOM, CSS, Runtime, Page, Network, Emulation, Input, Accessibility, Overlay, Performance, Log, DOMSnapshot, DOMStorage, IndexedDB, CacheStorage, Animation, LayerTree, Profiler, HeapProfiler, Debugger로 엄격히 제한한다. Target, Browser, SystemInfo, Storage, Fetch, WebAuthn을 비롯한 나머지 도메인은 호스트가 전유하며 reserved by the host 오류로 거부한다. DOM.setFileInputFiles를 포함한 파일 업로드 작업은 반드시 Upload 게이트와 경로 검증을 거쳐야 한다. Page.setDownloadBehavior 호출과 브라우저 전역의 쿠키·캐시 수정은 모두 차단한다. 프레임, 실행 컨텍스트, 객체, 수신자 대상 작업은 명령을 받는 문서의 오리진 게이트를 통과해야 한다. 렌더러 전역의 불투명 식별자(opaque renderer-wide id)를 다룰 때는 해당 페이지 세션의 모든 하위 문서 승인을 거쳐야 한다. 초기화 스크립트는 등록 시점에 허용한 불투명하지 않은 오리진에서만 실행하며, 거부된 오리진이나 등록 이후 새로 나타난 오리진에서는 결코 실행하지 않는다. 등록된 스크립트는 해당 browser_repl 호출이 끝나는 즉시 제거한다.

구형 API인 Page.addScriptToEvaluateOnLoad와 runImmediately: true 옵션은 지원하지 않으며, 오리진 검증이 적용된 초기화 등록을 거친 이후의 새 문서에만 스크립트가 실행된다. 명시적 쿠키·리소스 URL과 스토리지 키는 현재 소유 탭의 프레임에 속한 대상이어야 하며, CacheStorage 캐시 ID는 이번 browser_repl 호출에서 승인된 requestCacheNames가 반환한 값만 사용할 수 있다. 파일(file:) URL은 호스트가 전유(reserved by the host)하며, 원시 드래그 페이로드에 포함된 파일 경로는 반드시 Upload 게이트를 통과해야 한다.

브라우저가 내부적으로 연속 발행하는 다중 합성 이벤트인 Input.synthesizeScrollGesture, Input.synthesizePinchGesture, Input.synthesizeTapGesture는 개별 이벤트 사이에 실제 수신 프레임의 오리진을 다시 검증할 수 없으므로 호출을 거부한다. 이러한 동작이 필요할 때는 개별 page.mouse 이벤트를 대신 사용한다.

명령 시간 초과나 취소가 발생하면 버려진 각 렌더러 세션의 상태를 점검한다. 응답하지 않는 렌더러 프로세스를 종료하는 것이 아니라 Runtime.terminateExecution을 통해 응답 없는 페이지 자바스크립트 실행만 중단하므로, 소유한 다른 탭은 계속 정상적으로 사용할 수 있다. 개별 정리 명령은 250ms, 전체 정리 과정은 3s 이내에 완료되도록 제한 시간을 엄격히 적용한다.

탭을 다음 작업(Task)에 넘기기 전에, 처리 대기 중인 초기화 스크립트 등록 응답을 모두 소진(drain)하고 모든 스크립트 제거 승인(ACK)을 제한 시간 안에 확인한다. 제한 시간 내에 제거를 확인하지 못하면, 해당 타깃의 종료를 요청하기 전에 먼저 소유 탭을 격리(quarantine)하여 추가 진입을 차단하고, 해당 타깃이 완전히 닫히거나 사라진 것이 확인될 때까지 정확한 타깃 종료 요청을 재시도한다. 어떤 경우에도 브라우저 전체나 무관한 다른 탭을 종료하지 않는다.

비어 있음 검사 및 API 필터인 has, hasNot을 포함하여 프레임 내부 요소를 조회하거나 필터링하기 전에, 접근 대상 문서의 오리진 진입 승인 여부를 먼저 검증한다. 휠 스크롤과 마우스 버튼을 누른 채 이동하는 포인터 동작(held-move)은 입력을 실제로 수신하는 프레임의 오리진 검증을 거치도록 허용하되, 민감 동작(Sensitive action) 검사는 추가하지 않는다.

- **값 교환:** 인자와 반환값은 태그가 붙은 JSON 형태로 전달된다. 따라서 `undefined`, `NaN`, `±Infinity`, `-0`, `BigInt`, `Date`, `RegExp`, `Error`뿐 아니라 상호 참조나 순환 참조도 그대로 유지된다. 함수와 심볼은 `undefined`로 변환된다. DOM 노드, 문서, `window`는 각각 `ref: <Node>`, `ref: <Document>`, `ref: <Window>` 문자열로 변환되어 돌아온다. `arg` 안에 `JSHandle`을 담아 전달할 수도 있다. 결과 데이터 크기는 4 MiB 작업 결과 제한과 16 KiB 반환값 제한을 함께 적용받는다.
- **오류 처리:** 페이지 안에서 예외가 발생하면 페이지의 스택 추적을 포함해 최대 2000자까지 잘린 `Error: <API>: <the page's error text>` 형식으로 반환한다.
- **실행 시간:** 페이지 스크립트 실행 시간도 REPL 호출의 전체 30초 제한 시간에 포함된다. 무한 루프 등으로 페이지 스크립트가 끝나지 않은 상태에서 호출 제한 시간에 도달하면, REPL은 탭을 정상 상태로 유지하기 위해 `Runtime.terminateExecution`으로 스크립트 실행을 강제 중단한다. 끝나지 않는 프로미스를 단순히 기다리기만 하는 스크립트는 중단하지 않고 그대로 두며, 그 결과값은 버린다.
- **문서 바인딩:** 함수를 실행하기 전에 REPL은 실행 대상 객체(전역 객체, 엘리먼트, 문서)가 정책 검사를 통과한 문서(`loaderId`)에 속해 있는지 확인한다. 핸들 역시 자신이 생성된 문서에 계속 묶여 있다.
- **Vault 연동:** 데몬이 시크릿을 입력한 탭에서는 계속해서 `browser_repl` 전체 호출을 거부하며(`repl_unavailable_after_secret_fill`), 도구 실행 결과도 여전히 Vault 마스킹을 거친다. 다만 페이지 스크립트는 페이지 내 데이터를 직접 읽을 수 있고, 변환한 값을 반환하는 방식으로 마스킹을 우회할 수 있다(ADR-022).

## 탐색·대기·네트워크

탐색·대기 API는 브라우저 크레이트가 탭마다 두는 수명 주기·네트워크 추적기 위에서 동작한다. 탭에서 `goto`, `reload`, `goBack`, `goForward`, `waitForLoadState`, `waitForURL`, `waitForResponse`, `waitForRequest` 중 하나를 처음 호출할 때 `Network.enable`과 `Page.setLifecycleEventsEnabled`를 켜고 추적을 시작하며, 이 과정에서 페이지 스크립트는 실행하지 않는다.

| API | 동작 |
|---|---|
| `page.goto(url, { waitUntil, timeout })`, `reload`, `goBack`, `goForward` | `waitUntil`에는 `commit`(새 문서가 이전 문서를 대체한 시점), `domcontentloaded`, `load`(기본값), `networkidle`(`load` 이후에 더해, `load`와 마지막 요청 활동 중 더 늦은 시점부터 500 ms 동안 진행 중인 요청이 없을 때)을 지정할 수 있다. 메인 문서의 `Response`(상태 코드, 헤더, URL, `text()`/`json()`)로 이행하며, 새로 읽은 문서가 없을 때(해시 조각만 바뀐 경우, bfcache에서 복원된 페이지, 같은 문서 안의 히스토리 이동)는 `null`을 돌려준다. HTTP 4xx·5xx 응답은 오류가 아니며, 본문이 비어 있더라도(이 경우 Chromium이 `net::ERR_HTTP_RESPONSE_CODE_FAILURE`를 보고하지만 드라이버는 정상 탐색으로 처리한다) `response.ok()`가 `false`인 `Response`로 이행한다. 리다이렉트가 일어나면 최종 응답의 `Response`를 돌려준다. `timeout` 기본값은 동작 기본 제한 시간인 5초다. |
| `page.waitForLoadState(state, { timeout })` | `state`는 `load`(기본값), `domcontentloaded`, `networkidle` 중 하나다. 현재 문서가 이미 해당 상태에 도달했으면 즉시 반환하고, 그 밖의 값을 넘기면 허용 값 목록을 담은 `Error`를 던진다. |
| `page.waitForURL(url, { waitUntil, timeout })` | `url`에는 정확한 문자열, 글로브(`**`, `*`, `?`, `{a,b}`), `RegExp`, 또는 읽기 전용 URL 뷰(`href`, `protocol`, `origin`, `host`, `hostname`, `port`, `pathname`, `search`, `hash`, `searchParams.get/getAll/has`)를 받는 함수를 넘긴다. 호스트가 탐색 기록을 보관하고 호출 이후에 일어난 탐색만 따지므로, 폴링 사이에 잠깐 나타났다가 다시 바뀐 URL(해시 조각 변경, `history.pushState`, 리다이렉트)도 놓치지 않고 일치로 판정한다. URL이 일치한 뒤에는 `waitUntil`(기본값 `load`, `commit`이면 즉시 반환)까지 기다린다. |
| `page.waitForResponse(matcher, { timeout })`, `page.waitForRequest(matcher, { timeout })` | `matcher`에는 URL 글로브·`RegExp`·문자열이나 `Response` 또는 `Request`를 받는 함수(`async` 가능)를 넘긴다. 호출 이후에 발생한 요청과 응답만 대상으로 삼으므로 `Promise.all([page.waitForResponse(..), page.click(..)])`에서 두 호출의 순서를 어떻게 두든 똑같이 동작한다. `Response`는 `url()`, `status()`, `statusText()`, `ok()`, `headers()`(소문자 헤더 이름), `headerValue(name)`, `request()`, `text()`, `json()`을 제공하고, `Request`는 `url()`, `method()`, `resourceType()`을 제공한다. `text()`와 `json()`은 `Network.getResponseBody`로 본문을 읽는다. 브라우저는 페이지가 살아 있는 동안에만 본문을 보관하며 리다이렉트 중간 응답의 본문은 남기지 않는다. 바이너리 본문은 `NotSupported`를 던지고, 2 MiB를 넘는 본문은 거부한다. 네트워크 로그는 탭마다 최근 512개 항목을 유지한다. |
| `page.waitForFunction(fn, arg?, { timeout, polling })` | 페이지 함수(페이지 스크립트, ADR-022 게이트 적용)가 참으로 평가되는(truthy) 값을 돌려줄 때까지 `polling` ms(기본값 50)마다 반복 실행하고, 그 값의 `JSHandle`로 이행한다. 기본 `timeout`은 5초다. |

### 오류 분류

- REPL이 던지는 모든 오류에는 `name`이 붙으며, `error.transient`는 `TimeoutError`와 `NetworkError`일 때만 `true`여서 스크립트가 재시도 여부를 판단할 수 있다. 탐색 실패 메시지는 끝에 `(transient: retrying can succeed)` 또는 `(permanent: retrying will not help)`가 붙는다.
- `TimeoutError`: 탐색, 로드 상태, URL, 응답, 함수, 로케이터 대기가 제한 시간을 넘겼을 때 발생한다. 메시지에 기다리던 대상을 적으며, 로드 상태 대기에서는 아직 끝나지 않은 요청을 최대 3개까지 함께 보여 준다(예: `1 request still in flight: GET /never`).
- `NetworkError`(일시적): 서버에 닿지 못했거나 연결이 끊겼을 때 발생한다(`ERR_CONNECTION_REFUSED`, `ERR_CONNECTION_RESET`/`CLOSED`, `ERR_EMPTY_RESPONSE`, 시간 초과, 네트워크 단절, 도달할 수 없는 주소).
- `NavigationError`(영구적): 호스트 이름을 찾지 못함, 리다이렉트 루프, 잘못된 URL이나 지원하지 않는 스킴, 차단된 포트, TLS 인증서·핸드셰이크 거부, openside 정책에 막혔거나 다른 탐색으로 대체된 요청(`ERR_ABORTED`), 다운로드 URL일 때 발생한다.
- `PolicyError`: 정책 거부 또는 승인 요청이다.
- 일반 `Error`: 알 수 없는 `waitUntil` 값처럼 잘못된 인자를 넘겼을 때 발생한다.

### 동시성

- 응답이 오지 않는 탐색이 브라우저 전체의 상호작용 락을 계속 쥐고 있지 않으므로, 다른 세션의 상호작용이 밀리지 않는다(락은 진행 중인 상호작용을 경계 짓고 참조를 무효화할 때만 잡으며, 탐색이 끝날 때까지 유지하지 않는다). 탐색 시간은 `timeout`으로 제한되며, 시간을 넘기면 로드를 취소(`Page.stopLoading`)하고 탭은 이전 페이지를 그대로 유지한다.

## 정책과 승인

### 검사 지점

| 대상 | 검사 |
|---|---|
| 최상위 프레임 문서 요청 | 실행하는 동안 탭에 `Fetch` 가로채기를 켜 둔다. `goto`, `reload`, 뒤로·앞으로뿐 아니라 링크 클릭, 폼 제출, 페이지 스크립트, 리다이렉트가 만든 문서 요청도 서버로 나가기 전에 `Browse` 판정을 받는다. 거부되면 요청을 `Aborted`로 끊으므로 오류 페이지도 남지 않고, 스크립트는 그 자리에서 멈춘다. |
| 페이지 읽기와 조작 | 동작마다 현재 문서의 출처로 `Browse` 판정을 받는다. 자동 대기로 다시 찾을 때도 매번 다시 판정한다. 결과가 자식 프레임 안에 있으면 해당 프레임의 출처도 함께 판정하며(`Gate::Page`에 프레임 전달), 둘 중 하나라도 거부되면 동작이 실패한다. 읽은 값을 돌려주기 전에 최상위 프레임과 결과가 나온 모든 프레임의 문서(`loaderId`)가 그대로인지 확인하고, 바뀌었으면 값을 버린다. `page.url()`과 `waitForLoadState`도 판정을 받는다. |
| 클릭·호버·체크 | 대상 요소의 전체 경계 사각형(bounding rectangle)이 네 차례 연속된 requestAnimationFrame 샘플 동안 변하지 않아야 하며, 중심점 히트 테스트(OOPIF 하위 탐색 포함) 결과가 대상 요소나 그 하위 요소에 도달해야 한다(요소가 가려져 있으면 제한 시간까지 대기한다). 각 마우스 이벤트(move, down, up)를 전송하기 직전마다 동일 문서 여부, 대상 히트 여부, 현재 역할(role)·이름(name), 수신 프레임의 오리진을 다시 검증한다. 최초 누르기(press) 전에 요소가 움직이면 제한 시간 안에서 접근 단계를 다시 시도하지만, 최초 누르기가 전달된 뒤에는 부분 클릭을 절대 재시도하지 않으며 검증에 실패하면 남은 이벤트를 즉시 취소한다. clickCount는 1..3 범위만 허용하며, 그 외의 값은 입력을 전송하기 전에 실패 처리한다. |
| 키 입력 | 키 이벤트(누름, 뗌) 하나를 보내기 직전마다 최상위 문서나 어느 프레임에서든 그때 포커스를 가진 노드를 다시 찾고, 그 노드의 현재 역할과 이름 및 프레임 안이라면 해당 프레임의 출처로 판정한다. 한 동작 안에서 이미 판정을 통과한 노드가 역할과 이름도 그대로면 다시 승인을 요구하지 않는다. 페이지가 입력 도중 포커스를 옮기거나 이름을 바꾸면 다음 키는 새 상태로 판정한다. `object`나 `embed`처럼 검사할 수 있는 문서가 없는 임베드 객체에 포커스가 있으면 키를 보내지 않고 실패한다. |
| 판정할 노드 | 대상의 접근성 노드를 브라우저가 돌려주지 않으면 역할과 이름을 알 수 없으므로 입력을 보내지 않고 실패한다. 노드는 있지만 이름이 비어 있으면 이름 없는 대상으로 판정한다. |
| Enter | 폼 입력란에서 누르면 그 폼의 제출로 판정한다. 이름은 폼의 제출 버튼 이름과 `aria-label`·`name`·`id`·`action` 속성을 합친 것이다. `type`·`pressSequentially`의 `\n`, `page.keyboard`도 같다. |
| `page.mouse`·키보드 한계 | 클릭할 때 넘긴 수식키는 키 이벤트 없이 마우스 이벤트 플래그로만 전달한다. 터치 입력, 클립보드 `paste`, IME 키 이벤트 자체를 위한 `Input.dispatchKeyEvent`는 지원하지 않으며(글자 조합은 `imeSetComposition`으로 처리), 호출이 끝나도 누르고 있던 마우스 버튼이나 키를 자동으로 떼지 않는다. |
| `selectOption` | 키보드로 지나가는 모든 선택지를 하나씩 `select option` 동작으로 판정한다. 선택지 이름 앞에 select의 이름을 붙인다. 하나라도 걸리면 키를 하나도 보내지 않는다. |
| 페이지 스크립트 | 페이지 스크립트 호출은 실제 페이지에 닿기 전에 `Gate::Script` 검사를 거친다. 최상위 프레임 출처를 기준으로 해당 API(`evaluate`, `evaluateHandle`, `addInitScript`, `cdp:<method>`)의 실행 가능 여부를 판단하며, 프레임 내부라면 해당 프레임 출처도 함께 확인한다. 두 출처 모두 `Browse`가 허용된 상태라면 데몬이 실행을 승인하며, 호출 단위 개별 승인은 거치지 않는다. 스크립트가 페이지 안에서 수행하는 세부 동작은 하나씩 검사하지 않는다. |

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
- `Sensitive` 키워드 검사와 승인 절차는 로케이터나 키보드 입력 같은 REPL 자체 동작에만 적용된다. 페이지 스크립트가 페이지 내부에서 직접 실행하는 클릭, 폼 제출, 값 입력 등은 검사 대상이 아니며, 이로 인해 발생하는 최상위 프레임 문서 요청만 `Fetch` 검사를 거친다.
- `browser_act`에는 REPL 규칙이 더해지지 않고 브라우저 탐색 정책만 걸린다.
- 노드 수는 미리 확인하지만, 노드 수가 한도 안인 페이지(예: 아주 긴 속성 하나)의 CDP 응답 하나는 받은 뒤에야 크기를 잴 수 있다. 그 응답은 웹소켓 메시지 한도(별도 설정 없음, 라이브러리 기본값) 안에서 메모리에 올라온 다음 위 한도로 거절된다.
- 네트워크 추적은 추적이 시작된 뒤에 출발한 요청만 알 수 있으므로, `networkidle`은 추적이 켜진 첫 탐색부터 정확하게 동작한다. 다른 프로세스에서 실행되는 iframe(OOPIF)은 첫 요청을 보내기 전에 붙잡아 요청 추적이 시작될 때까지 기다리므로 `networkidle`도 이들의 요청을 함께 기다리지만 워커의 요청은 아직 추적하지 않으며, 스트리밍이나 폴링을 끝없이 이어 가는 페이지는 `networkidle`에 도달하지 못하고 진행 중인 요청 목록과 함께 시간 초과로 실패한다.
- browser_repl이 취소되거나 시간 초과되면 호스트 드라이버 작업은 즉시 멈추고 CPU 점유로 멈춘 렌더러를 복구하지만, 실행 중이던 page.evaluate가 이미 시작한 비동기 후속 작업은 페이지 자체 스크립트와 마찬가지로 문서 안에서 계속 이어질 수 있다. 문서 상태가 유실되지 않도록 임의로 페이지를 새로고침하지 않으므로, 페이지 내 모든 자바스크립트 실행을 완전히 멈춰야 할 때는 탭을 닫아야 한다.

## 입력 충실도

모든 포인터 및 키보드 입력은 계속해서 신뢰할 수 있는 `Input.dispatch*` 이벤트로 전달하며, 입력 전달 자체에는 JavaScript를 사용하지 않는다. 다만 신뢰할 수 있는 내부 JavaScript는 설정 정보를 읽거나 바운딩 박스를 샘플링하고, 스크롤을 계획하거나 입력 후 필드 값을 확인하는 보조 작업에 쓰일 수 있다. 모든 이벤트는 전송 직전에 정책 검사를 거친다.

### 조작 가능 검사

`click`, `dblclick`, `hover`, `check`, `dragTo`, `scrollIntoViewIfNeeded`는 대상이 다음 조건을 모두 만족하는 단 하나의 요소로 확정될 때까지 기다린다(기본 대기 시간 5초, `timeout` 옵션으로 변경 가능).

- **표시됨(visible):** 빈 상자가 아니어야 하며, `visibility:hidden`이 아니어야 한다.
- **활성화됨(enabled):** `disabled` 상태가 아니어야 한다. 비활성화된 `<fieldset>` 안에 있어도 첫 번째 `<legend>`에 속해 있다면 활성으로 보지만, 비활성화된 `<optgroup>` 내부이거나 요소 자신 또는 조상에 `aria-disabled="true"`가 붙어 있으면 안 된다.
- **안정됨(stable):** 네 차례 연속으로 requestAnimationFrame 샘플을 수집하는 동안 요소의 전체 경계 사각형(bounding rectangle)에 변동이 없어야 안정 상태로 판정한다.
- **히트 타깃 일치:** 브라우저 히트 테스트로 클릭 좌표를 조회했을 때 대상 요소 본인이거나 그 자손이어야 한다. `pointer-events:none` 스타일이 적용된 오버레이는 가림막으로 치지 않는다.

조건을 만족하지 못하면 재시도하다가 결국 `TimeoutError: Timeout exceeded waiting for <locator> to receive pointer events: it <reason>` 예외를 던지며 실패한다. 여기서 `<reason>`은 `is not attached`, `has no box`, `is not stable`, `is outside the viewport`, `is covered by <div id="scrim" class="x"> that would receive the pointer event` 가운데 하나다. 다른 요소에 가려졌을 때는 가림막 태그와 함께 `id`, `class`, `role`, `aria-label`, `name`, `type` 속성을 각각 최대 40자까지 잘라서 보고한다.

지원하는 옵션은 다음과 같다.

- `button`: `left`, `right`, `middle`, `back`, `forward` 중 하나를 고른다.
- `clickCount`: 1에서 3 사이의 정수다. 다른 값이 들어오면 어떤 이벤트도 보내지 않고 즉시 실패한다.
- `delay`: 누르고 뗄 때까지 걸리는 시간(ms)이다.
- `position`: 요소 상자 좌상단을 기준으로 한 `{x, y}` 상대 좌표다.
- `modifiers`: `Alt`, `Control`, `ControlOrMeta`, `Meta`, `Shift`를 받는다. 키 이벤트 없이 마우스 이벤트의 `modifiers` 필드로만 전달한다.
- `force`: 안정 상태, 히트 타깃, 활성화 여부 검사를 건너뛴다. 이벤트는 해당 좌표에 놓인 요소로 전달되며, 정책 검사 역시 로케이터 대상이 아니라 실제 좌표에 놓인 요소를 판정한다.
- `trial`: 실제 이벤트를 보내지 않고 모든 검사만 미리 수행한다.

포인터 이벤트를 하나 보낼 때마다 페이지 상태가 달라졌을 수 있다. 따라서 이벤트 직전마다 문서가 승인받은 원래 문서인지, 대상 요소가 여전히 이벤트를 받을 수 있는 상태인지 확인하고 정책 검사를 다시 거친다.

- 최초 마우스 누르기(press) 전에 요소가 움직이면 동작 제한 시간(action deadline) 범위 안에서 접근 단계를 다시 시도한다(호버 이벤트 등이 전달되었더라도 누르기가 발생하기 전이라면 재시도할 수 있다).
- 최초 마우스 누르기 이벤트가 전달된 후에는 해당 동작을 절대 재시도하지 않는다.

### 스크롤

포인터 동작을 시작하기 전에 대상을 실제 휠 이벤트(`Input.dispatchMouseEvent`, 타입 `mouseWheel`, `isTrusted`는 `true`)로 뷰포트 안에 끌어온다. 수식키 없이 최대 240 px 단위로 감속하며 휠을 굴린다. 페이지 안에서 실행되는 스크롤 계획 스크립트(숫자만 반환)가 shadow host와 slot을 포함해 가장 안쪽 조상부터 바깥쪽으로 스크롤 가능한 조상을 차례대로 탐색한다. 그런 다음 중첩된 스크롤 컨테이너를 피해 실제로 움직여야 하는 스크롤 영역의 보이는 위치를 골라 좌표로 지정한다. 휠 동작은 최대 8회까지 시도하며, 매번 부드러운 스크롤이 완전히 멈출 때까지 기다린다. 휠을 2회 굴려도 요소가 움직이지 않으면 최후의 수단으로 브라우저 내장 `DOM.scrollIntoViewIfNeeded`를 호출하고, 그 뒤 일반 히트 타깃 검사로 성공 여부를 판정한다. 이 스크롤 흐름은 `locator.scrollIntoViewIfNeeded()`로 직접 호출할 수도 있다.

### 드래그 앤 드롭과 `page.mouse`

`locator.dragTo(target, {sourcePosition, targetPosition, force, trial})`와 `page.dragAndDrop(source, target, options)`는 출발지에서 왼쪽 버튼을 누른 뒤 대상까지 8단계로 나누어 이동하고 대상 위에서 놓는다. 페이지가 HTML5 드래그(`draggable`, `dragstart`)를 시작하면 브라우저가 이를 알리고(`Input.setInterceptDrags` / `Input.dragIntercepted`), REPL은 `Input.dispatchDragEvent`로 `dragEnter`, `dragOver`, `drop`을 차례로 보낸다. 덕분에 `dataTransfer`를 다루는 `dragenter`/`dragover`/`drop` 핸들러가 정상 작동한다. HTML5 드래그가 아니라면 일반 마우스 이벤트를 전송해 슬라이더나 캔버스를 조작한다. 정책 검사의 경우 버튼을 누를 때는 출발지 요소를, 버튼을 놓을 때는 도착지 요소를 기준으로 판정한다(동작명은 `drop on`). 그사이의 이동 이벤트는 출처만 확인한다. 드래그가 실패하면 `dragCancel` 이벤트를 전송한다.

`page.mouse`는 `move(x, y, {steps})`, `down({button, clickCount})`, `up({button, clickCount})`, `click(x, y, {button, clickCount, delay})`, `dblclick(x, y, options)`, `wheel(deltaX, deltaY)`를 제공한다. 좌표는 뷰포트 기준 CSS 픽셀 단위이며, 뷰포트 바깥 좌표를 지정하면 `point (x, y) is outside the WxH viewport` 오류로 실패한다. 로케이터가 없으므로 모든 마우스 이벤트는 해당 순간 포인터 바로 아래에 있는 요소(히트 테스트 결과, 역할, 이름)를 기준으로 정책을 검사한다. 따라서 결제 버튼이나 삭제 버튼 위에 포인터가 올라가면 로케이터를 클릭할 때와 똑같이 승인을 요구한다. `wheel`은 출처만 검사하며, 현재 누르고 있는 수식키 상태를 그대로 싣는다.

### 키보드

키보드는 영문, 숫자, 문장 부호, Shift 기호(`!`, `?`, `{`), 기능키(F1-F24), 탐색키, 키패드 키(`Numpad0`-`Numpad9`, `NumpadEnter` 등; `location` 3), 좌우 수식키를 모두 포함하는 US 풀 레이아웃을 기준으로 동작한다. 각 키는 `key`, `code`, `windowsVirtualKeyCode`, `location`, `isKeypad`, `text` 값을 갖는다. 키 이름으로는 키 식별자(`Enter`), 코드(`KeyA`), 직접 입력할 문자(`a`, `A`, `!`), 또는 조합 단축키(`Control+Shift+T`, `ControlOrMeta+a`, Shift와 덧셈 기호를 함께 누르는 `Shift++`)를 넘길 수 있다. 알 수 없는 키나 수식키 이름이 들어오면 어떤 이벤트도 보내지 않고 즉시 실패한다.

`page.keyboard`는 `press(key, {delay})`, `down(key)`, `up(key)`, `type(text, {delay, ime})`, `insertText(text)`, `imeSetComposition(text, {selectionStart, selectionEnd})`를 제공한다. 누르고 있는 키 상태는 호출 전체에 걸쳐 추적한다. `down`으로 누른 수식키는 이후 키와 마우스 이벤트에 반영되고, Shift 키를 누르고 있으면 Shift 레이어가 선택되며, 키를 떼지 않고 다시 보내면 자동 반복(`autoRepeat`)으로 처리한다. `Control`이나 `Meta`가 들어간 조합 키에는 Blink 편집 명령을 덧붙이므로, 헤드리스 Chromium에서도 전체 선택, 복사, 잘라내기, 붙여넣기, 실행 취소, 다시 실행, 단어 단위 이동과 삭제, Home/End가 정상 동작한다. `type`은 레이아웃에 실제 존재하는 문자는 물리 키를 누르듯 전송하고, 레이아웃에 없는 문자(예: `é`)는 `Input.insertText`로 삽입한다. 호출이 끝나도 누른 상태의 키를 자동으로 떼지 않는다(키를 떼는 동작 자체도 정책 승인을 받지 않은 입력 이벤트이기 때문이다). 브라우저는 각 이벤트에 실려 오는 수식키 플래그 외에 별도 키 상태를 기억하지 않는다. 모든 키 이벤트는 현재 포커스를 가진 노드를 기준으로 승인 여부를 검사하며, 폼 필드 안에서 누른 Enter는 폼 제출 동작으로 승인을 판정한다.

### 한글 조합과 `fill`

`type`, `pressSequentially`, `keyboard.type`으로 한글을 입력하면 두벌식 한글 입력기처럼 조합한다. 자모마다 `Input.imeSetComposition`으로 조합 중인 텍스트를 갱신하며(페이지는 `compositionstart`, ㅎ·하·한 형태의 `compositionupdate`, `beforeinput`의 `insertCompositionText`를 차례로 수신), 뒤이어 모음이 오면 종성을 다음 음절로 넘긴다. 이중모음과 겹받침은 절반을 거쳐 조합되고, 음절이 완성되면 `Input.insertText`로 확정한다(`compositionend`). 본문 안의 공백, 숫자, 영문자는 조합을 먼저 확정한 뒤 일반 키로 입력한다. `ime: false`는 조합 없이 단순 삽입하고, `ime: true`는 조합을 강제하며, 기본값은 한글이 포함되어 있을 때만 조합한다.

`fill`은 조합하지 않는다. 필드 전체를 선택한 뒤 `Input.insertText`로 교체하여 신뢰할 수 있는 `input` 이벤트를 발생시키므로, React 제어 컴포넌트도 값 변경을 감지한다. 그런 다음 호스트를 벗어나지 않는 내부 페이지 스크립트로 60 ms 뒤 필드 값을 다시 읽어 검증한다. 결과는 세 가지다.

- **성공 (`null` 반환):** 다시 읽은 값이 입력값과 일치한다.
- **정규화 (`{normalized: true, value}` 반환):** 값이 다르지만 비어 있지 않은 경우다. 페이지가 대문자 변환, `maxlength` 절삭, 입력값 정제 등을 거친 것으로 본다(`type=password` 필드는 값을 숨기고 `{normalized: true, length}`만 반환).
- **실패 (`fill did not take: the page reset the field to empty` 오류):** 비어 있지 않은 값을 채웠는데도 필드가 빈 값으로 남아 있는 경우다. 제어 컴포넌트가 입력을 거부한 것으로 판정한다.

## 근사와 미지원

| 항목 | 동작 |
|---|---|
| `innerText`, 텍스트 매칭 | 요소 경계마다 공백을 넣는다. CDP DOM에는 공백뿐인 텍스트 노드가 없기 때문이다. |
| `textContent` | 원문 그대로다. 공백뿐인 텍스트 노드가 있으면 Playwright 결과와 달라진다(아래 표 32번). |
| `isEditable`, `toBeEditable` | 활성 상태와 `readonly`만 본다. 요소가 편집 가능한 종류인지는 보지 않는다. |
| `selectOption` | 포커스 후 Home과 ArrowDown 키로 고른다. |
| 범위 | 닫힌 shadow root 내부 탐색 미지원. CSS 결합자(공백, `>` 등)는 shadow 경계를 넘지 못하며 한 번에 하나의 트리 안에서만 일치 항목을 찾는다. 호스트 요소에서 shadow root 내부 요소로 접근하려면 `page.locator('#host').locator('.item')`처럼 로케이터를 체이닝해야 한다. `page.frames()`는 프로미스 반환. `page.frame({ url })`의 조건 함수 미지원. |
| 미지원 API | `addScriptTag`, `addStyleTag`, `exposeFunction/Binding`, `route`, `request`, `context`, `screenshot`(`browser_screenshot` 도구 사용), `pdf`, `setContent`, `dispatchEvent`, `$`, `$$`, `elementHandle(s)`, `innerHTML`, `expect.poll`, `expect.soft`, `getByRole({includeHidden})` |

미지원 API를 부르면 `NotSupported: <API> is not supported by the openside REPL (<이유>)`를 던진다. 목록에 없는 멤버도 `undefined`를 돌려주지 않고 같은 오류를 던진다.

### 이벤트와 파일

| 항목 | 내용 |
| --- | --- |
| 이벤트 구독 | `page.on/once/off/removeListener(name, handler)` 및 `page.waitForEvent(name, { timeout, predicate })`로 구독할 수 있으며, 지원하는 이벤트는 `dialog`, `popup`, `download`, `filechooser` 4개뿐이다. 다른 이벤트 이름을 전달하면 `NotSupported` 예외를 던진다. `waitForEvent`는 호출 즉시 대기자를 등록하므로 `Promise.all([page.waitForEvent('popup'), page.click('#open')])` 형태로 안전하게 쓸 수 있다. 대기 제한 시간 기본값은 5000ms이며 `timeout: 0`을 넘기면 무제한 대기한다. 제한 시간을 넘기면 `TimeoutError`를 던진다. |
| 대화상자 | 대화상자 객체는 `type()`(`alert`, `confirm`, `prompt`, `beforeunload`), `message()`, `defaultValue()`, `accept(promptText)`, `dismiss()` 메서드를 제공한다. 핸들러는 대화상자를 띄운 클릭 등의 동작이 끝나기를 기다리는 도중에 실행되므로 그 자리에서 응답할 수 있다. 한 대화상자에 두 번 응답하면 예외를 던진다. 핸들러가 응답하지 않고 반환되면 데몬의 기본 정책대로 처리한다(`alert`와 `beforeunload`는 수락, `confirm`과 `prompt`는 취소). 스크립트가 대화상자 이벤트를 구독하는 동안에는 해당 탭의 대화상자 제어권을 독점하며 데몬의 자동 응답은 보류된다. 이 제어권은 스크립트가 오류 등으로 종료될 때도 함께 풀린다. 대화상자가 뜬 상태에서 다른 페이지 조작(클릭, DOM 읽기 등)을 호출하면 선행 동작 뒤에 갇혀 멈추므로, 핸들러 안에서는 `accept`/`dismiss`와 순수 연산만 수행해야 한다. 핸들러가 등록되지 않은 상태에서 대화상자가 열리면 DOM 읽기, 입력, evaluate, 내비게이션 등 페이지 조작 요청은 무한 대기하지 않고 대화상자 이름을 담은 `DialogError`와 함께 즉시 실패한다. |
| 팝업 | REPL의 `popup` 이벤트는 `url()`, `title()`, `waitForLoadState()`, `opener()`, `tabId` 속성을 갖춘 읽기 전용 핸들을 반환한다. 팝업은 부모 창을 연 세션 및 동일한 격리 공간에 귀속된다. 핸들에서 `click` 등 지원하지 않는 변경 메서드를 호출하면 `NotSupported` 오류가 발생한다. 이 제약은 이벤트 핸들 객체에만 적용되며 팝업 작업 전반을 막는 것은 아니다. 활성 격리 공간의 소유 세션은 반환된 `tabId`를 일반 브라우저 도구와 로그인/OTP 도구에 전달하여 조작할 수 있다(동일한 정책 및 승인 기준 적용). 핸들 생성 시 팝업 origin이 정책에 부합하는지 검사한다. |
| 다운로드 | `download` 이벤트가 전달하는 객체는 `url()`, `suggestedFilename()`, `saveAs(name)`, `path()`, `failure()` 메서드를 갖춘다. `saveAs`는 다운로드가 끝난 파일을 세션 다운로드 폴더(격리 공간 사용 시 해당 공간 전용 폴더)로 복사하고 저장된 경로를 반환한다. 이때 파일 이름에는 `/`, `\\`, `..`을 포함할 수 없으며 단순 파일 이름 하나만 지정해야 한다. 이름을 생략한 채 `path()`를 호출하면 권장 파일 이름으로 저장한다. `failure()`는 다운로드가 정상이면 `null`, 도중에 취소되면 `'canceled'`를 반환한다. 다운로드는 시작할 때와 저장할 때 각각 정책 게이트(`Gate::Download`)를 거치며, 정책에 걸리면 스크립트 실행이 중단된다. 탭을 소유한 세션만 해당 탭의 다운로드 파일을 저장할 수 있다. Chromium은 저장이 완료되기 전까지 임시 파일명을 `<guid>` 형태로 기록한다. 디렉터리, 원본 경로, 파일 작업 시 심볼릭 링크를 따라가지 않는 디렉터리 디스크립터 기준(descriptor-relative no-follow) 연산만 허용한다. 파일은 이름을 유추할 수 없는 전용 스테이징 파일을 배타적으로 생성해 먼저 기록한 뒤, 검증을 마친 동일 디렉터리 디스크립터를 통해 최종 이름으로 변경한다. 심볼릭 링크를 통해 쓰기 대상을 우회하는 동작은 거부한다. |
| 파일 업로드 | `locator.setInputFiles(files)`와 `page.setInputFiles(selector, files)`에 절대 경로 또는 절대 경로 배열을 전달해 업로드할 수 있으며, 빈 배열을 넘기면 기존 선택을 지운다. 대상 요소는 반드시 `<input type=file>`이어야 한다. 경로 문자열만 지원하며 `{ name, mimeType, buffer }` 형태의 메모리 버퍼를 넘기면 `NotSupported`를 던진다. 정책 게이트(`Gate::Upload`)는 입력된 원본 경로와 심볼릭 링크를 모두 푼 정규 경로(canonical path)를 함께 검사한다. 세션 작업 공간, 세션 메시지 첨부 파일, 세션 전용 다운로드 폴더 안의 파일만 허용하며, 전체 접근 모드라도 그 외의 경로나 `..`이 포함된 경로는 모두 차단한다. 정책에 걸리면 스크립트 실행을 즉시 중단한다. 디렉터리 경로를 전달하거나 대상 파일이 없으면 에러를 낸다. |
| 파일 선택창 | 스크립트가 `filechooser`를 대기하면 브라우저 기본 파일 선택 창을 가로채 `setFiles(files)`와 `isMultiple()` 메서드를 갖춘 객체로 전달한다. `setFiles`에는 `setInputFiles`와 동일한 업로드 정책을 적용한다. 선택 창 가로채기는 스크립트가 끝나면 자동으로 해제된다. 파일 선택기를 트리거한 프레임의 frameId와 host 정보를 유지한다. 파일 선택기 이벤트를 전달할 때와 실제 Upload 게이트를 통과할 때 대상 문서의 오리진을 각각 검증한다. 동일 프로세스 하위 프레임을 정상 지원하며, 사이트 격리 하위 프레임(OOPIF)은 Chromium이 가로챈 이벤트를 명시적으로 발행할 때만 지원한다. |
| 스크린샷 | REPL 환경에서는 `page.screenshot`을 지원하지 않으므로 전용 도구인 `browser_screenshot`을 써야 한다. `browser_screenshot`은 스크롤 가능한 전체 페이지를 캡처하는 `full_page`(크기 제한 적용), 뷰포트 좌표계(CSS 픽셀 단위, `browser_pointer`와 동일) 기반의 `clip` `{x,y,width,height}`, `full_page`와 함께 문서 좌표계로 지정하는 `clip`, 특정 요소 하나를 지정하는 `target`(`eN` 스냅샷 참조, 화면에 보이도록 먼저 스크롤하며 타 오리진 프레임 내 요소는 미지원) 옵션을 지원한다. `target`은 `full_page`나 `clip`과 함께 쓸 수 없다. |

원본 경로와 정규화(canonical) 경로에 대해 Upload 검증을 마친 뒤, 모든 상위 디렉터리와 대상 파일을 심볼릭 링크를 따라가지 않는 디렉터리 디스크립터 기준(descriptor-relative no-follow) 탐색으로 열어 일반 파일(regular file)인지 확인한다. 검증된 바이트를 탭별 전용 스테이징 디렉터리(0700)에 원본 파일명(basename)으로 복사하고, 변경 불가능한 스테이징 경로를 Chromium에 전달한다. 스테이징된 업로드 사본은 탭의 수명주기와 함께 유지되며, 탭이 닫히거나(격리 공간 아카이브로 인해 탭이 닫히는 경우 포함) 소유 세션이 삭제(delete)될 때 제거한다. 세션 중지(stop)나 공유 세션 아카이브 시에는 여전히 열려 있는 탭의 페이지 내 지연(lazy) File 읽기가 정상 작동하도록 스테이징 파일을 그대로 보존하며, 데몬이 시작되거나 재연결될 때 살아 있는 탭의 정리 레코드를 유지·재바인딩하고 활성 탭이 없는 고아 스테이징 디렉터리를 일괄 정리(sweep)한다. 작업 도중 상위 디렉터리가 심볼릭 링크로 교체되더라도 접근을 거부하거나 이미 열어 둔 원본 파일의 바이트만 유지하므로, 심볼릭 링크가 가리키는 대상의 바이트가 읽히는 일은 없다.

스크립트 실행 중 발생한 이벤트는 다른 실행으로 전달되지 않으며, 스크립트가 구독을 등록하기 전에 이미 일어난 이벤트는 다시 전달하지 않는다. 단, 이벤트 수신 대기는 스크립트가 시작되는 시점부터 즉시 유효하다.

## 호환성 스위트

`crates/openside-repl/tests/compat.rs`가 로컬 고정 페이지(`tests/fixtures/`)와 실제 Chromium으로 아래 표를 계산한다. 이 표와 다르면 테스트가 실패한다. 또한 REPL이 입력 이벤트를 쓰지 않고 `DOM.setAttributeValue`나 `DOM.setOuterHTML` 등으로 DOM을 직접 수정하면 테스트가 실패한다. 통과율은 아래 52개 케이스 안에서의 비율일 뿐이고, Playwright API 전체의 호환 정도를 뜻하지 않는다.

정책 검사는 `crates/openside-repl/tests/policy.rs`(다른 출처로 가는 링크·스크립트·리다이렉트 탐색, 덮개 요소, Enter 제출, 민감한 선택지, 입력 중 포커스 이동, 이벤트 사이에 바뀐 대상 이름, iframe 내부 키 입력, 동시 동작과 정지, 콘솔·실행 한도, 호스트 크기 한도), `crates/opensided/src/tests/repl.rs`(동작 단위 승인, 인자 변경 거부, 탭 묶음, 두 번의 승인 지점, 출처의 `sensitive` 거부 규칙과 승인, 비밀값 탭 거부), `crates/openside-repl/tests/script.rs`(값 왕복 직렬화, 핸들 동작, `$eval` 및 `$$eval`, `locator.evaluate`, 초기화 스크립트, `page.cdp`, `Gate::Script` 검사, 무한 루프 등 미반환 스크립트 복구, 드라이버 동작과 페이지 스크립트 일괄 실행), `crates/openside-repl/tests/frames.rs`(동일 출처 및 프로세스 분리 프레임에서의 동작, `page.frames` 및 `page.frame`, 프레임 내부 스냅샷 ref, shadow DOM, XPath, 엄격 모드 후보 목록, 프레임 출처 차단 및 스냅샷 제외 검증)가 확인한다.

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
| 50 | `evaluate-arg` | page.evaluate(fn, arg) | pass |
| 51 | `eval-selectors` | page.$eval · page.$$eval | pass |
| 52 | `locator-evaluate` | locator.evaluate · evaluateAll | pass |

통과율: 51/52 (98.1%)
<!-- compat:end -->

32번은 알려진 차이다. CDP DOM 도메인이 공백뿐인 텍스트 노드를 보내지 않아서, `textContent`가 `"Apple Delete"` 대신 `"AppleDelete"`를 돌려준다.
