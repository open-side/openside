# openside 확장

패치하지 않은 순정 Chromium 에 openside UX 를 제공하는 MV3 확장이다. TypeScript 로만 작성했다.

## 요구 사항

- Node 22 이상
- pnpm
- Chromium 126 이상(`sidePanel` API 와 `Promise.withResolvers` 가 필요하다)

## 설치

1. `pnpm -C extension install` 과 `pnpm -C extension build` 를 실행한다.
2. Chromium 에서 `chrome://extensions` 를 연다.
3. Developer mode 를 켠다.
4. Load unpacked 를 누르고 `extension/dist` 디렉터리를 고른다.

## 기기 연결

옵션 페이지에 데몬 URL(루프백 http 또는 https)과 범위를 정한 기기 토큰을 넣는다. 기기는 `read`, `steer`, `approve` 범위로 연결한다. 비밀값 입력·기기 승인·직접 조작(takeover) 요청을 확장에서 처리하려면 `human_step` 을 더한다.

- 토큰은 `chrome.storage.session` 에만 둔다. local·sync 저장소에는 쓰지 않고, 로그에 남기지 않으며, 웹 페이지에 넘기지 않는다. 브라우저를 다시 시작하면 다시 연결해야 한다.
- 데몬 origin 권한은 저장할 때만 요청한다.
- 루프백이 아닌 호스트로 가는 평문 http, 사용자 정보·쿼리·프래그먼트가 붙은 URL 은 저장하지 않는다.
- 저장한 뒤에는 토큰을 다시 보여 주지 않는다.

## 기능

### Ask (사이드 패널)
- 이 탭에서 시작한 세션 목록을 보여 준다.
- 입력창은 고른 세션이나 새 세션으로 메시지를 보내고, 데몬의 답을 실시간으로 받아 보여 준다.
- 보류 중인 승인·정보 요청 카드는 패널에서 바로 답하거나 거절한다.
- 기기 승인과 직접 조작 카드는 로컬에서 작업을 마친 뒤 Done 을 누르라고 안내만 한다.
- 비밀값은 password 입력란으로만 받는다.
- 어느 대화에서 들어온 코드 요청이든 패널 안에 코드 입력 폼을 바로 띄우며, 패널이 닫혀 있을 때는 툴바 배지와 일회성 알림으로 알려준다. 알림을 클릭하면 패널이 열린다. 이 알림에 `notifications` 권한을 쓴다.
- `@` 버튼을 누르거나 `@` 만 따로 입력하면 현재 탭의 URL·제목·선택한 텍스트를 붙인다. 사용자가 직접 할 때만 동작한다.

### 화면 디자인
- Ask 사이드 패널과 Options 페이지는 웹 앱(`web/DESIGN.md`)과 같은 디자인 언어를 쓴다. 라이트·다크 색상 토큰, 28px 라운드 입력창, 단색 말풍선, 아웃라인 아이콘, 토스트, 빈 상태와 오류 상태가 같다.
- 테마는 `prefers-color-scheme` 을 따라 브라우저 설정에 맞춰진다. 확장 안에 테마 선택 기능은 없다.
- Wanted Sans 와 D2Coding(둘 다 OFL-1.1, 라이선스 전문 포함)은 빌드할 때 `web/src/assets/fonts` 에서 `dist/fonts` 로 복사한다. 확장 페이지는 외부 폰트나 스크립트를 불러오지 않고, 아이콘도 Lucide 인라인 SVG 를 쓴다.
- 상태 안내와 오류 메시지는 화면 위쪽(Options 는 아래쪽)에 토스트로 뜨고, 일정 시간이 지나거나 직접 닫으면 사라진다.

### Omnibox
주소창에 `os`, 공백, 질문을 차례로 입력하고 Enter 를 누르면 새 세션을 시작하고 Ask 를 연다.

### Gmail·Slack 웹
"Handle with openside" 버튼이나 컨텍스트 메뉴를 쓰면 메시지 안에서 선택한 텍스트(또는 열린 메시지 본문)와 출처 URL 을 새 세션으로 넘긴다. 자동으로 보내는 것은 없다. 페이지 내용은 지시가 아니라 인용한, 신뢰할 수 없는 맥락으로 전달한다.

버튼은 닫힌 shadow root 안에 있어 실제 사용자가 클릭했을 때만 반응한다. 웹페이지 스크립트가 `.click()`을 호출하거나 가짜 이벤트를 발생시켜도 아무런 동작도 일어나지 않는다. 컨텍스트 메뉴 동작은 브라우저가 사용자 입력을 바탕으로 생성하며, 현재 탭의 URL과 일치하는 최상위 Gmail/Slack 문서에서 보낸 요청만 받아들인다. SPA 특성상 URL 해시(#)는 바뀔 수 있지만, origin·path·query는 반드시 일치해야 한다.

### Agent Tabs
- 프로덕션 빌드에서 `debugger` 권한은 선택 사항이다. Options 화면에서 `Enable Agent Tabs`를 누르면 Chrome의 `debugger` 권한을 요청하고(타깃 메타데이터를 읽는 용도로만 사용) 이번 브라우저 세션에 한해 탭 그룹 기능을 켠다. `Disable Agent Tabs`를 누르면 확장 프로그램이 만든 탭 그룹을 모두 해제한다.
- Ask, 메시지 처리, 페어링은 이 권한 없이도 동작한다.
- 확장 프로그램은 주기적으로 `GET /v1/browser/tabs`를 호출해 데몬의 탭 ID를 `chrome.debugger.getTargets` 결과와 대조한다. URL 일치 여부로 탭을 묶지 않으며, 디버거를 연결하거나 브라우저를 직접 제어하지도 않는다.
- 에이전트가 이 브라우저에서 직접 연 탭만 그룹으로 묶인다. 비공개 세션의 탭은 데몬이 목록에서 제외한다.
- 에이전트 탭을 묶으려면 `browser.extensions` 설정에 `extension/dist` 경로를 지정해 데몬이 실행하는 브라우저에 이 확장 프로그램을 올려야 한다. 사용자가 따로 띄운 브라우저에서는 데몬의 탭을 묶을 수 없다.
- `browser.extensions`는 Chromium이나 Chrome for Testing에서 동작한다. Google Chrome 정식 버전은 137부터 명령줄 인수인 `--load-extension` 옵션을 제거했다. 사용자가 직접 연 브라우저의 `chrome://extensions`에서 압축 해제된 확장 프로그램을 로드할 수는 있지만, 이 브라우저로는 데몬 소유의 탭을 묶을 수 없다. 설정된 실행 파일이 Google 브랜드 Chrome으로 보이면 `openside doctor`가 경고를 띄운다.

## 비밀번호 관리자

`openside doctor`는 표준 NativeMessagingHosts 디렉터리를 훑어 1Password나 Bitwarden 같은 알려진 호스트 매니페스트 파일이 있는지 확인한다. 이때 데몬 관리 프로필 디렉터리인 `<data_dir>/browser-profile/NativeMessagingHosts`도 함께 점검한다. 일반 사용자 브라우저 디렉터리에 매니페스트가 있다고 해서 해당 관리 프로필에서도 연동이 동작한다는 뜻은 아니다. Linux에서는 Flatpak 브라우저 경로와 Snap Chromium 공용 프로필 경로도 확인하며, 샌드박스 패키지는 패키지별 추가 설정이 필요할 수 있다. 매니페스트 파일이 존재한다고 해서 네이티브 호스트가 실제로 실행되거나 벤더 연결이 정상 동작한다고 단정할 수는 없다. doctor 명령은 매니페스트 내용을 읽거나, 파일을 복사·링크하거나, 호스트 프로그램을 실행하거나, 기존 연동 설정을 변경하지 않는다.

`tools/pwmgr-qa.sh all` 로 openside 와 비밀번호 관리자 확장을 함께 띄우는 검증을 재현한다(단계는 `tools/pwmgr-qa/run.mjs`).

**Bitwarden**: 스토어 빌드(확장 2026.9.3)를 로컬 Vaultwarden 서버에 연결해 순정 Chromium 152 와 패치 빌드에서 확인했다.
- 데몬이 띄운 브라우저(`browser.extensions`)에 openside 확장과 함께 로드되고 두 서비스 워커가 모두 동작한다.
- 에이전트 탭의 로그인 폼은 비어 있고 Bitwarden 이 임의로 채우지 않는다. 탭 원장에는 에이전트 탭만 나온다.
- openside 는 비밀번호 관리자 데이터를 읽지 않는다. 소스 정적 검사와, Bitwarden 잠금 해제·자동완성 뒤 openside 데이터 트리 검사에서 마스터 비밀번호와 항목 비밀번호가 나오지 않았다.
- 데몬이 관리하는 헤드리스 패치 브라우저에서 Bitwarden 로그인이 된다. 사용자 탭 자동완성과 폼 제출은 창을 띄운(headed) 순정 Chromium 개발 실행에서 확인했다. 순정 헤드리스 Chromium 은 Bitwarden 팝업을 그리지 못하지만 openside 때문은 아니다.

주의할 점:
- 비밀번호 관리자가 채운 사용자명·이메일 같은 필드는 일반 페이지 상태다. 자동완성이 에이전트 탭에 들어가면 에이전트가 REPL 로 읽을 수 있고 이벤트 로그에 남을 수 있다. 비밀번호 입력란은 Chromium 이 가려서 읽히지 않았다.
- `--user-data-dir` 를 준 Chromium 은 `<data_dir>/browser-profile/NativeMessagingHosts` 만 읽는다. Bitwarden 데스크톱 호스트 매니페스트를 그 디렉터리로 복사해야 한다(doctor 도 같은 안내를 한다).
- 알려진 문제: 데몬이 브라우저를 띄울 때 `XDG_CACHE_HOME`·`XDG_DATA_HOME` 을 프로필 안의 디렉터리로 바꾼다. Bitwarden `desktop_proxy` 는 데스크톱 앱 소켓을 `$XDG_CACHE_HOME/com.bitwarden.desktop/s.bw` 에서 찾으므로, 데몬이 관리하는 브라우저에서 Bitwarden 데스크톱 앱으로의 연결은 아직 되지 않는다(재현: `tools/pwmgr-qa.sh up && STAGES=native tools/pwmgr-qa.sh run`). 실제 데스크톱 앱과의 전체 핸드셰이크는 실행하지 않았다.

**1Password**: 설치와 로드까지만 확인했다(확장 8.12.41.1 이 openside 확장과 함께 로드되고 서비스 워커가 둘 다 동작하며, 설치 직후 자체 온보딩 페이지를 연다). 로그인, 자동완성, 데스크톱 앱 연동은 유료 계정과 데스크톱 앱이 필요해 `blocked-external` 이다.

사용자 할 일: 일회용 브라우저 프로필에 1Password 확장과 데스크톱 앱을 본인 계정으로 설치하고, 제품 안내에 따라 네이티브 연결이 되는지 확인한다.

## 테스트

```bash
pnpm -C extension lint
pnpm -C extension typecheck
pnpm -C extension test
pnpm -C extension build
cargo test -j 8 -p openside-e2e --test extension
```

- 배포 빌드의 콘텐츠 스크립트는 `https://mail.google.com/*`, `https://app.slack.com/*`, `https://*.slack.com/*` 에만 붙는다.
- 플래그 없이 빌드하면 이름이 `openside`로 지정되고 실제 호스트에만 매칭된다. `debugger` 권한은 선택 사항으로 남으며 루프백 호스트 권한도 포함되지 않는다. `OPENSIDE_EXTENSION_TEST_MATCHES=1`로 빌드하면 이름이 `openside (test build)`로 바뀌고, `http://127.0.0.1/*` 및 테스트 픽스처 경로인 `/gmail`, `/slack` 매칭이 추가된다. 이 빌드는 일회용 테스트 프로필에서만 쓰이며 `debugger` 권한과 루프백 접근 권한을 처음부터 부여받는다.
- E2E 테스트는 임시 디렉터리(`OPENSIDE_EXTENSION_OUTDIR`)에 있는 테스트 빌드를 `browser.extensions`로 가리키는 실제 테스트 데몬을 띄운다. 데몬은 독립된 임시 프로필 환경에서 stock Chromium을 `--headless=new` 옵션으로 실행한다. 이 테스트는 마지막 메시지뿐 아니라 이전에 선택한 Gmail 및 Slack 메시지가 출처 URL과 함께 전달되는지, 실제 CDP 마우스 클릭만 처리하고 웹페이지의 가짜 클릭은 무시하는지, Ask 응답이 스트리밍되는지, 정확한 ID로만 탭을 묶고 동일한 URL을 띄운 일반 사용자 탭은 묶지 않는지 검증한다.
- 헤드리스 환경에서는 Chrome의 권한 확인 팝업을 클릭할 수 없으므로 E2E 테스트에서 이 과정을 다루지 않는다. 대신 실제 Options 페어링 절차와 `Enable Agent Tabs` 버튼 동작은 그대로 검증한다.
- 테스트 빌드는 평소 쓰는 프로필에 설치하지 않는다.
