# WebContents 탭 소유권

## 1. 상태와 저장 수명

`TabOwnership`은 `content::WebContentsUserData<TabOwnership>`다. 전용 agent 브라우저의 CDP 탭 생성 훅(`patches/0001`)이 붙인다. gateway(`patches/0002`)는 모든 탭에 `tab-<n>` ID를 주고 `tabs.list`·탭 이벤트로 소유권을 보고한다. agent 탭은 "Agent Tabs" 탭 그룹에 넣는다. 실행 증거는 `crates/openside-browser/tests/patched.rs::ownership_ledger_reports_agent_tabs_in_their_group`다. 닫기 권한 검사와 사용자 takeover 관찰자는 아직 없다.

메타데이터는 탭 index가 아니라 `WebContents` 수명을 따른다. 같은 `WebContents`의 navigation·탭 재정렬·detach/reinsert에는 기록을 유지한다. 교체한 `WebContents`에는 새 생성 출처를 기록하며 기존 권한을 무조건 복사하지 않는다. 알 수 없거나 복원한 탭은 `external`로 취급한다.

## 2. 필드

| `TabMetadata` 필드 | C++ | wire·의미 |
| --- | --- | --- |
| `owner` | `TabOwner::kAgent|kUser|kExternal` | 현재 제어 주체 `agent|user|external` |
| `session_id` | `std::string` | 생성 세션 `string|null`. 빈 값은 없음 |
| `subagent_id` | `std::string` | 생성 서브에이전트 `string|null`. 빈 값은 루트 |
| `label` | `std::string` | 탭 용도 표시. 인증 정보·비밀을 넣지 않는다 |
| `background` | `bool` | agent background 생성·제어 상태 |
| `opened_by` | `OpenedBy::kAgent|kUser|kExternal` | 변경하지 않는 생성 출처 `agent|user|external` |

기본값은 `owner:kExternal`, `opened_by:kExternal`, 빈 식별자·label, `background:false`다. agent에는 비어 있지 않은 `session_id`가 필요하다. 빈 `subagent_id`는 루트이며 모든 자식을 뜻하지 않는다.

`owner`와 `opened_by`는 다르다. 사용자 takeover 후에도 생성 세션·서브에이전트·`opened_by`를 남긴다. `owner:user`이면 agent 권한은 없다.

생성자는 `TabMetadata`를 move한다. 문자열 accessor는 const reference를 반환한다. `ReturnToUser()`는 `owner:kUser`, `background:false`만 바꾼다. 생성 출처·식별자는 바꾸지 않는다. P1에는 이 함수를 호출하는 사용자 관찰자가 없다.

## 3. 출처 판정과 첫 훅

| 생성 경로 | `owner` | `opened_by` | 귀속 |
| --- | --- | --- | --- |
| 인증된 gateway `tab.open` | `agent` | `agent` | 요청 실행 scope |
| native 사용자 탭 생성 | `user` | `user` | 없음 |
| 복원·import·미확인·외부 앱 | `external` | `external` | 없음 |
| 페이지 팝업 | `external` | `external` | opener만 별도로 보고 |

이 표는 후속 gateway·관찰자의 계약이다. P1은 `TargetHandler::CreateTarget`에서 비어 있지 않은 `--openside-agent-session-id`를 확인한 경우만 agent로 기록한다. 선택 `--openside-agent-subagent-id`가 없으면 루트다. 신뢰한 데몬이 전용 임시 프로필로 시작한 한 scope의 브라우저에만 붙인다. 인증 토큰이나 공유 브라우저의 multiplexing 수단이 아니다.

훅은 성공한 `Navigate` 뒤, target ID 응답 전에 다음을 저장한다.

```text
owner = kAgent
session_id = launch session
subagent_id = launch subagent or empty root
label = empty
background = true
opened_by = kAgent
```

표시된 경로에는 `background=true`, `focus=false`를 강제한다. 기록을 건너뛰는 `hidden=true`와 `Normal`이 아닌 `window_state`는 거부한다. marker가 없는 경로는 기존 Chromium 동작을 유지하며 메타데이터를 만들지 않는다. 후속 adapter는 미기록 탭을 `external`로 해석한다.

페이지 팝업은 opener가 agent여도 소유권을 상속하지 않는다. ancestry로 권한을 얻을 수 없다. 지원하지 않은 생성 경로를 agent로 추측하지 않는다.

## 4. 닫기 권한

| 호출자·탭 | 허용 |
| --- | --- |
| native 사용자 UI | 모든 탭 |
| agent와 정확히 같은 세션·서브에이전트가 연 agent 탭 | 아래 조건을 모두 만족할 때만 |
| 세션 루트 → 자식 탭 | 거부 |
| 서브에이전트 → 형제·루트 탭 | 거부 |
| `owner:user`, 생성 출처는 `agent` | 거부 |
| 사용자·외부·미확인 탭 | 거부 |

agent 닫기는 `owner:agent`, `opened_by:agent`, `session_id` 일치, `subagent_id` 일치, 사용자 제어 상태 아님을 모두 확인한다. 권한 검사를 실패하면 raw CDP 닫기로 우회하지 않는다. 데몬의 취소·정리도 원래 scope로 수행한다. 전체 탭을 닫는 특별 권한을 만들지 않는다.

탭 조회·권한 검사·닫기는 UI sequence에서 원자적으로 처리한다. `beforeunload`가 막으면 `dialog_pending`을 반환한다. 실제 파괴 전에 닫기 성공을 보고하지 않는다. 세션 종료 시에도 여전히 소유한 background agent 탭만 정리한다. 사용자가 가져간 탭은 유지한다.

기존 탭을 claim하거나 다른 agent로 재할당하는 API는 없다. 다른 agent는 자기 탭을 새로 만든다. 첫 패치에는 이 닫기 강제 지점이 아직 없다.

## 5. 상태 전이

| 사건 | 처리 |
| --- | --- |
| 생성 | 생성 출처·creator 식별자를 기록한다 |
| native 사용자 선택·키 입력·take-control | `ReturnToUser()`, focus grant 취소, `tab.updated(reason:user_takeover)` |
| 같은 소유 scope의 label 변경 | 표시만 갱신한다. 권한은 바꾸지 않는다 |
| discarded·unloaded | 기록 유지. attach·캡처 거부 |
| 파괴 | `tab.closed` 한 번 발행 후 안정 `tab_id`를 해제한다 |
| 교체·미확인 `WebContents` | 권한을 상속하지 않는다 |

focus grant는 탭 소유권과 별개다. 다른 소유자의 탭을 제어할 권한을 주지 않는다. 후속 lifecycle은 안정 `tab_id`와 `cdp_target_id`를 연결하며 strip index를 쓰지 않는다. 데몬 attach에는 12초 race를 적용한다. 탭 수 `>=5`는 경고 기준이지 사용자 탭을 닫을 근거가 아니다.

## 6. 빌드·Rust 연결

오버레이 `BUILD.gn`의 `source_set("openside")`는 `//content/public/browser`를 공개 의존성으로 가진다. P1 패치는 `//chrome/browser:browser`에 `//chrome/browser/openside`를 연결한다. `//chrome/browser/devtools`에도 직접 의존성을 추가한다. devtools는 GN cycle을 피하려고 browser target에 직접 의존하지 않으므로 이 연결이 필요하다.

`openside-browser`는 wire `opened_by`를 Stage A 장부의 `openedBy:agent|user|external`로 매핑한다. 후속 wire 스키마의 단일 원본은 `openside-proto`다. D-3은 Rust 파일을 수정하지 않는다.

## 7. 검증 계약과 출처

후속 검증 항목은 재정렬·navigation 보존, 교체 시 권한 미상속, 루트·자식·형제 닫기 경계, takeover 후 출처 보존, 지연된 `beforeunload`, 팝업 권한 미상속, 복원 탭 external 판정, asleep attach 거부다. patch dry-run은 문맥 적용 가능성만 검사한다. C++ 컴파일·이 규칙의 실행 증거가 아니다.

xplorer `64b3ef1c40e899aa9b9d14657514a5a47838f250`의 다음 파일을 읽었다.

- [`src/chrome/browser/agent_gateway/tab_ownership.h`](https://github.com/daniel-farina/xplorer/blob/64b3ef1c40e899aa9b9d14657514a5a47838f250/src/chrome/browser/agent_gateway/tab_ownership.h)·[`tab_ownership.cc`](https://github.com/daniel-farina/xplorer/blob/64b3ef1c40e899aa9b9d14657514a5a47838f250/src/chrome/browser/agent_gateway/tab_ownership.cc): 수명 기반 기록.
- [`agent_gateway.cc`](https://github.com/daniel-farina/xplorer/blob/64b3ef1c40e899aa9b9d14657514a5a47838f250/src/chrome/browser/agent_gateway/agent_gateway.cc): 생성 기록·`AgentMayUseTab`.
- [`browser_api.cc`](https://github.com/daniel-farina/xplorer/blob/64b3ef1c40e899aa9b9d14657514a5a47838f250/src/chrome/browser/agent_gateway/browser_api.cc): 닫기.
- [`focus_arbiter.h`](https://github.com/daniel-farina/xplorer/blob/64b3ef1c40e899aa9b9d14657514a5a47838f250/src/chrome/browser/agent_gateway/focus_arbiter.h)·[`focus_arbiter.cc`](https://github.com/daniel-farina/xplorer/blob/64b3ef1c40e899aa9b9d14657514a5a47838f250/src/chrome/browser/agent_gateway/focus_arbiter.cc): 사용자 포커스 복귀.

설계만 참고했으며 코드는 복사하지 않았다. xplorer의 `base::SupportsUserData::Data` 구현 대신 Chromium의 `content::WebContentsUserData` API를 사용한다.

라이선스는 `unverified`다. 기준 revision에 루트 `LICENSE`가 없고 파일 헤더는 BSD-style, [`README.md`의 License 절](https://github.com/daniel-farina/xplorer/blob/64b3ef1c40e899aa9b9d14657514a5a47838f250/README.md#license)은 owner-defined 오버레이 조건을 설명한다. [NOTICE.md](../NOTICE.md)에 기록한다.
