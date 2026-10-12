# 플랫폼 지원

| 플랫폼 | 상태 | 근거 |
|---|---|---|
| Linux | 지원한다. 개발과 테스트 모두 Linux 환경에서 진행했고, CI는 Linux에서 `tools/gate.sh`를 실행한다 | 헤드리스 브라우저 컨트롤러 테스트는 디스플레이 종속성이 없음을 보이려고 일부러 잘못된 디스플레이 엔드포인트를 지정한 채 Wayland와 X11 세션 환경 모두에서 통과했다(특정 윈도우 매니저 호환성 인증은 아님). B-5 포커스 테스트는 실제 Wayland 컴포지터 세션에서 통과했다 |
| macOS | 빌드 및 핵심 런타임 지원. `x86_64-apple-darwin` 및 `aarch64-apple-darwin` 크로스 컴파일 점검을 통과했고, macOS 26.7.1 x86_64(Quickemu Tahoe VM) 환경에서 `cargo build --workspace -j 4` 무경고 빌드와 `cargo test --workspace --no-run -j 4` 전체 테스트 바이너리 링크를 확인했다. 데몬, CLI, 볼트/키링, 세션, 릴레이, 설정, 임포트 테스트가 모두 통과했으며 `opensided`·`openside`와 Keychain 볼트 초기화·추가·잠금 해제 E2E 동작을 검증했다. 브라우저 연동, 사이드카, launchd 동작은 `unverified` | 네이티브 검증(2026-10-13), 아래 "macOS 네이티브 검증" 절 |
| Windows | 런타임 미지원. `x86_64-pc-windows-gnu` 와 `x86_64-pc-windows-msvc` 크로스 컴파일 점검 통과(msvc 는 `cargo-xwin` 으로 실제 Microsoft CRT/SDK 헤더를 받아 점검) | Unix 소켓 전송, 프로세스 그룹·신호, bwrap에 의존하며 런타임 구현은 여전히 Unix 전용이다. Windows 환경에서는 해당 경로 진입 시 명시적인 미지원 오류를 반환한다. `openside doctor` 는 `daemon.transport` 를 `fail` 로 알리고, `openside service` 는 systemd unit 을 만들지 않고 미지원 오류를 낸다 |

설치는 [INSTALL.md](INSTALL.md), 경로 규칙은 [CONFIGURATION.md](CONFIGURATION.md#파일-위치와-경로) 를 본다.

## 플랫폼 의존 코드

`crates/` 에서 `cfg(unix)`, `cfg(target_os`, `cfg(windows)`, `std::os::unix`, `nix::` 를 찾아 정리했다(2026-10-11, 테스트 파일 제외).

| 위치 | 의존 | Unix 밖에서의 동작 |
|---|---|---|
| `openside-browser/src/browser.rs` | Unix 환경에서는 `nix`(`kill`, `killpg`, `getpgid`, `prctl`), Chromium `process_group(0)` 실행, 프로필 디렉터리 0700 권한 사용(`nix`는 `cfg(unix)` 조건부 의존). Linux 전용 게이트웨이 FD/Unix 소켓 전송 경로 포함. Linux에서는 자식 프로세스의 `pre_exec` 훅에 `prctl(PR_SET_PDEATHSIG, SIGKILL)`을 설정하여, 스폰 스레드가 소멸자를 실행하지 못하고 비정상 종료(abort, SIGKILL, 하네스 타임아웃)되더라도 Chromium이 함께 종료되도록 보장한다(데몬 재연결이 필요한 detached 실행은 의도적으로 감독 대상에서 제외되며, 테스트는 `die_with_spawner` 설정 플래그로 이를 명시적으로 활성화한다) | `cfg(not(unix))` 환경에서 브라우저 실행·접속·종료·정리 시도는 `Browser execution is unsupported on this platform` 오류로 거부한다. 필수 게이트웨이(`required = true`) 설정 시 Linux 전용 오류를 반환하며, 선택적 게이트웨이는 CDP 전용으로 동작한다 |
| `openside-browser/src/uploads.rs`, `downloads.rs`, `spaces.rs` | Linux와 macOS에서는 Unix 파일 디스크립터 기반 no-follow 방식(`rustix` `openat`)으로 업로드 및 다운로드 스테이징을 처리하고 전용 디렉터리 권한을 적용한다. 이들 경로는 `cfg(unix)`로 제한된다. | 비Unix 환경에서는 경로 기반 대체 연산이나 권한 완화를 허용하지 않고 명시적으로 `Unsupported` 에러를 반환한다. |
| `openside-cli/src/client.rs` | `tokio::net::UnixStream` 기반 로컬 소켓 통신은 `cfg(unix)`로 한정, 기본 소켓은 `$XDG_RUNTIME_DIR/openside/opensided.sock` | `cfg(not(unix))` 환경에서 로컬 데몬 전송 시도는 `Unix daemon transport is unsupported on this platform` 오류를 반환한다. 인증 루프백 TCP 클라이언트 코드는 컴파일되나 Windows 데몬 전송 구현체는 아니다 |
| `openside-cli/src/cursor.rs` | `std::os::unix::fs::DirBuilderExt` 기반 비공개 디렉터리 잠금 및 영속화(0700 디렉터리, 0600 커서)는 `cfg(unix)`로 한정 | `cfg(not(unix))` 환경에서는 권한을 완화해 저장하지 않고, 미지원 개인 재개 커서(resume-cursor) 영속화 요청에 대해 명시적 오류를 반환한다 |
| `opensided/src/engine.rs` | `serve` 는 `cfg(unix)`: Unix 소켓 bind, 0700/0600 권한, SIGTERM 처리. data 디렉터리 0700, lock 파일 0600. 소켓 상위 디렉터리는 기존 디렉터리라면 0700 이면서 소유자가 데몬의 euid 여야 한다(다른 사용자가 미리 만든 `/tmp/openside-<uid>` 는 거부) | `cfg(not(unix))` `serve` 는 `Unix daemon transport is unsupported on this platform` 오류. TCP 만 여는 대체 경로는 없다 |
| `opensided/src/tools/shell.rs` | `bash` 실행은 `cfg(unix)`: `process_group(0)`, 시간 초과 시 `nix::sys::signal::killpg`, bwrap 인자 | `cfg(not(unix))` 는 `Shell execution is unsupported on this platform` 로 거부 |
| `opensided/src/tools/browser.rs`, `opensided/src/tickets.rs` | 스크린샷·audit 파일을 0600 + `O_NOFOLLOW`(`nix::libc`)로 열고 디렉터리 0700 | 권한 지정 없이 연다 |
| `openside-vault/src/lib.rs`, `audit.rs` | Vault 파일·audit 파일 0600 | 권한 지정 없이 연다 |
| `openside-vault/src/platform/secret_service.rs` | `cfg(target_os = "linux")`: `unlock = "secret-service"`(또는 Linux의 `auto`)는 세션 D-Bus의 Secret Service(`org.freedesktop.secrets`)에서 래핑 키를 조회하거나 생성함. 잠긴 컬렉션은 사용자 프롬프트 없이 `Unlock`에 성공할 때만 사용하며, 프롬프트가 필요하면 볼트를 열지 않음. `keyring_reachable`은 D-Bus 버스 이름의 소유자 유무 및 활성화 가능 여부만 확인함 | 다른 플랫폼에서는 해당 백엔드를 지원하지 않음 |
| `openside-vault/src/platform/native.rs` | `cfg(any(target_os = "macos", target_os = "windows"))`: `unlock = "native"`(또는 해당 환경의 `auto`)는 `keyring` 크레이트를 통해 macOS Keychain 또는 Windows Credential Manager에 래핑 키를 보관함 | Linux 빌드에서는 컴파일 제외. Windows(`x86_64-pc-windows-msvc`) 빌드는 `tools/check-platforms.py` 컴파일 검사를 통과했고, macOS 는 SDK 가 없어 컴파일 검사를 하지 못했다. 둘 다 실행은 미검증(`unverified`) |
| `openside-agent-sidecar/src/lib.rs`, `process.rs`, `connection.rs` | `process` 모듈 전체가 `cfg(unix)`: 세션별 Unix 소켓(`UnixListener`), `killpg`, 0700 디렉터리. `nix` 는 `cfg(unix)` 의존 | `cfg(not(unix))` `AgentRuntime` 은 세션을 열지 못하고 오류를 돌려준다 |
| `openside-config/src/paths.rs` | runtime 대체 경로 `openside-<uid>`(`nix::unistd::getuid`) | 임시 디렉터리`/openside` |
| `openside-config/src/discovery.rs` | 실행 비트 검사(`cfg(unix)`), Windows `PATHEXT` 후보(`cfg(windows)`), macOS·Windows 브라우저 위치 | Windows 는 실행 비트 없이 파일 존재만 본다 |
| `openside-config/src/doctor.rs` | 디렉터리 소유자 uid·0700 검사, `sandbox.bwrap`·`remote.*` 점검 | 읽기 전용 여부만 검사. `sandbox.bwrap`·`remote.*` 대신 `daemon.transport` 를 `fail`("daemon transport is unsupported on this OS")로 보고한다 |
| `openside-e2e/src/lib.rs` | 크레이트 전체 `#![cfg(unix)]`, `nix` 의존성은 `cfg(unix)`로 조건화. `set_child_subreaper`는 `cfg(target_os = "linux")`, `agent/.venv/bin/python` 경로 사용 | Unix 밖에서는 빈 크레이트로 컴파일된다. 크레이트 타깃이 컴파일된다고 해서 Windows에서 E2E 테스트를 실행할 수 있는 것은 아니다. macOS 에서는 subreaper 없이 실행 |
| `openside-import/src/crypto.rs`, `openside-import/src/store.rs` | Chrome·Chromium 쿠키 복호화 키 파생. `cfg(target_os = "macos")`는 Keychain 비밀번호와 1003회 반복, 그 밖에는 Linux 방식(`v10` 고정 비밀번호 `peanuts`, `v11` 키링 비밀번호, 1회). 키링 명령 프로세스 그룹과 심볼릭 링크 안전 DB 경로는 `cfg(unix)`로 한정 | Linux는 단위·CLI·E2E 테스트로 검증. macOS는 키 파생 경로가 컴파일되나 `unverified`. Windows는 DPAPI·AES-GCM 쿠키 복호화를 지원하지 않으며, 키링 명령 및 안전 DB 경로 진입 시 기존 오류 카테고리로 거부한다 |
| `extension/`(MV3), `openside-config/src/doctor.rs`(native messaging 점검) | sidePanel·tabGroups·omnibox 를 지원하는 순정 Chromium 126 이상. OS 별 NativeMessagingHosts 경로: Linux 는 XDG 사용자·시스템 디렉터리, macOS 는 `~/Library/Application Support` 와 시스템 디렉터리, Windows 는 레지스트리 키(문서에만 적고 조회하지 않음) | 확장 빌드는 플랫폼과 무관하지만, 실행 확인은 Linux headless Chromium E2E 뿐이다. macOS·Windows 실제 브라우저 동작은 `unverified`. Windows 에서 doctor 는 레지스트리 키 위치만 안내한다 |

Linux 전용 기능:

- bubblewrap 샌드박스(`bash` 도구). Linux 커널 namespace 기반이다.
- `openside-e2e` 의 child subreaper(`prctl`).
- `openside-memory` 의 소켓 감시 테스트(`tests/no_network.rs`). `/proc/self/fd` 를 읽으며, 다른 플랫폼에서는 이 테스트가 빠지고 의존성 그래프 검사만 돈다.
- 볼트의 Secret Service 백엔드(`unlock = "secret-service"` 및 Linux 환경의 `auto` 선택). 격리된 `dbus-daemon` 환경의 모의 Secret Service를 대상으로 테스트를 실행한다(`crates/openside-vault/tests/secret_service.rs`, `dbus-daemon` 미설치 시 건너뜀). 또한 개발 머신의 gnome-keyring 환경에서 직접 수동 검증(`openside doctor`, `openside vault add`/`items`/`rm`)을 마쳤다. KWallet 대상 검증은 진행하지 않았다.
- `openside memory model fetch`의 모델 설치: 세대 디렉터리를 가리키는 심볼릭 링크(`std::os::unix::fs::symlink`)를 원자적 rename으로 교체하는 방식은 Unix 전용이다(macOS 포함). 비-Unix 환경에서는 설정을 읽거나 모델 파일을 다운로드하기 전에 미지원 오류(Unsupported I/O)를 반환하며 작업을 즉시 중단한다.
- 패치한 Chromium 의 AgentGateway(`browser/`, `browser.gateway`). 패치 overlay 는 Linux 에서만 gateway 를 빌드하고, Rust 클라이언트도 Linux 가 아니면 gateway 없이 CDP 만 쓴다(`required = true` 면 실패). 클라이언트는 `/proc/self/fd` 로 검증한 디렉터리를 고정해 파일을 연다. 다른 플랫폼과 stock Chromium 은 CDP 경로를 그대로 쓴다.
- GOAL 3 시나리오 스위트(`docs/specs/e2e-scenarios.md`, 실행: `tools/e2e-scenarios.sh`)는 `openside-e2e`의 다른 부분과 마찬가지로 Unix 환경이 필요하다. 태스크 진행 중 Chromium SIGKILL 복구 시나리오와 컴포지터 포커스 테스트는 `cfg(target_os = "linux")`로 한정된다. 포커스 테스트는 컴포지터 환경을 임의로 단정하지 않고 `swaymsg`나 `hyprctl`을 확인한다. 둘 다 응답하지 않으면 사유를 출력하고 건너뛰며, 시나리오 실행기는 건너뛰거나 누락된 테스트를 실패로 처리한다.

## macOS·Windows 컴파일 점검

`tools/check-platforms.py`는 전체 워크스페이스와 모든 Cargo 타깃을 대상으로 크로스 컴파일 점검을 수행한다. 이미 설치된 타깃 std를 우선 사용하며, 없는 경우 `rust-src` 컴포넌트가 설치된 로컬 nightly 툴체인을 탐색해 `build-std`(`-Zbuild-std=std,panic_abort`) 모드로 검사한다. `x86_64-pc-windows-msvc` 는 `PATH` 에 `cargo-xwin` 이 있거나 `OPENSIDE_XWIN` 이 `cargo-xwin` 실행 파일(또는 `bin/cargo-xwin` 이 있는 설치 루트)을 가리키면 `cargo xwin check` 로 점검한다. 없으면 plain `cargo` 로 시도하며 C 의존성의 Microsoft SDK 헤더가 없어 `BLOCKED-EXTERNAL` 이 된다. 이 스크립트는 Rust 툴체인, std 라이브러리, C 컴파일러나 SDK를 임의로 내려받거나 설치하지 않는다(`cargo-xwin` 은 처음 실행할 때 `XWIN_CACHE_DIR` 로 CRT/SDK 를 직접 받는다). 타깃 std나 `rust-src`, 또는 교차 검증에 필요한 C/SDK 선행 도구가 없으면 `BLOCKED-EXTERNAL` 상태와 함께 `User action:` 안내를 출력하며, 실제 Rust 컴파일 오류가 발생할 때만 실패로 판정한다. 호출 환경에 설정된 컴파일러 및 헤더 환경 변수는 그대로 계승된다.

`tools/check-platforms.py`는 `cargo -j 4`로 빌드 검사를 수행한다. Cargo가 컴파일러를 자동으로 찾는 네이티브 러너와 달리, Linux 크로스 컴파일 환경에서는 상속된 `CC`, `AR`, `CFLAGS` 환경 변수 등을 통해 타깃 컴파일러와 아카이버, SDK 헤더가 사전에 구성되어 있어야 C 네이티브 종속성을 빌드할 수 있다. 과거 `ring`이나 `libsqlite3-sys`에서 발생한 컴파일러 및 SDK 누락 오류는 이러한 외부 환경 구성 블로커에 해당하며, 환경 문제로 빌드가 중단된 상태를 두고 Rust 코드 자체의 컴파일이 성공했다고 볼 수는 없다.

재현 환경 구성 예시:

```bash
# macOS 타깃 (Darwin libc 헤더 디렉터리 지정)
export DARWIN_HEADERS="/path/to/darwin-libc-headers"
CC_x86_64_apple_darwin=clang AR_x86_64_apple_darwin=llvm-ar CFLAGS_x86_64_apple_darwin="-isystem $DARWIN_HEADERS" \
    cargo +nightly-2026-08-08 check -Zbuild-std=std,panic_abort --target x86_64-apple-darwin --workspace --all-targets --locked -j 8

# Windows GNU 타깃 (llvm-mingw 툴체인 루트 지정)
export MINGW_ROOT="/path/to/llvm-mingw"
export PATH="$MINGW_ROOT/bin:$PATH"
CC_x86_64_pc_windows_gnu=x86_64-w64-mingw32-clang \
AR_x86_64_pc_windows_gnu=llvm-ar \
CARGO_TARGET_X86_64_PC_WINDOWS_GNU_LINKER=x86_64-w64-mingw32-clang \
    cargo +nightly-2026-08-08 check -Zbuild-std=std,panic_abort --target x86_64-pc-windows-gnu --workspace --all-targets --locked -j 8
```

교차 검증에 사용한 헤더와 툴체인 출처:
- Darwin C 의존성(`ring 0.17.14`, `libsqlite3-sys 0.35.0`, `rquickjs-sys 0.14.0`): 로컬 Clang/llvm-ar 및 GitHub의 공식 [Zig 0.15.1 소스](https://github.com/ziglang/zig/tree/0.15.1/lib/libc/include/any-macos-any)에 포함된 순정 Darwin libc 헤더를 사용했다. 임의 제작한 모조 헤더나 호스트 C 헤더 대체, C 빌드 no-op 처리는 사용하지 않았다.
- Windows GNU: GitHub의 공식 [llvm-mingw 20261006 릴리스](https://github.com/mstorsjo/llvm-mingw/releases/tag/20261006)를 임시 디렉터리에 압축 해제해 사용했다.

점검 결과 (`tools/check-platforms.py`):
- `x86_64-apple-darwin`: **PASS** (미사용 변수/데드 코드 경고 외 컴파일 통과)
- `aarch64-apple-darwin`: **PASS**
- `x86_64-pc-windows-gnu`: **PASS** (런타임 미지원 경로로 인한 데드 코드 경고 외 컴파일 통과)
- `x86_64-pc-windows-msvc`: **PASS**. `cargo-xwin 0.23.1` 이 실제 Microsoft CRT/SDK 헤더를 받아(`XWIN_CACHE_DIR`) `ring 0.17.14`, `libsqlite3-sys 0.35.0`, `rquickjs-sys 0.14.0` 의 C 빌드까지 통과했다. 실행한 명령: `OPENSIDE_XWIN=<cargo-xwin 설치 루트> python3 tools/check-platforms.py`, 내부적으로 `cargo +nightly-2026-08-08 xwin check --workspace --all-targets --locked -j 8 --target x86_64-pc-windows-msvc -Zbuild-std=std,panic_abort --target-dir /tmp/openside-xcheck`. 경고만 있고 오류는 없다(런타임 미지원 경로로 인한 데드 코드 경고)

주의할 점:
- `build-std`를 통한 `cargo check` 통과는 타깃 바이너리의 링크나 실행을 뜻하지 않는다.
- 이 점검은 nightly 컴파일러를 사용해 타깃 std API 준수 여부를 확인한 것이며, 저장소에 고정된 stable 툴체인이 실제 네이티브 macOS/Windows 러너에서 통과함을 입증하는 것은 아니다. 네이티브 CI 매트릭스의 `cargo check --workspace --all-targets --locked -j 8` 검증은 여전히 필수 후속 과제로 남아 있다.

## macOS 주의점

- bwrap 이 없으므로 `sandbox.required = false` 이면 `bash` 가 샌드박스 없이 실행되고(doctor `warn`), `true` 이면 `bash` 가 숨겨진다.
- `XDG_RUNTIME_DIR` 이 보통 없다. 데몬과 CLI 모두 runtime 경로를 임시 디렉터리`/openside-<uid>` 로 잡고(`Loaded::socket_path`, 데몬과 CLI 가 공유) `daemon.socket` 이 있으면 그 값을 쓰므로 기본값으로 서로 연결된다. `--socket` 은 필요 없다. macOS 에서 확인했다(`/var/folders/.../T/openside-501/opensided.sock` 바인드·연결 성공).
- `dirs` crate 는 macOS 에서 state·runtime 디렉터리를 주지 않으므로 state 는 data 디렉터리를 쓴다.
- 브라우저는 PATH 다음에 `/Applications/Chromium.app/Contents/MacOS/Chromium`, `/Applications/Google Chrome.app/Contents/MacOS/Google Chrome` 을 찾는다.
- Unix 소켓 경로 길이 제한(macOS `sun_path` 104바이트)이 Linux(108바이트)보다 짧다. 기본 `$TMPDIR/openside-<uid>` 경로(약 55바이트 + `opensided.sock`)는 안전하지만, 더 깊은 임시·런타임 디렉터리에서는 데몬·사이드카 소켓 bind 가 실패할 수 있다.
- 호스트 파일 스크리닝(`crates/opensided/src/credential_paths.rs`의 `check_filesystem`)은 Linux 환경에만 구현되어 있다. macOS를 포함한 다른 Unix 계열 시스템에서는 "지원하지 않음(unsupported)" 오류를 반환하므로, 호스트 파일 도구인 `read`, `write`, `edit` 및 아티팩트 파일 수집 기능이 fail-closed 방식으로 동작을 중단한다(스크리닝 없이 실행되지 않고 거부됨). 이는 의도된 동작이며, macOS로의 스크리닝 기능 이식은 아직 진행하지 않았다.
- 서비스 예시(launchd)는 [INSTALL.md](INSTALL.md#launchd-macos-unverified) 를 본다.

## macOS 네이티브 검증 (2026-10-13)

- **검증 환경**: macOS 26.7.1 (25G241), x86_64, 로컬 워크스테이션의 Quickemu VM. rustup stable 1.99.0 설치, Xcode 없이 Command Line Tools(CLT)만 사용.
- **워크스페이스 빌드**: `cargo build --workspace -j 4` 통과(경고 0건, 약 2분 소요). `cargo test --workspace --no-run -j 4` 통과로 `#[cfg(unix)]` 테스트 타깃이 모두 정상 컴파일·링크됨을 확인했다(Linux 전용 타깃은 빈 크레이트로 컴파일된다).
- **단위 및 통합 테스트**: `cargo test -j 4 -p openside-config -p openside-proto -p openside-vault -p openside-sessions -p openside-cli -p openside-relay -p openside-import -- --test-threads=2` 실행 결과 대상 크레이트가 모두 통과했다(config 46, proto 17, vault 32, sessions 25, cli 85, relay 3개 타깃 총 21, import 34).
- **진단 도구 (`openside doctor`)**: 볼트 백엔드로 macOS Keychain을 정상 인식하고 런타임 디렉터리로 `$TMPDIR/openside-<uid>`, 데이터 및 native messaging 경로로 `~/Library/Application Support`를 잡는다. bwrap 부재 및 브라우저 미설치 경고도 의도대로 출력한다.
- **데몬 스모크 테스트**: `opensided`가 Unix 소켓을 정상 바인드하며, `openside status` 호출 시 예상된 API 응답(`model_not_configured`)을 반환해 전송 계층 동작을 확인했다.
- **볼트 네이티브 백엔드 엔드투엔드 검증**: `openside vault init` 실행 시 로그인 키체인에 `openside-vault` 항목으로 래핑 키를 저장하며, 항목 추가(`add`), 목록 조회(`items`), 잠금(`lock`), 잠금 해제(`unlock`) 왕복 동작이 정상 동작함을 확인했다.
- **키체인 제약 사항**:
  1. 테스트 환경처럼 `HOME`을 임의로 재지정한 상태에서 데몬을 실행하면 Security.framework가 `errSecNoDefaultKeychain`(-25307)을 반환하며, 데몬은 의도된 대로 `keyring_unavailable` 상태로 fail-closed 처리된다.
  2. SSH 등 비-GUI 세션에서는 `security unlock-keychain`을 실행하기 전까지 `errSecInteractionNotAllowed` 오류가 발생한다. 이는 openside 결함이 아니라 macOS 세션 자체의 보안 특성이다.
- **미검증 항목**: Chromium 계열 브라우저 미설치(브라우저 스택 미검증), Python 사이드카 가상환경 미설치(에이전트 런타임 미검증), launchd 서비스 파일 미검증.

## Windows

Windows 환경은 런타임 지원 대상이 아니며, 현재는 `x86_64-pc-windows-gnu` 와 `x86_64-pc-windows-msvc` 크로스 컴파일 점검만 통과한 상태다.

Windows를 실제로 지원하려면 단순 컴파일을 넘어 다음 요소가 구현되어야 한다:
- 안전한 Windows 프로세스 트리 관리 및 수명 주기 제어
- 데몬, 사이드카, 권한 분리 작업 간 프로세스 간 통신(Unix 도메인 소켓 대체 전송 계층)
- 샌드박스 격리 체계
- 네이티브 런타임 및 OS 기능 통합

코드 내 `cfg(not(unix))` 경로에서 반환하는 명시적인 `Unsupported` 오류 분기는 런타임 지원이 아니며, 안전하지 않은 실행을 방지하기 위한 거부 처리다.

`x86_64-pc-windows-msvc` 타깃은 plain `cargo` 로는 Microsoft CRT/SDK 헤더가 없어 C 의존성(`assert.h`, `stdlib.h` 누락)에서 막히지만, `cargo-xwin` 을 쓰면 컴파일 점검이 통과한다(위 "점검 결과"). 이 점검은 링크와 실행을 뜻하지 않으며, 고정된 stable 툴체인으로 네이티브 Windows CI 러너에서 하는 점검은 여전히 필요하다.

## Windows 런타임 시험 (wine)

wine 11.17 가상 환경(임시 prefix)에서 Windows 런타임을 시험했다. 다만 wine은 실제 Windows가 아니므로, 네임드 파이프와 접근 제어 목록(ACL), Job Objects 등은 실제 Windows 환경에서 추가로 검증해야 한다. 빌드는 llvm-mingw로 링크하여 `cargo +nightly-2026-08-08 test --no-run --workspace --target x86_64-pc-windows-gnu -Zbuild-std=std,panic_abort,test` 명령으로 수행했다. rustc의 windows-gnu 타깃은 `-lgcc -lgcc_eh`를 링크하려 하지만 llvm-mingw에는 해당 라이브러리가 없다. 따라서 이를 `libunwind.a` 및 `libclang_rt.builtins-x86_64.a`로 대응시키는 스텁 라이브러리를 만들어 `-L native=`로 넘겨 링크했다. 생성된 76개 테스트 바이너리는 `wine <exe> --test-threads=4`로 실행했다.

| 크레이트 | 통과 | 실패 | 원인 |
| --- | --- | --- | --- |
| core | 8 | 0 | - |
| proto | 14 | 0 | - |
| sched | 39 | 0 | - |
| sessions | 25 | 0 | - |
| sync | 17 | 0 | - |
| vault | 19 | 0 | - |
| memory lib+store | 24 | 0 | - |
| memory no_network | 0 | 1 | wine 환경 내에 rustc가 없어 `rustc -vV` 프로세스 생성 실패 |
| push | 4 | 0 | - |
| repl | 9 | 0 | - |
| skills | 19 | 0 | - |
| agent-sidecar | 6 | 0 | 프로세스 관련 테스트는 `#[cfg(unix)]`로 제외 |
| browser | 19 | 0 | 브라우저 관련 테스트는 `#[cfg(unix)]`로 제외 |
| opensided lib | 38 | 0 | 나머지 테스트는 `#[cfg(unix)]`로 제외 |
| openside-e2e | 0 | 0 | 크레이트 전체가 `#![cfg(unix)]` |
| policy | 30 | 2 | 테스트 픽스처가 Windows에서 절대 경로가 아닌 `/work`를 사용 |
| config | 34 | 1 | `drm::tests::widevine_finds_distribution_windows_and_macos_install_layouts`에서 경로 구분자 혼용 |
| cli | 2 | 3 | `service::tests::*`에서 Windows의 절대 경로가 아닌 `/opt/...` 사용 |
| channels lib | 18 | 1 | Windows 실행 파일에 `echo` 명령어가 존재하지 않음 |
| channels tests/channels | 0 | 5 | TOML 기본 문자열 내 `\u…` 형태의 Windows 경로 포맷팅 오류 |
| import | 10 | 18 | 심볼릭 링크 안전 프로필 임포트 및 프로필 잠금 미지원(예상된 실패) |

실패 원인은 크게 심볼릭 링크 안전 임포트 미지원(설계상 예상된 결과)과 policy, channels, cli, config 크레이트의 테스트 픽스처에 하드코딩된 Unix 경로 문제로 나뉜다. 실행 파일 스모크 테스트 결과, `opensided.exe`와 `openside.exe status`는 "Unix daemon transport is unsupported on this platform"을 출력하며 종료했다. `openside.exe doctor --json`은 종료 코드 1을 반환했다. 이 도구는 이제 `daemon.transport`를 실패로 정상 보고하며, 수정 전 나타나던 오해 소지가 있는 bwrap 및 remote.listener 경고는 출력하지 않는다.

## 잠재 문제: 정책 glob (Windows 런타임 전에 고칠 것)

`crates/openside-policy/src/glob.rs`는 glob 패턴을 `/` 기준으로만 분리하고 대소문자를 구분해 비교하며, `path_within`은 `Path::starts_with`를 사용한다. Windows 환경에서 정책 규칙을 `C:/x/**`로 작성하면 `canonicalize`가 생성하는 `\\?\C:\x\...` 경로와 결코 일치하지 않으며, `*` 패턴이 디렉터리 구분자인 `\`를 넘어서 매칭된다. 이는 향후 Windows 런타임이 구현되었을 때 거부(deny) 규칙을 우회하는 보안 취약점이 된다. macOS에서는 APFS 파일시스템이 대체로 대소문자를 구분하지 않지만, 대상 경로를 먼저 정규화(files.rs)하므로 위험도가 낮다 [INFERENCE: Darwin의 realpath는 디스크에 기록된 대소문자 형태를 반환함]. Windows 런타임을 배포하기 전에 반드시 정책 대상 경로를 정규화(`\\?\` 접두사 제거, `\`를 `/`로 변환, 드라이브 문자 소문자화, 대소문자 무시 비교)한 뒤 `glob_match` 및 `path_within`을 호출하도록 수정하고 테스트를 추가해야 한다. 이는 보안상 매우 중요한 작업이며 별도 검토가 필요하다(예상 작업량 약 2인일).

## 지원 계획과 견적

2026-10-12 점검을 바탕으로 산정한 예상 공수다. 실제 검증용 네이티브 호스트가 없어 모두 추정치이며, 두 플랫폼 모두 사용자의 진행 결정이 필요하다.

macOS는 위 "macOS 네이티브 검증" 절에 따라 데몬·CLI·Keychain 볼트의 네이티브 실행까지 확인했다. 남은 지원 작업 단계는 다음과 같다.
- `fstatfs`의 `f_fstypename`을 활용해 macOS용 `check_filesystem`을 구현(0.5~1인일): 읽기, 쓰기, 수정 및 산출물 수집 기능을 다시 활성화한다.
- bwrap 부재에 따른 `bash` 샌드박스 정책 결정: macOS에서 기본적으로 `bash` 도구를 숨기거나(비-Linux 환경에서 `sandbox.required`를 true로 설정, 0.5인일), `sandbox-exec` Seatbelt 백엔드를 추가하고(3~5인일) 고정 PATH에 `/opt/homebrew/bin`을 등록한다.
- `macos-14` CI 잡 구성(2~3인일). 네이티브 검증에서 `/proc` 헬퍼는 이미 모두 `cfg(target_os = "linux")`로 한정돼 있음을 확인했고, 발견된 두 건의 테스트 수정(키링 도출 반복 횟수 픽스처, 죽은 리스너 소켓 파일)은 이미 반영됐다.
- 선택적 보안 강화: `private_directory` 검사 시 소유자 uid 검증(작업 완료).
총 예상 공수는 4~6인일이며, Seatbelt 샌드박스까지 포함하면 1.5~2주가 걸린다.

Windows의 경우 필요한 작업 항목은 다음과 같다.
- 현재 사용자 SID 대상 DACL을 적용한 네임드 파이프 데몬 트랜스포트 구현(4~6인일).
- CLI 네임드 파이프 엔드포인트 구현(1~2인일).
- 기존 128비트 임의값(nonce) `Hello`를 사용한 `127.0.0.1:0` 사이드카 트랜스포트 구현 및 Python `asyncio.open_connection` 연동(2~3인일).
- 사이드카, Chromium, 계정 탐색, 키체인 명령 제어를 위한 Job Objects(`JOB_OBJECT_LIMIT_KILL_ON_JOB_CLOSE`) 기반 프로세스 감독(5~8인일).
- 정책 경로 정규화(2인일).
- `bash` 도구 차단 유지(XS).
- 사이드카 실행 기본값을 `python` 및 `py -3`로 설정(0.5인일).
- 작업 스케줄러(Task Scheduler) 기반 `openside service` 구현(1인일).
- OS 환경을 인식하는 doctor 진단(트랜스포트 점검 항목은 이미 완료).
- 테스트 픽스처 수정 및 `windows-latest` CI 잡 구성(3~4인일).
데몬, CLI, 사이드카 구현에 총 4~5인주가 필요하며, 파일 도구(`CreateFileW(FILE_FLAG_OPEN_REPARSE_POINT|FILE_FLAG_BACKUP_SEMANTICS)` 기반 핸들 탐색 구현, 완료 전까지 읽기·쓰기·수정 차단 유지)에 1~1.5인주가 추가된다. 브라우저 프로필 임포트(DPAPI/AES-GCM) 및 Seatbelt/AppContainer 샌드박스는 최소 지원 범위에서 제외한다. 추가 발견 사항으로, Windows에서는 `dirs::data_dir`이 Roaming 프로필 경로를 가리키므로 SQLite 이벤트 로그, vault, 메모리 모델 파일이 로밍 프로필에 저장된다(Local 경로로 변경 필요). 또한 Windows 환경에는 통상 `python3` 명령어가 없다.
