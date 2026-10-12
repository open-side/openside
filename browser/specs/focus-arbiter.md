# 포커스 중재

## 1. 상태와 불변식

BR-1 계약에 해당하는 내용이다. 활성화 권한은 대상 탭이나 창뿐만 아니라 요청 주체가 누구인지에 따라서도 결정된다. agent 브라우저(launch marker가 있는 브라우저)에서는 DevTools 엔드포인트를 데몬이 소유하므로, 대상을 어떤 탭으로 지정하든 모든 DevTools 프로토콜 활성화(`Target.activateTarget`, `Page.bringToFront`)는 agent의 요청으로 간주한다. `patches/0010`에서는 이 호출들에 `content::ScopedProtocolActivation`을 표시하여 `BrowserWebContentsDelegate::ActivateContents`가 요청을 거부하도록 만든다. 또 다른 행위자인 페이지의 경우, agent 탭과 그 팝업 자식 창에서 들어오는 `window.focus()`를 거부하며(`patches/0003`), 팝업은 백그라운드 탭이나 비활성 창으로 열린다. 네이티브 권한 부여 UI가 없으므로 agent의 포커스 요청은 항상 거부된다. 실행 가능한 검증 코드는 `crates/openside-browser/tests/patched.rs::agent_tabs_never_take_focus` 및 `window_activation_follows_the_requesting_actor`(헤드리스)에서 확인할 수 있다. 창 단위 활성화(`patches/0009`, `0010`) 및 sway 헤디드 환경 격리 검증은 §4를 참조한다.

에이전트는 포커스를 빼앗지 않는다. native 탭 선택·클릭·키 입력이 우선한다. DOM 이벤트, CDP 입력, wire의 `user_gesture` 주장은 실제 사용자 제스처가 아니다. 포커스 상태는 브라우저 UI sequence가 소유한다. 데몬·모델이 임의로 바꾸지 않는다.

사용자가 agent 탭을 직접 제어하기 시작하면 포커스 권한을 취소하고 `TabOwnership::ReturnToUser()`를 호출한다. 해당 탭은 `owner:user`, `background:false` 상태로 바뀌지만 출처, 세션, subagent 정보는 유지된다. `ReturnToUser()`는 탭에 적용되었던 1440x900 에뮬레이션도 즉시 해제한다(`OnTabOwnerChanged`). 현재는 네이티브 제어 전환을 감지하는 옵저버가 없어 이 함수를 호출하는 곳이 없으며, 사용자 인수인계는 아직 지원하지 않는다. 사용자는 모든 탭을 선택하고 닫을 수 있다. 캡처 동작은 탭을 선택·활성화하거나 창을 올리거나 `Show`를 호출하지 않는다.

## 2. 상태 전이

| 현재 상태 | 입력 | 판정·다음 상태 |
| --- | --- | --- |
| `User` | agent 요청 | 거부. `User` 유지 |
| `User` | native UI의 명시적 작업 포커스 요청 | `AgentGranted`. 지정한 scope의 일회성 grant 발급 |
| `AgentGranted` | 일치하는 scope·탭·run의 유효 grant 요청 | 한 번 허용. grant를 소비하고 `User` 복귀 |
| `AgentGranted` | 다른 세션·서브에이전트 요청 | 거부. 기존 grant 유지 |
| 모든 상태 | 실제 사용자 제스처 | 즉시 grant 취소, `User` 복귀 |
| 모든 상태 | grant 만료, run 종료, 탭 닫힘, 연결 종료, 브라우저 종료 | grant 취소, `User` 복귀 |

grant는 인스턴스·탭·세션·서브에이전트·run에 묶이며 5초 후 만료한다. `grant_id`는 opaque string이다. 한 탭에서만 한 번 쓴다. socket에는 grant 발급 명령이 없다. 토큰을 가진 데몬도 사용자 입력을 만들 수 없다.

사용자 입력 처리, grant 검사·소비, 활성화는 같은 UI sequence에서 직렬화한다. IO thread에서 허용한 뒤 늦게 활성화하지 않는다. 허용 요청은 grant를 소비하여 `User`로 돌린 뒤 활성화를 완료한다. 후속 agent 요청이 같은 grant를 재사용할 수 없다.

## 3. 구현한 판정 함수

```text
DecideFocus(FocusActor, session_id, subagent_id, FocusContext)
    -> FocusDecision
```

| 조건 | 결과 |
| --- | --- |
| `FocusActor::kUserGesture` | `FocusDecision::kAllow` |
| agent의 `session_id`가 비어 있음 | `kDenyInvalidIdentity` |
| `FocusContext::state != FocusState::kAgentGranted` | `kDenyNoUserGrant` |
| 세션 또는 서브에이전트가 grant와 다름 | `kDenyDifferentOwner` |
| 세션·서브에이전트 모두 일치 | `kAllow` |

빈 `subagent_id`는 세션 루트이며 wildcard가 아니다. `FocusActor::kUserGesture`는 native 입력을 확인한 호출자만 사용할 수 있다. wire 필드에서 직접 만들지 않는다.

`FocusContext`의 `std::string_view`는 동기 호출 동안만 브라우저가 소유한 식별자를 참조한다. 함수에는 singleton·전역 lock·할당·문자열 복사가 없다. grant의 탭·run·만료·일회성 검사, 인증, 실제 UI 변경은 호출자가 맡는다. 판정 함수 자체는 grant를 발급하거나 소비하지 않는다.

## 4. 강제 지점과 P1 범위

후속 훅은 `Target.activateTarget`, 프로그램의 탭 선택, 새 창 표시, `window.focus()`, 팝업, split-tab, 다이얼로그, 다운로드 UI까지 같은 판정을 적용해야 한다. 브라우저 탭 포커스와 native 창·WM 활성화는 다른 문제다. 브라우저에서 요청을 거절한 뒤 OS 활성화가 일어나면 불변식을 지키지 못한 것이다.

`NEW_BACKGROUND_TAB`만으로 충분하지 않다. 빈 strip에서는 새 탭이 선택될 수 있다. 별도 inactive agent 창과 WM 규칙을 함께 검증해야 한다. 숨은 탭 캡처 실패를 foreground 캡처로 바꾸지 않는다.

P1의 agent 경로는 `TargetHandler::CreateTarget`에서 다음 launch marker를 확인한다.

- `--openside-agent-session-id`: 비어 있지 않은 세션 ID.
- `--openside-agent-subagent-id`: 선택 서브에이전트 ID. 없으면 루트.

표시된 프로세스는 신뢰한 데몬이 전용 임시 `--user-data-dir`로 시작한 브라우저여야 한다. 하나의 프로세스에 하나의 세션·서브에이전트를 붙인다. marker는 인증 수단이 아니다. 사용자 프로필이나 다중 scope 공유 프로세스에 붙이지 않는다.

이 경로는 `background=true`, `focus=false`를 강제한다. 소유권 기록을 건너뛰는 `hidden=true` 경로와 `Normal`이 아닌 `window_state`를 거부한다. marker가 없으면 기존 Chromium 동작을 유지한다.

P1은 CDP 활성화, HTTP `/json/new`, 페이지 팝업, content의 hidden-target 경로, 사용자 제스처 관찰자를 연결하지 않는다. 전체 포커스·닫기 강제를 완료한 상태가 아니다.

### P2에서 막는 지점

agent 탭과 그 팝업 후손(agent 탭이 연 팝업, 그 팝업이 다시 연 팝업)을 "agent 출처"로 본다. 후손의 소유권은 `external` 그대로이고 agent 권한을 물려받지 않는다. 포커스 제한만 물려받는다.

| 진입점 | 동작 |
| --- | --- |
| `Target.activateTarget`(agent 브라우저의 모든 탭), agent 출처 페이지의 `window.focus()`(`ActivateContents`) | 거부하고 대상 탭에 `focus.decided`를 전송한다. DevTools 클라이언트가 사용자 탭을 지정한 경우도 거부된다(`0010`). |
| `Page.bringToFront` | 위 행과 동일하게 처리한다(`0010`). agent 브라우저(launch marker가 있는 브라우저)에서는 뒤이어 실행되는 뷰 `Focus()`도 건너뛴다. |
| agent 출처 페이지의 foreground 팝업·새 탭(`AddNewContents`, `OpenURLFromTab`) | background로 바꾼다 |
| fullscreen 요청 | agent 출처 페이지는 거절한다 |
| `Browser.setWindowBounds`, `Browser.setContentsSize` | agent 브라우저에서는 사용자 창을 포함한 모든 창에 에러를 반환한다. 해당 환경의 DevTools 클라이언트는 모두 agent이기 때문이다. 최대화 등 창 상태를 바꾸는 요청도 마찬가지로 에러를 반환한다. |
| 페이지 레이아웃(P5) | agent가 소유한 탭은 전용 창에 있든, 공유 창에 숨겨져 있든, 사용자가 보고 있는 창에 표시되든 상관없이 항상 디바이스 배율(device scale factor) 1의 1440x900 CSS 픽셀로 레이아웃을 잡는다. 브라우저는 자체 내부 DevTools 클라이언트를 통해 이 오버라이드를 유지하며 유일한 소유자로 동작한다. 컨트롤러는 agent 탭에 오버라이드를 별도로 보내지 않는다. 크기 변경, 가시성 변화, 탭 스트립 삽입 및 활성화, 다른 DevTools 세션의 연결 해제(세션마다 감지한다) 시, 그리고 매 캡처 직전에 이를 다시 확인하며, 캡처는 렌더러가 오버라이드를 적용할 때까지 최대 3초 기다린다. 크기는 그대로 두고 배율만 바꾼 다른 클라이언트의 오버라이드는 그 클라이언트가 연결을 끊거나 다음 캡처가 시작될 때 되돌린다. 전용 창의 경우 창 관리자가 허용하면 사용자가 전체 페이지를 볼 수 있도록 콘텐츠 크기에 맞춰 창 크기도 함께 조절한다. 사용자 탭은 에뮬레이션하지 않으며, agent 소유에서 벗어난 탭은 에뮬레이션을 즉시 해제한다. |
| 창 표시/활성화(`BrowserView::Show`/`Activate`, `0009`), 확장 프로그램(extension) `chrome.windows.update`(`0010`) | 창 안의 모든 탭이 agent 출처이면 창을 비활성 상태로 표시하고 절대 활성화하지 않으며, `focus.decided`를 전송한다. 이런 창에 포커스, 상태 변경, 크기/위치(bounds), `drawAttention`을 요청하는 `chrome.windows.update`는 작업을 시작하기 전에 에러를 반환한다. `Target.createTarget`의 `newWindow`나 agent 탭을 이동하는 `focused:true` 속성의 `chrome.windows.create`로 생성된 창도 비활성 상태로 표시된다. 확장 프로그램은 사용자를 대신해 동작하므로, agent 탭 포함 여부와 관계없이 사용자 탭이 하나라도 있는 창은 기존처럼 확장 프로그램(즉 사용자)이 직접 띄우고 크기를 바꾸고 포커스할 수 있다. |
| 다운로드 UI | agent 출처 페이지에서 시작된 다운로드는 다운로드 버블을 자동으로 열지 않는다. |
| 탭 모달 JavaScript 대화상자 | 탭에 주의 필요 표시만 남기고 창을 활성화하지 않는다(코드 확인 완료, 변경 불필요). |

`crates/openside-browser/tests/patched.rs`의 헤드리스 테스트에서 관련 경로를 검증한다(`agent_tabs_never_take_focus`, `window_activation_follows_the_requesting_actor`, `agent_viewport_has_one_owner_across_clients_and_tab_moves`, `agent_viewport_is_restored_when_another_client_leaves_without_a_resize`, `capture_slots_come_back_when_the_renderer_stops_answering`, `maximized_agent_window_is_emulated_at_1440x900`). 2026-10-12 격리된 헤드리스 sway(루트리스 podman, 네트워크 없음) 환경에서 진행한 헤디드 검증: 816x991로 타일 정렬된 전용 agent 창에서도 1440x900으로 레이아웃되며 `screenshot.capture`가 1440x900을 반환한다. agent 경로에서 CDP 새 창, `window.focus()`, 팝업, 다운로드, alert, `chrome.windows.update`/`create`를 실행하는 동안 sway 포커스와 urgency는 유지되었다(순정 Chromium은 `chrome.windows.update` 시 창을 urgent로 표시함). 트레이드오프: 탭 검색이나 알림 클릭 등 전용 agent 창을 띄울 수 있는 Chromium 내부의 사용자 동작 역시 창을 활성화하지 않으므로 사용자가 컴포지터에서 창을 직접 클릭해야 한다 [INFERENCE: 확인 안 함]. 미검증: 실제 사용자가 창 관리자를 통해 창을 활성화하는 동작은 대조군이 없어 검증하지 못했다(헤드리스 sway 환경에서는 가상 Ctrl+N 입력이나 확장 프로그램 요청으로 포커스 변화를 관찰할 수 없었다). Picture-in-Picture, 파일 선택 창, 인쇄 대화상자, `Input.*`의 `widget_host->Focus()`를 통한 페이지 내 포커스, 다운로드 버블 숨김 처리는 자동화 테스트가 없다. 위의 헤디드 검증은 `0010` 패치와 단일 뷰포트 소유자 모델을 적용하기 전에 수행한 것이며 다시 실행하지는 않았다. 권한 부여 UI와 사용자 제어 감지기는 아직 구현되지 않았다.

## 5. Wayland compositor 규칙

데스크톱 설정은 변경하지 않는다. [docs/03 §6](../../docs/03-architecture.md)의 `noinitialfocus`, `suppressevent activate`는 전용 agent 창에 한정한 보조 규칙이다(compositor가 이런 창 규칙을 지원하는 경우). 모든 Chromium 창에 적용하지 않는다.

compositor 버전별 규칙 문법·window class 매칭은 `unverified(no-desktop-test)`다. `openside doctor`는 지원 여부·누락을 알려야 하며 몰래 설정을 수정하지 않는다. WM 규칙은 브라우저 내부 강제를 보완할 뿐 숨은 탭 캡처 안전성의 증거가 아니다. 검증은 `/tmp` 임시 프로필과 `127.0.0.1` 목업만 사용한다.

## 6. 검증 계약과 출처

후속 검증은 기본 거부, 일치 grant, 루트·자식·형제 scope 불일치, 사용자 takeover, 만료·run 종료, 동시 요청의 단일 소비, 캡처·팝업·다이얼로그·다운로드 중 포커스 불변, Wayland compositor의 빈 창 생성이다. D-3에서 C++ 컴파일·실행 포커스 테스트는 하지 않았다.

xplorer `64b3ef1c40e899aa9b9d14657514a5a47838f250`의 다음 파일을 읽고 설계만 참고했다.

- [`src/chrome/browser/agent_gateway/focus_arbiter.h`](https://github.com/daniel-farina/xplorer/blob/64b3ef1c40e899aa9b9d14657514a5a47838f250/src/chrome/browser/agent_gateway/focus_arbiter.h)·[`focus_arbiter.cc`](https://github.com/daniel-farina/xplorer/blob/64b3ef1c40e899aa9b9d14657514a5a47838f250/src/chrome/browser/agent_gateway/focus_arbiter.cc).
- [`agent_gateway.cc`](https://github.com/daniel-farina/xplorer/blob/64b3ef1c40e899aa9b9d14657514a5a47838f250/src/chrome/browser/agent_gateway/agent_gateway.cc)의 `/focus`, `MayActivate` 호출 지점.
- [`tab_screenshot.cc`](https://github.com/daniel-farina/xplorer/blob/64b3ef1c40e899aa9b9d14657514a5a47838f250/src/chrome/browser/agent_gateway/tab_screenshot.cc)의 비활성 캡처 경로.

기본 거부·실제 사용자 동작 시 복귀를 참고했다. 누락한 헤더를 사용자 신분으로 취급하는 규칙과 singleton 구현은 가져오지 않았다. 코드는 복사하지 않았다.

라이선스는 `unverified`다. 기준 revision에 루트 `LICENSE`가 없고 파일 헤더는 BSD-style, [`README.md`의 License 절](https://github.com/daniel-farina/xplorer/blob/64b3ef1c40e899aa9b9d14657514a5a47838f250/README.md#license)은 owner-defined 오버레이 조건을 설명한다. [NOTICE.md](../NOTICE.md)를 따른다.
