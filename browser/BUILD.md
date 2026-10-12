# BR-0 핀 빌드와 openside 오버레이

> 패치된 브라우저는 선택 사항이며, openside는 일반 Chromium이나 Chrome에 MV3 확장 프로그램을 설치해도 바로 동작한다. 패치 버전을 빌드하려면 별도의 Chromium 소스 코드 체크아웃과 넉넉한 디스크 공간 및 시간이 필요하다. 이 문서는 저장소에 없는 로컬 구동 스크립트(`br0.sh`, `br0-guarded.sh`)로 작업했던 과정을 기록한 것이므로 각자의 환경에 맞추어 진행해야 한다.

## 1. 기준과 현재 상태

- 베이스: 공식 `ungoogled-chromium-portablelinux` 태그 `155.0.8059.39-1`.
- Chromium: `155.0.8059.39`.
- 작업장: 환경 변수 `OPENSIDE_BROWSER_BUILD_DIR`가 가리키는 Chromium 빌드 디렉터리(openside 저장소 밖). 아래 명령은 모두 이 변수를 사용한다.
- 데몬 저장소에는 Chromium 소스를 넣지 않는다. `browser/`에는 오버레이와 얇은 patch만 둔다.

태그는 작업장에서 `git describe --tags --exact-match`로 확인했다. 실제 소스의 `chrome/VERSION`은 다음과 같다.

```text
MAJOR=155
MINOR=0
BUILD=8059
PATCH=39
```

BR-0 결과(2026-10-11, Main): 순수 베이스 빌드가 16:57:06 에 `[38702/38702] LINK ./chrome` 로 끝났고 `out/Default/chrome --version` 은 `Chromium 155.0.8059.39` 다. 그 뒤 `browser/build.sh` 로 P1 오버레이를 적용해 재빌드했다(exit 0, 95초). 측정값과 스모크는 [`docs/spikes/BR-0.md`](../docs/spikes/BR-0.md) 에 있다.

**BR-0 빌드가 돌고 있는 동안에는 `apply.sh`·`build.sh`를 실행하지 않는다.** `.domsub.stamp`는 domain substitution 완료 표시일 뿐 빌드 종료 표시가 아니다. 지금의 `portablelinux/build/src` 에는 P1–P7(`patches/0001`–`0005`)이 적용돼 있다. 순수 베이스가 다시 필요하면 fetch 단계부터 새로 푼다.

## 2. 공식 소스와 외부 드라이버

새 작업장에 베이스만 받는 명령은 다음과 같다. 기존 작업장에서는 다시 clone하지 않는다.

```bash
: "${OPENSIDE_BROWSER_BUILD_DIR:?set to the Chromium build workspace}"
ROOT="$OPENSIDE_BROWSER_BUILD_DIR"
PL="$ROOT/portablelinux"
git clone --branch 155.0.8059.39-1 --recurse-submodules \
    https://github.com/ungoogled-software/ungoogled-chromium-portablelinux "$PL"
```

공식 README의 지원 절차는 Debian Docker 기반이다. 이 머신의 native 절차는 Main이 만든 다음 외부 드라이버를 사용한다.

- `$OPENSIDE_BROWSER_BUILD_DIR/br0.sh`
- `$OPENSIDE_BROWSER_BUILD_DIR/br0-guarded.sh`

두 파일을 직접 읽고 아래 절차를 작성했다. 드라이버는 공식 clone이나 `browser/`에 포함된 파일이 아니다. 다른 머신에서 재현하려면 이 드라이버와 해당 호스트 환경을 별도로 확보해야 한다. Arch native 빌드는 이 머신에서 한 번 끝까지 성공했다(위 결과). 다른 Arch 머신은 확인하지 않았다.

다운로드는 공식 GitHub·Chromium·ungoogled·googleapis 등의 허용 출처만 사용한다. `sudo`·`pacman`·시스템 패키지 설치는 하지 않는다. 필요한 시스템 패키지가 없으면 해당 단계와 패키지를 `BLOCKED`로 기록한다. git commit·push·tag·stash·reset·checkout·restore도 하지 않는다.

## 3. 호스트 도구와 막힐 수 있는 지점

| 구분 | 요구사항·근거 |
| --- | --- |
| 베이스 제어 | `bash`, `git`, Python 3, 다운로드·압축 해제 도구 |
| 생성·빌드 | `ninja`, `clang`, `clang-format`, `node`, `gperf`, Go, `pkg-config`, `bison`, `flex`, `cmake` 등 |
| 다운로드 툴체인 | `scripts/shared.sh`가 Rust·Clang·sysroot를 받는다 |
| 리소스 가드 | `systemd-run`, `systemctl`, user bus, `nice`, `ionice` |
| 오버레이 적용 | `patch`, `sha256sum`, `flock`, `cp`, `cmp`, `mktemp`, `mv` |
| 오버레이 빌드 | 위 도구와 `du`, `grep`, `cat` |

`portablelinux/docker/build.Dockerfile`에는 Debian 개발 패키지 전체 목록이 있다. 이 머신에서는 `use_sysroot=true` 로 Chromium 공식 Debian sysroot 의 헤더·라이브러리를 써서 빌드했고, 호스트 개발 패키지를 추가로 설치하지 않았다.

`shared.sh`의 `setup_toolchain` 은 시스템 `gperf` 와 `/usr/bin/esbuild` 를 전제해 이 머신에서 실패한다. Main 은 다음으로 대체했다. gperf 는 Chromium CIPD `infra/3pp/tools/gperf/linux-amd64` `version:3@3.2` 를 `third_party/gperf/cipd` 에 풀었다. esbuild 는 GitHub `evanw/esbuild` `v0.28.2` 를 로컬에서 빌드해 devtools 기대 경로 `third_party/esbuild/esbuild` 에 두었다.

`shellcheck` 는 이 머신에 없어 두 스크립트에 실행하지 않았다.

네트워크·다운로드 manifest·캐시 압축 해제·디스크·user bus·툴체인·개발 헤더·GN 인자·컴파일 오류는 각각 실제 로그를 보고 분리한다. 인증 파일이나 사용자 브라우저 프로필을 읽어 해결하지 않는다.

## 4. Main의 BR-0 재현 절차

```bash
: "${OPENSIDE_BROWSER_BUILD_DIR:?set to the Chromium build workspace}"
ROOT="$OPENSIDE_BROWSER_BUILD_DIR"
export NINJA_JOBS=14
"$ROOT/br0-guarded.sh" fetch patch domsub args toolchain fixups gen build
```

`br0.sh`는 받은 단계 이름을 순서대로 실행한다. 실패하면 그 자리에서 종료한다.

| 단계 | 실제 드라이버 동작 |
| --- | --- |
| `fetch` | 공식 download manifest로 retrieve·unpack. `.downloaded.stamp` |
| `patch` | pruning 후 ungoogled·portablelinux patch 적용. `.patched.stamp` |
| `domsub` | domain substitution. `.domsub.stamp` |
| `args` | ungoogled `flags.gn` → portablelinux `flags.linux.gn` → `target_cpu`·`v8_target_cpu` |
| `toolchain` | 공식 다운로드 URL 보정, Rust·Clang·sysroot·호스트 도구 연결 |
| `fixups` | devtools TypeScript `error TS9025`(`--isolatedDeclarations`) 를 피하려고 `ARIAUtils.ts`·`Widget.ts` 두 매개변수에 `\| undefined` 를 명시 |
| `gen` | `out/Default/gn` bootstrap, `gn gen out/Default --fail-on-unused-args` |
| `build` | `ninja -j "$NINJA_JOBS" -C out/Default chrome chromedriver` |

`args` 단계는 `$ROOT/openside-args.gn`이 있을 때 마지막에 덧붙인다. 현재 P1의 `browser/args.gn`에는 설명만 있다. `enable_openside` 같은 미선언 GN 인자를 추가하지 않았다.

기본 로그는 openside 저장소 루트의 `.goal/br0-build.log`다. `BR0_LOG`로 경로를 바꿀 수 있다. 단계 stamp만 보고 바이너리가 성공했다고 판단하지 않는다.

장시간 빌드는 반드시 다음 가드를 사용한다.

```text
systemd-run --user --scope -p MemoryMax=24G -p CPUWeight=20
    nice -n 19 ionice -c3 ...
```

병렬도는 `<=14`다. `br0-guarded.sh`는 15초마다 `MemAvailable`을 확인한다. 8 GiB 미만이면 scope에 `SIGSTOP`, 12 GiB 초과이면 `SIGCONT`를 보낸다. user bus 환경이 없으면 `XDG_RUNTIME_DIR`·`DBUS_SESSION_BUS_ADDRESS`의 fallback을 설정한다. `MemoryMax`는 자식·cache도 포함하므로 호스트 RAM에 여유가 있어도 scope가 OOM으로 종료될 수 있다.

작업장 합계는 150GB 이하로 유지한다. Main의 BR-0 드라이버에는 디스크 watchdog이 없으므로 별도 확인이 필요하다.

```bash
du -sh "$ROOT"
```

Main의 BR-0 대기 상한은 6시간이다. 제한 안에 끝나지 않으면 실제 상태를 기록한다. 이 문서는 완료 결과를 대신하지 않는다.

## 5. 오버레이 적용과 증분 빌드

핀 베이스의 첫 빌드가 끝난 소스에만 적용한다. 아래 명령은 openside 저장소 루트에서 실행한다.

```bash
: "${OPENSIDE_BROWSER_BUILD_DIR:?set to the Chromium build workspace}"
./browser/apply.sh                          # 오버레이만 적용
OPENSIDE_NINJA_JOBS=14 ./browser/build.sh   # 적용 + args.gn marker + 가드 빌드
./browser/guarded-build.sh chrome           # 적용 없이 가드 빌드만
```

세 스크립트는 인자가 없으면 `$OPENSIDE_BROWSER_BUILD_DIR/portablelinux/build/src`를 쓴다. `apply.sh`·`build.sh`에는 소스 경로를 인자로 줄 수 있다. `guarded-build.sh`는 `OPENSIDE_CHROMIUM_SRC`로 바꾼다. 베이스 다운로드·툴체인 bootstrap·최초 GN 생성은 하지 않는다.

`guarded-build.sh [TARGET...]`의 가드는 다음과 같다. 기본 target은 `chrome`이다. 인자는 ninja target 이름(`^[[:alnum:]_./:+][[:alnum:]_./:+-]*$`)만 받는다. `-j99`·`-C dir` 같은 옵션은 아무것도 실행하기 전에 거절하므로 가드의 job 상한을 우회할 수 없다.

- `systemd-run --user --scope -p MemoryMax=24G -p CPUWeight=20 nice -n 19 ionice -c3`
- `OPENSIDE_NINJA_JOBS`: 기본 8, 허용 범위 `1..14`
- 시작 전에 `MemAvailable` 8 GiB 이상을 기다린다.
- 15초마다 메모리를 확인한다. 8 GiB 미만이면 `SIGSTOP`, 12 GiB 초과이면 `SIGCONT`.
- 120초마다 `OPENSIDE_BROWSER_BUILD_DIR` 전체를 `du`로 잰다. 150 GiB를 넘으면 빌드 scope를 종료한다.
- `OPENSIDE_BUILD_LOG`를 주면 출력을 그 파일에 덧붙인다. 마지막 줄에 exit code와 경과 초를 남긴다.

빌드한 브라우저는 런타임이 다음 순서로 찾는다(`crates/openside-config/src/discovery.rs`의 `discover_browser`): `browser.executable` → 환경 변수 `OPENSIDE_BROWSER` → `browser.bundle_dir` 아래의 `chrome`·`chromium` → `PATH` → 플랫폼 기본 위치.

## 6. apply.sh와 rebase.sh

핀 버전은 `browser/CHROMIUM_VERSION` 한 곳에 둔다. `apply.sh`는 소스의 `chrome/VERSION`이 이 값과 다르면 거부한다.

`apply.sh`는 여러 번 실행해도 된다. 적용한 patch의 사본을 `SRC/.openside-overlay/applied/`에 남긴다.

1. 버전·`.domsub.stamp`를 검사하고 `.openside-overlay.lock`을 잠근다.
2. series 앞부분에서 기록과 내용이 같은 patch는 건너뛴다.
3. 내용이 바뀌었거나 빠진 patch는 그 뒤 patch와 함께 기록된 사본으로 역적용한다.
4. `src/.`를 소스에 복사한다.
5. 남은 patch를 순서대로 `--fuzz=0` dry-run 후 적용한다. 하나씩 기록한다.
6. 처음 실패한 patch에서 멈춘다. 파일명, 순번, 적용된 개수, `patch` 출력을 보여 준다.

멈춘 뒤에도 소스와 기록은 일치한다. 단, 이 보장은 patch가 충돌해 멈춘 경우에 한한다. 적용 도중 프로세스가 죽거나 디스크가 차면 기록과 소스가 어긋날 수 있다(crash-safe 하지 않다). patch를 고치고 다시 실행하면 이어서 적용한다. 역적용 dry-run이 실패하면 소스가 기록과 달라진 것이므로 새 핀 베이스를 준비한다. `git reset`은 쓰지 않는다.

새 핀으로 옮기기 전에는 `rebase.sh`를 실행한다.

```bash
./browser/rebase.sh /path/to/new/chromium/src
```

series가 건드리는 파일만 임시 디렉터리로 복사해 patch를 순서대로 적용한다. 새 소스는 바꾸지 않는다. patch마다 `clean`·`offset`·`fuzz`·`conflict`를 출력한다. `conflict`이면 실패한 hunk를 함께 보여 준다. 충돌이 나도 나머지 patch를 계속 검사한다. 모두 `clean`일 때만 exit 0이다.

## 7. patch 범위(P1–P11)

| patch | 내용 |
| --- | --- |
| `0001` P1 | `//chrome/browser/openside` 연결. 전용 프로세스의 `Target.createTarget`을 agent 소유·background로 강제 |
| `0002` P1 | 시작 시 AgentGateway 실행(DevTools handler보다 먼저, 그래서 discovery가 debugging endpoint보다 먼저 생긴다). 모든 탭에 gateway 탭 ID 부여 |
| `0003` P2 | agent 출처(agent 탭과 팝업 후손)의 `ActivateContents`·foreground 팝업·fullscreen 차단, agent 브라우저의 `Page.bringToFront` view focus 생략, agent 브라우저에서 모든 창의 `Browser.setWindowBounds`·`setContentsSize` 거절 |
| `0004` P6 | 탭 모달 JavaScript 다이얼로그 열림·닫힘 hook |
| `0005` P7 | `navigator.webdriver`·`HeadlessChrome` UA·자동화 infobar 제거([NOTICE](NOTICE.md)) |
| `0006` P9 | openside 빌드에서 네이티브 세로 탭 설정의 기본값을 켬(on)으로 변경한다. 사용자가 값을 직접 지정하지 않은 기존 프로필에도 적용되며, 명시적인 사용자 설정이 항상 우선하므로 원하면 끌 수 있다. 분할 화면과 북마크 사이드 패널은 고정된 Chromium 버전에 이미 포함되어 있다. |
| `0007` P8 | 자격 증명을 담지 않은 루프백 데몬 웹 UI URL이 명시적으로 설정되어 있으면, 일반(비 시크릿) 새 탭 페이지(NTP)에서 해당 웹 UI를 연다. 게이트웨이가 활성화된 프로필에는 `ask` 및 `routines` 옴니박스 키워드를 등록한다. 기존 키워드와 기본 검색 엔진은 그대로 유지되며, 입력한 질문을 자동으로 전송하지 않는다. |
| `0008` P10 | 네이티브 자동완성 추출과 채우기 동작을 수신 렌더러의 document에 바인딩한다. `PasswordAutofillAgent`는 필드 정보를 추출할 때와 채우기 직전(`FillFieldForOpenside` Mojo 호출 시점)에 노드의 document, document token, 기대 오리진(expected origin)을 각각 검증한다. 기존 채우기 경로 역시 다른 document의 노드는 거부한다. |
| `0009` P2 | 창 내 모든 탭이 agent 출처인 경우 `BrowserView::Show`는 창을 비활성 상태(`ShowInactive`)로 띄우고 `Activate`는 아무 동작도 하지 않으며, `focus.decided` 이벤트를 전송한다. `focused:true`인 `chrome.windows.create`/`chrome.windows.update`, CDP로 생성한 창, 그리고 이로 인해 발생하는 Wayland 활성화 요청처럼 탭 수준 검사를 건너뛰는 창 활성화도 차단한다. agent 출처 페이지에서 시작된 다운로드는 다운로드 버블을 자동으로 열지 않는다. 사용자 탭이 포함된 창은 기존대로 동작한다. |
| `0010` P2 | 활성화 권한은 요청 주체에 따라 결정된다. `content::ScopedProtocolActivation`(신규 `content/public/browser/openside_protocol_activation.{h,cc}`)으로 `Target.activateTarget`과 `Page.bringToFront`를 표시하여 agent 브라우저의 포커스 중재자가 모든 탭에 대해 해당 요청을 거부하도록 한다. 탭이 모두 agent 출처인 창에 `chrome.windows.update`로 포커스, 상태, 크기/위치, `drawAttention`을 요청하면 어떤 작업도 실행하기 전에 거부된다. 사용자 탭이 포함된 창은 확장 프로그램, 즉 사용자가 관리할 수 있도록 유지된다. 탭에서 나가는 모든 DevTools 세션(`ChromeDevToolsManagerDelegate::ClientDetached`)도 함께 감지해 보고하므로, 세션 종료 시 리셋되어 지워진 agent 탭 오버라이드를 게이트웨이가 다시 적용할 수 있다. |

게이트웨이 디렉터리는 `--openside-gateway-runtime-dir` 스위치로 지정하며, 스위치가 없을 때만 `XDG_RUNTIME_DIR`을 사용한다. 컨트롤러와 동일한 규칙에 따라 사용자가 소유한 권한 0700의 실제 디렉터리여야 한다. 브라우저는 심볼릭 링크를 따라가지 않고 이 디렉터리를 한 번 열어 파일 디스크립터를 확보한 뒤, 그 디스크립터를 기준으로 소켓, 토큰, 탐색 파일이 포함된 `openside/browser/<instance>`를 생성한다. 종료할 때도 경로를 다시 해석하지 않고 같은 디스크립터로 지운다. `agent_gateway.cc`의 내부 DevTools 클라이언트는 탭이 어디에 표시되든 모든 agent 탭의 레이아웃을 디바이스 배율(device scale factor) 1의 1440x900으로 구성한다. 컨트롤러는 agent 탭에 오버라이드를 직접 보내지 않으므로 브라우저가 유일한 소유권을 갖는다. 다른 클라이언트가 연결을 끊었을 때, 크기가 바뀌었을 때, 그리고 매 캡처 직전에 브라우저가 오버라이드를 다시 적용한다. 크기는 그대로 두고 배율만 바꾼 다른 클라이언트의 오버라이드는 그 클라이언트가 연결을 끊거나 다음 캡처가 시작될 때 되돌린다. 사용자 탭은 에뮬레이션하지 않으며, agent 소유에서 벗어난 탭은 에뮬레이션을 즉시 해제한다.

P3와 P5는 `agent_gateway.cc`에 구현되어 있다. agent 탭은 "Agent Tabs" 탭 그룹에 들어가 1440x900으로 레이아웃이 잡히며, 백그라운드 탭은 보이지 않는 캡처기를 통해 렌더링을 유지한다. 게이트웨이는 Linux 환경에서 `--openside-gateway-instance-id` 플래그를 넘겼을 때만 활성화된다(`BUILD.gn`에서도 `is_linux`일 때만 빌드에 포함). 프로토콜 및 구현 차이는 [gateway-protocol](specs/gateway-protocol.md) §10을 참고한다. `agent_gateway.cc`에는 `browser.status`, `autofill.fields`, `autofill.fill`도 함께 구현되어 있다([gateway-protocol](specs/gateway-protocol.md#11-authenticated-native-status-and-autofill) §11). 자동완성 핸들은 무작위로 생성되는 1회용 토큰으로 60초 뒤 만료되며, 에이전트 소유 탭, 실행 스코프, 메인 프레임 document, 탭의 프라이머리 페이지 세대(generation)에 묶인다. 따라서 BFCache에서 복원된 페이지는 이전 핸들을 다시 사용할 수 없다. `browser.status`는 네이티브 자동화 정보 표시줄(infobar) 개수, 네이티브 UX 플래그와 함께, 관리 프로필에 한해 `{ask, routines}` 불리언 값만 반환하며, 저장된 검색 템플릿 URL은 일절 반환하지 않는다. P8–P11의 지원 기능과 한계는 [native-ui](specs/native-ui.md)에 정리되어 있다. 네이티브 Ask 사이드 패널은 구현되어 있지 않으며, 확장 프로그램 사이드 패널이 계속해서 Ask UI 역할을 맡는다.

marker는 인증이 아니다. 전용 브라우저 하나에 scope 하나만 붙인다. 사용자 프로필에는 쓰지 않는다. 데몬은 세션들이 같이 쓰는 브라우저를 고정 scope `opensided`로 띄우고, 세션별 소유권은 Rust 장부가 나눈다([gateway-protocol](specs/gateway-protocol.md) §10).

## 8. 검사

```bash
export OPENSIDE_PATCHED_BROWSER="$OPENSIDE_BROWSER_BUILD_DIR/portablelinux/build/src/out/Default/chrome"
cargo test -j 8 -p openside-browser --test patched
```

`OPENSIDE_PATCHED_BROWSER`가 없으면 각 테스트는 `SKIP patched:` 이유를 출력하고 끝난다. 테스트는 `127.0.0.1` 목업, 임시 프로필, 권한 0700인 임시 runtime 디렉터리만 쓴다. `shellcheck`는 이 머신에 없어 실행하지 않았다.

## 9. 출처

- [공식 portablelinux README](https://github.com/ungoogled-software/ungoogled-chromium-portablelinux/blob/155.0.8059.39-1/README.md).
- [핀 shared.sh](https://github.com/ungoogled-software/ungoogled-chromium-portablelinux/blob/155.0.8059.39-1/scripts/shared.sh), [핀 build.Dockerfile](https://github.com/ungoogled-software/ungoogled-chromium-portablelinux/blob/155.0.8059.39-1/docker/build.Dockerfile).
- [gateway-protocol](specs/gateway-protocol.md), [focus-arbiter](specs/focus-arbiter.md), [tab-ownership](specs/tab-ownership.md).
- [NOTICE](NOTICE.md): xplorer 라이선스 표기의 차이와 `unverified` 상태. 설계만 참고했으며 코드는 복사하지 않았다.

## 10. 패치 브라우저 스모크

바이너리를 확인하고 단일 명령 스모크 및 네이티브 Vault 채우기 테스트를 실행한다.

```bash
export OPENSIDE_PATCHED_BROWSER="$OPENSIDE_BROWSER_BUILD_DIR/portablelinux/build/src/out/Default/chrome"
"$OPENSIDE_PATCHED_BROWSER" --version
./tools/patched-smoke.sh
OPENSIDE_PATCHED_BROWSER="$OPENSIDE_PATCHED_BROWSER" \
  cargo test -j 8 -p opensided patched_vault_login_uses_native_autofill_without_cdp_secret_values -- --nocapture
```

`tools/patched-smoke.sh`는 바이너리가 없거나 실행 권한이 없으면 실행을 거부한다. 패치가 적용된 실제 통합 테스트 스위트를 실행해 P1–P11 항목별 PASS/FAIL 표를 출력하며, 하나라도 실패하면 0이 아닌 종료 코드를 반환한다. 또한 스크립트 고유의 임시 디렉터리 경로를 포함하는 프로세스만 정리한다. P8 및 P11 UI 점검에는 실제 `web/dist` 빌드가 필요하다. 스모크 테스트는 바이너리를 명시적으로 요구하므로 브라우저가 없어서 건너뛴 테스트를 표에서 성공(PASS)으로 계산하지 않는다. 브라우저 설정 없이 Rust 테스트를 직접 호출한 경우에만 테스트를 건너뛸 수 있다.

`apply.sh`는 적용된 각 패치를 기록하며, dry-run이 한 번이라도 실패하면 즉시 중단하고 후속 패치를 진행하지 않는다. 고정된 Chromium 버전을 직접 수정하지 않고 새 버전을 점검하려면 `browser/rebase.sh /path/to/new/chromium/src`를 실행한다. 이 스크립트는 패치 시리즈의 모든 항목을 영향받는 소스 파일의 임시 복사본에 대조해 테스트하며, clean/offset/fuzz/conflict 결과를 보고한다. 모든 패치가 clean 상태일 때만 0으로 종료한다. Chromium 핀을 변경하기 전에 먼저 이 스크립트를 실행하고, 리소스 가드를 거친 증분 빌드를 수행한 뒤 스모크 테스트를 다시 실행한다.

