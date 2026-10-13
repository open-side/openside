# 플랫폼 지원

| 플랫폼 | 상태 | 근거 |
|---|---|---|
| Linux | 지원한다. 개발과 테스트 모두 Linux 환경에서 진행했고, CI는 Linux에서 `tools/gate.sh`를 실행한다 | 헤드리스 브라우저 컨트롤러 테스트는 디스플레이 종속성이 없음을 보이려고 일부러 잘못된 디스플레이 엔드포인트를 지정한 채 Wayland와 X11 세션 환경 모두에서 통과했다(특정 윈도우 매니저 호환성 인증은 아님). B-5 포커스 테스트는 실제 Wayland 컴포지터 세션에서 통과했다 |
| macOS | macOS 26.7.1 환경에서 네이티브 x86_64 빌드/링크, 핵심 데몬/CLI, 스크립트 기반 Python sidecar, 직접 CDP 브라우저 열기/스냅샷/스크린샷, 실제 Keychain 볼트 스모크 테스트를 검증했다. 이는 모든 기능이나 전체 테스트 스위트의 통과를 뜻하지 않는다. Apple Silicon 네이티브 실행, 패치된 AgentGateway, launchd 연동은 아직 검증되지 않았다 | 구체적인 실패 내역과 복구 현황은 아래 네이티브 검증 섹션을 참고한다 |
| Windows | Windows 런타임은 지원하지 않는다. VS2022 및 stable 1.99.0 MSVC 기반 네이티브 Windows 11 build 26100.1742 환경에서 작업공간 전체 타깃 빌드와 링크를 완료했다. 초기 전체 Rust 테스트에서는 475개 통과, 40개 실패를 기록했으나, Unix/Linux 전용 픽스처 스코프를 명확히 분리하고 이식성을 수정한 뒤 이전에 실패했던 9개 타깃을 집중 검사하여 모두 통과했다 | 미지원 데몬 및 상태 경로는 패닉 없이 문서화된 Unix 트랜스포트 오류를 반환한다. 컴파일 성공이나 인메모리 API 테스트 통과를 정상 작동하는 Windows 데몬으로 소개하지 않는다 |

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
| `openside-vault/src/platform/native.rs` | `cfg(any(target_os = "macos", target_os = "windows"))`: `unlock = "native"`(또는 해당 환경의 `auto`)는 `keyring` 크레이트를 통해 macOS Keychain 또는 Windows Credential Manager에 래핑 키를 보관함 | 네이티브 macOS CLI init/add/items/lock/unlock/remove 스모크 테스트로 실제 Keychain 래핑 항목 및 vault format 2를 검증했으며 일회용 픽스처 항목은 모두 제거했다. Windows 네이티브 빌드와 링크는 확인했으나 Credential Manager 직접 연동은 본 환경에서 실행하지 않았다 |
| `openside-agent-sidecar/src/lib.rs`, `process.rs`, `connection.rs` | `process` 모듈 전체가 `cfg(unix)`: 세션별 Unix 소켓(`UnixListener`), `killpg`, 0700 디렉터리. `nix` 는 `cfg(unix)` 의존 | `cfg(not(unix))` `AgentRuntime` 은 세션을 열지 못하고 오류를 돌려준다 |
| `openside-config/src/paths.rs` | runtime 대체 경로 `openside-<uid>`(`nix::unistd::getuid`) | 임시 디렉터리`/openside` |
| `openside-config/src/discovery.rs` | 실행 비트 검사(`cfg(unix)`), Windows `PATHEXT` 후보(`cfg(windows)`), macOS·Windows 브라우저 위치 | Windows 는 실행 비트 없이 파일 존재만 본다 |
| `openside-config/src/doctor.rs` | 디렉터리 소유자 uid·0700 검사, `sandbox.bwrap`·`remote.*` 점검 | 읽기 전용 여부만 검사. `sandbox.bwrap`·`remote.*` 대신 `daemon.transport` 를 `fail`("daemon transport is unsupported on this OS")로 보고한다 |
| `openside-e2e/src/lib.rs` | 크레이트 전체 `#![cfg(unix)]`, `nix` 의존성은 `cfg(unix)`로 조건화. `set_child_subreaper`는 `cfg(target_os = "linux")`, `agent/.venv/bin/python` 경로 사용 | Unix 밖에서는 빈 크레이트로 컴파일된다. 크레이트 타깃이 컴파일된다고 해서 Windows에서 E2E 테스트를 실행할 수 있는 것은 아니다. macOS 에서는 subreaper 없이 실행 |
| `openside-import/src/db.rs`, `db_windows.rs`, `crypto.rs`, `safe_storage.rs`, `chromium_passwords.rs`, `store.rs` | 프로필 탐색에는 Linux/macOS/Windows의 네이티브 경로를 사용한다. SQLite 스냅샷은 온라인 백업이나 검증된 콜드 메인 파일 복사를 거친다. Windows에서는 소스 핸들과 리파스 포인트(reparse-point) 상위 경로를 검증하고 소유자 전용 DACL 스크래치 파일을 생성한다. 자세한 내용은 아래 스냅샷 경계 절을 참고한다. | 이 지원은 Windows 소스 리더만을 활성화하며, 미지원 상태인 Windows 데몬·브라우저 실행이나 대상 프로필 잠금까지 지원하는 것은 아니다. Chromium의 App-Bound 암호화는 여전히 지원하지 않으며, 브라우저 잠금이나 OS 자격 증명 보호 체계를 우회하지 않는다. |
| `extension/`(MV3), `openside-config/src/doctor.rs`(native messaging 점검) | sidePanel·tabGroups·omnibox 를 지원하는 순정 Chromium 126 이상. OS 별 NativeMessagingHosts 경로: Linux 는 XDG 사용자·시스템 디렉터리, macOS 는 `~/Library/Application Support` 와 시스템 디렉터리, Windows 는 레지스트리 키(문서에만 적고 조회하지 않음) | 확장 프로그램 빌드는 플랫폼 독립적임. Linux 헤드리스 E2E 및 공식 CfT 환경의 네이티브 Mac MV3 확장 프로그램 클릭/위임 픽스처 1건 통과. 실제 macOS 데스크톱 확장 프로그램 및 네이티브 메시징 연동은 미검증 상태이며, Windows 브라우저 런타임 역시 검증되지 않음. Windows 에서 doctor 는 레지스트리 키 위치만 안내한다 |

Linux 전용 기능:

- bubblewrap 샌드박스(`bash` 도구). Linux 커널 namespace 기반이다.
- `openside-e2e` 의 child subreaper(`prctl`).
- `openside-memory` 의 소켓 감시 테스트(`tests/no_network.rs`). `/proc/self/fd` 를 읽으며, 다른 플랫폼에서는 이 테스트가 빠지고 의존성 그래프 검사만 돈다.
- 볼트의 Secret Service 백엔드(`unlock = "secret-service"` 및 Linux 환경의 `auto` 선택). 격리된 `dbus-daemon` 환경의 모의 Secret Service를 대상으로 테스트를 실행한다(`crates/openside-vault/tests/secret_service.rs`, `dbus-daemon` 미설치 시 건너뜀). 또한 개발 머신의 gnome-keyring 환경에서 직접 수동 검증(`openside doctor`, `openside vault add`/`items`/`rm`)을 마쳤다. KWallet 대상 검증은 진행하지 않았다.
- `openside memory model fetch`의 모델 설치: 세대 디렉터리를 가리키는 심볼릭 링크(`std::os::unix::fs::symlink`)를 원자적 rename으로 교체하는 방식은 Unix 전용이다(macOS 포함). 비-Unix 환경에서는 설정을 읽거나 모델 파일을 다운로드하기 전에 미지원 오류(Unsupported I/O)를 반환하며 작업을 즉시 중단한다.
- 데몬 GUI는 CLI와 동일한 Unix 모델 설치 프로그램을 사용한다. 통제된 데몬 재시작은 Unix 환경에서 인플레이스 exec 방식을 취해 systemd/launchd 환경에서도 PID를 유지한다. Windows 데몬 전송 계층과 통제된 재시작은 여전히 지원하지 않는다.
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

- macOS에서는 bubblewrap이 없어 선택적 샌드박스를 지원하지 않으며, 필수 샌드박스 요청 시 bash 실행을 숨기거나 거부한다. 치명적이지 않은 `sandbox_unavailable` 이벤트는 CLI 경고로 표시되며 태스크 스트리밍이나 대기를 중단하지 않는다. 그 외 런타임 오류는 기존대로 대기를 종료한다. bubblewrap 전용 성공/설정 픽스처는 Linux 환경으로 한정하며, 샌드박스 없는 셸의 0이 아닌 종료 코드는 모든 Unix 플랫폼에서 테스트한다.
- `XDG_RUNTIME_DIR` 이 보통 없다. 데몬과 CLI 모두 runtime 경로를 임시 디렉터리`/openside-<uid>` 로 잡고(`Loaded::socket_path`, 데몬과 CLI 가 공유) `daemon.socket` 이 있으면 그 값을 쓰므로 기본값으로 서로 연결된다. `--socket` 은 필요 없다. macOS 에서 확인했다(`/var/folders/.../T/openside-501/opensided.sock` 바인드·연결 성공).
- `dirs` crate 는 macOS 에서 state·runtime 디렉터리를 주지 않으므로 state 는 data 디렉터리를 쓴다.
- 브라우저는 PATH 다음에 `/Applications/Chromium.app/Contents/MacOS/Chromium`, `/Applications/Google Chrome.app/Contents/MacOS/Google Chrome` 을 찾는다.
- macOS Unix 소켓의 `sun_path` 길이는 104바이트로 Linux의 108바이트보다 짧다. 데몬은 구동 전 소켓 주소를 검증하며, 경로가 초과되면 공개 엔드포인트를 임의로 변경하지 않고 더 짧은 `--socket`, `OPENSIDE_SOCKET` 또는 `daemon.socket` 경로를 요청하는 메시지를 반환한다. sidecar 런타임의 상위 디렉터리 경로가 너무 깊어 유효한 Unix 소켓을 생성할 수 없을 때는 해당 sidecar의 소켓만 무작위 이름의 0700 비공개 `/tmp` 디렉터리에 배치하고, 0600 소켓 권한과 nonce 인증은 유지한다. Python 프로세스 픽스처 역시 짧은 임시 소켓 디렉터리를 사용한다.
- Linux와 macOS에서 호스트 파일 디스크립터 검사를 지원한다. macOS에서는 `fstatfs` 파일시스템 이름을 기준으로 판별하며, APFS/HFS+, NFS/SMB 및 일반 디스크 포맷을 포함한 정규 스토리지 파일시스템은 허용하고 devfs나 프로세스/커널 가상 파일시스템은 거부한다. 확인되지 않은 macOS 파일시스템 유형과 미지원 Unix 플랫폼은 fail-closed 원칙에 따라 거부한다. 이 검사가 보호 대상 자격 증명 경로, 디스크립터 동일성, 심볼릭 링크 추적 금지(no-follow) 검사를 완화하지는 않는다.
- 관리 대상 브라우저 프로필은 정규화된 물리 경로로 기록한다. 구동 시 소유권 검사에서 신뢰하도록 설정된 상위 데이터 디렉터리를 정규화하므로 `/var` 및 `/private/var` 별칭으로 인해 관리 대상 프로필을 비관리 프로필로 오판하지 않는다. 신뢰 루트 외부의 프로필은 계속 거부하며, 신뢰 루트를 벗어나는 최종 프로필 심볼릭 링크 역시 허용하지 않는다. 심볼릭 링크 추적 금지(no-follow) 업로드 레이스 픽스처는 플랫폼 네이티브 정규 작업공간 루트를 사용한다.
- Unix 환경의 데몬 구동 시, 기존 소프트 제한이 더 낮은 경우에만 `RLIMIT_NOFILE` 소프트 제한을 `min(hard, 8192)`로 상향한다. 하드 제한과 이미 더 높게 설정된 소프트 제한은 변경하지 않는다. 단기 실행 런타임에서 `scheduler→host→Shared→scheduler` 순환 참조와 SQLite 디스크립터가 유지되지 않도록, 종료 및 드롭(drop) 시점에 루틴 스케줄러 슬롯을 정리한다. 수명 주기 회귀 검증에서는 프로세스 전역 런타임 1회 초기화 이후 디스크립터 개수를 비교한다.
- 네이티브 macOS 헤드리스 Chromium에서 `selectOption`은 비어 있지 않고 고유하며 출력 가능한 옵션 라벨에 신뢰할 수 있는 연속 타이핑(type-ahead) 입력을 전달한 뒤 커밋된 값을 다시 읽어 확인한다. 첫 글자를 입력하기 전에 가능한 모든 중간 선택지가 정책상 유효한지 미리 검사한다. 라벨이 모호하거나 비어 있거나 입력할 수 없는 경우, 포커스가 변경되거나 컨트롤이 교체된 경우, 또는 값이 커밋되지 않은 경우에는 수동 조작 안내와 함께 `select_needs_takeover` 오류를 반환한다. DOM value/change-event 스크립트를 주입하거나 전역 공간/화살표 키 탐색 동작을 임의로 변경하지 않는다. 네이티브 연속 타이핑의 1초 이전 입력 버퍼가 만료되어야 새로운 선택을 진행할 수 있다.
- Unix 환경에서 브라우저 프로세스 종료는 다음과 같이 처리된다. 최종 `Browser.close` ACK를 정상 수신했거나 일반적인 전송 계층 EOF를 수신한 종료 코드 0(`exit 0`)은 가드 유지나 중간 증명 확보 여부와 관계없이 기존대로 성공(`Ok`)으로 처리한다. 반면 최종 `Browser.close`의 프로토콜 오류나 타임아웃, 정리 과정의 I/O 오류는 결코 성공으로 전환되지 않는다. 시그널에 의한 종료는 (1) 닫기 시작 전 자식 프로세스가 살아 있었고, (2) 닫기를 시작하기 전과 ACK 수신 시점 모두에 보류 중인 CDP 요청이나 리소스, 완료되지 않은 보호 대상 프로필 쓰기 트랜잭션이 전혀 없는 유휴 상태였으며, (3) 모든 브라우저 페이지 타깃이 ACK와 함께 닫혔고, (4) 최종 `Browser.close` ACK를 수신한 경우에 한해서만 예외적으로 완료된 종료(`completed shutdown`)로 간주한다. 타깃 조회나 닫기 증명 확인에 실패하면 시그널 예외 적용 자격만 상실되며, 정상 `exit 0` 종료를 실패로 바꾸지는 않는다. 시그널로 종료되었으나 이 조건을 충족하여 완료 처리될 때는 시그널 정보를 포함한 경고 로그를 `stderr`에 남기고 프로세스 수명 주기 메트릭인 `browser_shutdown_crashes` 카운트를 1 증가시킨다. `GET /v1/doctor` API는 `browser.shutdown_crashes` 점검 항목과 카운트를 표시하며 수치가 0이 아니면 `"warn"` 상태를 반환한다. 반면 CLI의 `openside doctor`는 데몬에 연결하지 않고 독립적인 설정 점검을 수행한다. 시그널 종료 시점에 자격 요건을 충족하지 못했거나 쓰기가 완료되지 않은 상태라면 `ProcessExit` 오류로 처리되며, 0이 아닌 종료 코드로 끝난 비정상 종료 역시 별도로 오류로 처리된다. 원시 CDP를 포함하여 명시적인 `ProfileWriteGuard`로 프로필 쓰기 트랜잭션을 관리하며, 자체 브라우저의 쿠키 임포트는 디스크 플러시를 위해 해당 브라우저가 정상 종료될 때까지 트랜잭션을 유지하고(이 상태에서도 정상적인 `exit 0` 종료는 통과함), 이미 실행 중인 브라우저로 전달할 때는 트랜잭션 전체 동안 가드를 유지한다. CDP 프로필 쓰기 요청을 취소하더라도 완료가 보장되지 않으므로 지연된 응답이 올 때까지 등록 상태를 유지한다. 이 규칙이 적용되어도 ACK 수신이 디스크 플러시 완료를 보장하지는 않으며 브라우저 자체의 버그가 해결된 것도 아니다.
- 서비스 예시(launchd)는 [INSTALL.md](INSTALL.md#launchd-macos-unverified) 를 본다.

## macOS 네이티브 검증 (2026-10-13)

- 네이티브 환경: macOS 26.7.1 build 25G241 x86_64, stable Rust 1.99.0, 전체 Xcode 대신 CLT 사용, uv 0.13.0/Python 3.13.16, 공식 Node 22.23.3/Corepack 0.36/pnpm 9.15.9, 공식 Chrome for Testing 155.0.8059.39. 고정된 cryptography 50.0.2 버전은 Intel macOS용 휠이 제공되지 않아 의존성을 낮추지 않고 공식 OpenSSL 3.5.9 소스와 `OPENSSL_DIR`/`OPENSSL_STATIC`을 지정하여 직접 빌드했다. 빌드는 공유 폴더가 아닌 게스트 로컬 소스/타깃 디렉터리에서만 수행한다. 복구 검증은 CPU Tctl 발열 제어를 위한 일시 정지와 호스트 고부하 잠금 하에 `cargo -j 2` 및 `--test-threads=1` 설정으로 실행한다.
- 초기 커밋 트리 기준 전체 macOS 검증 결과: `cargo build --workspace --all-targets --locked -j 4` 빌드를 통과했다. `cargo test --workspace --locked -j 2 --no-fail-fast -- --test-threads=1 --show-output` 실행 시 129개 단위/통합/문서 타깃에서 966개 통과, 137개 실패, 10개 무시를 기록했고 27개 타깃에서 실패가 발생했다. 명목상 통과로 집계된 6개 항목은 사전 요구사항 미충족에 따른 조기 반환 스킵이므로 실제 실행 검증으로 간주하지 않는다. `pytest -m 'not live'`로 실행한 Python 비라이브 테스트는 224개 통과, 1개 실패, 2개 제외(deselected)를 기록했으며 실패 원인은 Unix 소켓 픽스처의 경로 길이 초과였다. 초기 웹(53개 파일/332개 테스트)과 확장 프로그램(11개 파일/45개 테스트)은 고정 설치(frozen install) 및 프로덕션 빌드 조건에서 모두 통과했다. 이 수치는 초기 커밋 상태의 결과이며 복구 후 소스 코드의 결과가 아니다.
- 네이티브 볼트 래핑 키 백엔드는 macOS Keychain 또는 Windows Credential Manager용 `keyring`을 사용한다. 네이티브 macOS CLI init/add/items/lock/unlock/remove 스모크 테스트로 실제 Keychain 래핑 항목 및 vault format 2를 검증했으며 일회용 픽스처 항목은 모두 제거했다. Windows 네이티브 빌드와 링크는 확인했으나 Credential Manager 직접 연동은 본 환경에서 실행하지 않았다. 본 VM 환경의 SSH Keychain 검증에서는 잠금 해제된 로그인 키체인, 상속된 로그인 환경 변수와 실제 게스트 `HOME`을 유지한 동일 로그인 세션 내 데몬 구동 조합에서 성공을 확인했다. 별도 unlock 실행 후 환경 변수가 초기화된 새 SSH 세션으로 접속하는 방식은 본 VM에서 충분하지 않았다. 문서에 VM 비밀번호나 키 데이터를 포함하지 않는다.
- 본 헤드리스 Intel macOS 26.7.1 QEMU 게스트 및 공식 Chrome for Testing 155.0.8059.39 환경에서는 `Target.close`와 `Browser.close`가 포함된 수명 주기 시퀀스 진행 중 SIGSEGV 11로 비정상 종료되는 현상이 관측되었다. 다만 모든 개별 닫기 호출마다 크래시가 발생하는 것은 아니며, 초기 1~2회의 호출은 정상적으로 ACK 응답을 받았고 별도로 진행한 단일 탭 스모크 테스트의 닫기도 성공했다. 공식 154.0.8037.92 역시 대조군으로 테스트한 임포트 정리 단계의 `Browser.close`에서 SIGSEGV가 확인되었다(154 버전의 전체 매트릭스를 점검한 것은 아님). 기본 botcheck의 `Disconnected` 현상은 완료 전 발생한 브라우저 프로세스 결함을 반영하며, 이러한 실패는 여전히 오류로 처리된다. 현재 정책은 ACK를 수신하고 보류 중인 요청이 없는 유휴 상태에서 완료된 시그널 종료와, 조기 발생하거나 미완료 상태에서 발생한 실패를 규칙에 따라 엄격히 구분한다. 한편 네이티브 쿠키 API가 `Storage.setCookies` 호출에 ACK를 보냈음에도 영속화된 대상 쿠키가 0행으로 남은 현상은 디스크 플러시 전 크래시로 설명할 수 있으나([INFERENCE]), 이것이 0행 발생의 유일한 입증된 원인은 아니다. 보류 중이던 임포트 작업은 결코 성공으로 처리되지 않는다. 정확한 C++ 및 시스템 계층 원인은 밝혀지지 않았고, 관측된 그래픽 및 알림 경고도 직접적인 원인으로 확인되지 않았다. 이 결과는 본 VM 및 브라우저 구성에 국한되며 모든 Mac이나 Apple Silicon 환경을 대변하지 않는다.
- 작업 트리 스냅샷 `SHA 15245c73da7f` 기준 검증 결과(동시 진행 중인 기능 추가로 인해 초기 베이스라인 커밋과 테스트 목록이 다름): 1회차 전체 네이티브 all-target 빌드(`-j2`)를 통과했다. 실행 로그에 기록된 전체 Rust 결과는 140개 타깃에서 1171개 통과, 140개 실패, 14개 무시, 30개 실패 타깃이었다. 이 중 102건은 사전 요구사항 미충족에 따른 조기 반환 스킵이었다. 97건은 실행 순서 착오로 신규 소스의 `agent/.venv`가 이후 pytest 단계 전까지 준비되지 않아 발생했고, 2건은 패치된 브라우저 부재, 3건은 bubblewrap 부재 때문이었다. 따라서 1171개 항목 전체가 실제로 실행 검증된 것은 아니며, 영향받은 타깃의 재실행 결과는 별도로 기록하고 전체 작업공간 2회차 재실행은 수행하지 않았다. 한편 초기 베이스라인에서 54건 발생했던 후반 데몬 DB 오류와 달리, 이번 실행에서는 SQLite `CannotOpen` 오류가 0건을 기록했다.
- 전체 `pytest -m 'not live'` 실행 결과 252개 통과, 11개 실패, 2개 제외(`deselected`)를 기록했다. 새로 발생한 실패는 대기 중이던 전송 쓰기가 완료되기 전에 버퍼가 NUL `0x00` 바이트로 채워지면서 발생했다. 이로 인해 관측상 약 8KiB 시점부터 NUL 데이터 깨짐이 나타났다. 전송이 소유하는 불변 복사본을 유지하도록 수정한 뒤, 기존 실패 11건과 300KB 저속 리더 회귀 테스트를 포함한 대상 6개 파일 32개 테스트가 모두 통과했다. 이를 전체 pytest 재실행에서 전건 통과한 것으로 해석해서는 안 된다.
- 고정 설치(`frozen install`)와 프로덕션 빌드 조건에서 `web`과 `extension` 빌드가 모두 통과했다. 실제 `web` 전체 실행 결과는 75개 파일에서 523개 통과, 8개 실패였으며, `extension`은 16개 파일에서 90개 통과, 0개 실패를 기록했다. `web` 실패 8건 중 6건은 오래된 delivery 및 onboarding mock 응답 설정 때문이었으며, 본 검증 스냅샷 기준으로 대상 테스트(onboarding 39개, delivery 7개)는 모두 통과한다. 해당 테스트 실행 당시 남은 2건의 `UI ↔ API map` 및 OpenAPI 스냅샷 실패는 과거에 생성된 아티팩트가 원인이었다. 점검 후 Main이 `docs/specs/openapi.json`, `web/src/api/schema.ts`, `extension/src/schema.ts`를 새로 생성했으나, 본 macOS 검증에서는 해당 버전을 바탕으로 전체 `web` 테스트나 2개 스냅샷 검사를 다시 실행하지 않았다. `UI ↔ API map`을 비롯한 생성 문서는 Main에서 별도로 검증한다.
- 게스트 Python 가상환경(venv)과 일관된 오버레이를 구성한 뒤, 2회차 전체 작업공간 실행 대신 `openside-e2e` 크레이트만 단독 재실행했다. 63개 바이너리/문서 타깃에서 76개 통과, 42개 실패, 14개 무시, 20개 실패 타깃을 기록했으며 Python 사전 요구사항 누락 마커는 0건이었다. 통과된 76건 중 2건은 사전 요구사항 조기 반환 스킵(패치된 네이티브 임포트 1건, Linux bwrap 스코프 픽스처 1건)이므로 76건 전체가 실제 동작 검증을 거친 것은 아니다. 해당 bwrap 픽스처는 실제 Linux 커널 사용자 네임스페이스에 의존하므로 정직하게 `cfg(target_os = "linux")`로 한정했다. 상세 결과는 별도 보고서에 기록한다. Chromium 프로세스 크래시가 다수의 정리(cleanup) 실패 원인을 설명하지만, 계정·채널·인증·작업·mock 관련 실패가 Chromium 크래시 때문에 발생했는지는 입증되지 않았으며 전체 통과를 의미하지 않는다.
- 게스트 `opensided` 및 `openside` CLI 실제 스크립트 스모크 테스트에서 15개 명령이 모두 exit 0으로 완료되었다. `sandbox_unavailable` 경고가 표시되어도 CLI 스트리밍과 대기가 중단되지 않았으며, `OPENSIDE_AGENT_SCRIPT`로 미리 지정한 모의 모델 응답 픽스처가 실제 데몬과 CLI 간에 엔드투엔드로 정상 전달되어 `session.settled` 및 `idle` 상태에 도달함을 확인했다. 네이티브 Keychain 연동(`init`/`add`/`items`/`lock`/`unlock`/`status`/`remove`)이 정상 작동하여 래핑 항목 생성과 `vault format 2`를 확인했고, 정리 후 자체 래핑 계정이 삭제됨(`security 44`)을 확인했다. 브라우저가 루프백 페이지를 정상적으로 열었고, 스냅샷에 실제 헤딩이 포함되었으며, PNG 파일 생성 후 close까지 성공적으로 완료되었다. 스크립트 실행이 끝난 뒤 출력된 1440x900 PNG 파일을 육안으로 확인했다. 이 검증은 동일 SSH 로그인 세션 내 잠금 해제와 상속된 환경 변수, 실제 게스트 `HOME` 구성을 바탕으로 이루어졌으며, 문서에 비밀번호나 키 데이터를 기록하지 않는다. 이 결과가 네이티브 Chromium의 정상 종료 크래시 해결이나 Mac 패치 `AgentGateway`/ARM/`launchd` 검증을 뜻하지는 않는다.
- 미검증 영역: Apple Silicon 네이티브 실행, macOS 환경의 패치된 AgentGateway(현재 Linux 전용 구현), launchd 서비스 등록 및 구동, macOS 실환경 데스크톱 확장 프로그램/네이티브 메시징 동작. 기본 브라우저의 루프백 감지 실패는 기본 데몬/브라우저/Keychain 스모크 테스트와 별개로 기록하며, 패치된 브라우저 감지 인증을 통과한 것으로 서술하지 않는다.

## Windows

Windows 런타임은 지원하지 않는다. VS2022 및 stable 1.99.0 MSVC 기반 네이티브 Windows 11 build 26100.1742 환경에서 작업공간 전체 타깃 빌드와 링크를 완료했다. 초기 전체 Rust 테스트에서는 475개 통과, 40개 실패를 기록했으나, Unix/Linux 전용 픽스처 스코프를 명확히 분리하고 이식성을 수정한 뒤 이전에 실패했던 9개 타깃을 집중 검사하여 모두 통과했다. 미지원 데몬 및 상태 경로는 패닉 없이 문서화된 Unix 트랜스포트 오류를 반환한다. 컴파일 성공이나 인메모리 API 테스트 통과를 정상 작동하는 Windows 데몬으로 소개하지 않는다.

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

네이티브 데몬/CLI/Keychain 실행을 검증했으며, macOS `fstatfs` 검사는 구현 및 검증을 완료했다. 현재 남아 있는 작업은 다음과 같다.

- 실제 stable 툴체인, 빌드/테스트 매트릭스 및 정직한 사전 요구사항 스킵 집계를 반영한 네이티브 macOS CI 구성
- 기존의 선택적 bwrap 경고와 필수 샌드박스 fail-closed 정책을 유지하며 보안을 약화하지 않는 macOS 자격 증명 보호 호스트 셸 백엔드 및 Seatbelt 제품 정책 수립
- 네이티브 Chrome CfT 종료 시 크래시 근본 원인 규명, 그리고 앞서 명시한 미검증 영역(패치된 `AgentGateway` Linux 전용 구현 대응, Apple Silicon 네이티브 실행, `launchd` 서비스 등록, 실제 데스크톱 네이티브 메시징 연동)에 대한 검증

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

## 브라우저 임포트 SQLite 스냅샷

다음 내용은 스냅샷 경계에 관한 기술 문서이다.

- `openside-import/src/db.rs`는 데이터베이스, WAL, SHM, 저널을 각각 독립 복사하지 않고 통상 rusqlite의 SQLite 온라인 백업 API를 사용한다. 일반적인 소스 읽기에는 `mode=ro`, `immutable=0` 및 번들 Unix/Windows VFS의 `readonly_shm=1`을 적용하며, `query_only`와 읽기 트랜잭션으로 체크포인트되지 않은 WAL 페이지를 포함한 특정 커밋 시점을 고정한다. 사전 점검(preflight)으로 안정성을 검증한 소스를 상대로는 소스 SQL 쓰기, WAL 체크포인트, 저널 복구, 공유 메모리 생성을 시도하지 않는다. SQLite는 읽기 전용 메인 데이터베이스라도 누락된 WAL을 생성할 수 있다. 따라서 유효한 WAL 헤더를 갖추었으나 WAL, SHM, 저널이 모두 없는 데이터베이스의 경우, 소스 SQLite 연결을 열기 전에 비공개 스크래치 디렉터리로 크기가 제한된 메인 파일 복사를 먼저 진행한다. 이 스크래치 복사본만 일반 방식으로 열리므로, SQLite가 새로 만드는 사이드카도 비공개 상태로 유지된다. 복사 전후로 새 `O_NOFOLLOW` 경로 열기를 포함해 소스의 inode, 크기, 수정 시각, 100바이트 헤더가 일치해야 하며, 사이드카가 계속 존재하지 않아야 한다. 소스가 변경된 경우 스냅샷을 폐기하고 1회 재시도한다. 사이드카가 이미 존재하는 소스는 이 대체 경로를 절대 타지 않으며, 소스는 `immutable=1`을 결코 사용하지 않는다. 이 검사는 일반적인 작성기 동작을 감지하기 위한 것이며, 메타데이터를 유지하거나 일시적으로 경로를 바꾸는 동일 사용자의 악의적인 프로세스까지 막지는 못한다. 일반 경로에서 SQLite는 여전히 권고형 읽기 잠금을 걸어 체크포인트를 지연시킬 수 있고, 파일시스템 읽기 과정에서 접근 시각 메타데이터가 갱신될 수 있다.
- 기존 소스 데이터베이스와 각 사이드카, 그리고 SQLite 페이지 수와 페이지 크기로 계산한 논리적 스냅샷은 모두 `MAX_SOURCE_BYTES`(2GiB) 상한을 넘지 못한다. 상한을 초과하면 타입이 지정된 `ImportError::SourceTooLarge` 오류를 반환한다. 소스 및 대상 연결의 비지 타임아웃은 2초이다. 백업 단계는 한 번에 128페이지를 복사하고, 비지/잠금 상태에서는 50ms 대기 후 최대 3회까지 재시도하며, 백업 경과 시간이 30초 한도에 이르면 중단한다. 이는 이미 진행 중인 커널 또는 파일시스템 작업을 강제로 중단하는 것은 아니다.
- 쓰기 가능한 대상 파일은 Unix에서는 비공개 0700 임시 디렉터리 안의 새 0600 파일이고, Windows에서는 소유자 전용 DACL 파일·디렉터리이다. 스냅샷이 드롭되면 해당 디렉터리와 함께 삭제된다.
- Linux 환경에서 소스 사전 점검 열기는 루트 디렉터리 핸들로부터 상위 경로들을 검증한 뒤, 검증된 프로필 및 파일 부모 디렉터리 핸들을 기준으로 `openat2(RESOLVE_BENEATH | RESOLVE_NO_SYMLINKS)`를 호출해 소스를 연다. 이 인터페이스가 없는 커널에서는 모든 경로 구성 요소에 `O_NOFOLLOW`를 지정한 디렉터리 핸들 순회 방식으로 대체한다. `fstat` 전에 `O_NONBLOCK`을 설정하고 일반 파일만 허용하므로, 정적 FIFO나 상위 심볼릭 링크는 즉시 실패한다. SQLite 자체 연결 역시 `SQLITE_OPEN_NOFOLLOW`를 사용한다.
- Linux 잔여 위험: 표준 SQLite VFS는 경로를 정규화하여 소스와 사이드카를 스스로 다시 열며, Rust에서 검증한 파일 디스크립터를 넘겨받아 사용하지 않는다. 프로필 트리를 변경할 수 있는 동일 사용자 프로세스라면 사전 점검과 SQLite 재열기 시점 사이에 상위 경로, 데이터베이스, 사이드카를 바꿔칠 가능성이 여전히 남아 있다. 열기 전후의 데이터베이스 inode 검사로 일부 치환을 감지할 수는 있으나 디스크립터의 동일성까지 증명하지는 못하며, 이처럼 뒤늦은 SQLite 경계에서 교체된 FIFO는 비지 타임아웃과 무관하게 블로킹될 수 있다. 사전 점검 후 소스 WAL이 삭제되면 표준 VFS가 재열기 시점에 WAL 재생성을 시도할 수도 있으며, 소스의 `mode=ro` 설정만으로는 이 경쟁 상태를 독립적으로 막지 못한다. 커스텀 VFS는 설치되어 있지 않으며, Linux 임포트가 경로 경쟁 상태를 원자적으로 완전히 제거했다고 보장하지는 않는다. 아울러 `readonly_shm`으로도 같은 프로세스 안의 다른 SQLite 연결이 소유한 기존 쓰기 가능 공유 메모리 매핑을 읽기 전용으로 바꿀 수는 없다.
- macOS 잔여 위험: 최초 소스 열기는 리프에 `O_NOFOLLOW | O_NONBLOCK`을 사용하고 일반 파일이 아닌 대상을 거부하지만, Linux처럼 루트에 고정된 상위 심볼릭 링크 배제 순회를 제공하지는 않는다. 표준 SQLite VFS는 여전히 정규화된 경로와 사이드카를 다시 열기 때문에 상위 경로 치환과 동일 사용자 재열기 경쟁 상태가 남아 있다. 네이티브 macOS 환경에서 `db::tests`를 실행한 결과 닫힌 WAL, 동시 WAL 스냅샷, 고아 SHM, 크기 한도, FIFO 거부 등 적용 가능한 8개 테스트를 모두 통과했다. Linux 상위 순회 테스트 2건은 macOS에서 컴파일되지 않으며, 네이티브 테스트 통과가 SQLite의 경로 기반 재열기 시점에서 원자적 보호를 확립하는 것은 아니다.
- Windows 소스 사전 점검은 로컬 드라이브 루트부터 프로필을 거쳐 리프 파일에 이르는 모든 상위 경로에 대해 `CreateFileW(FILE_FLAG_OPEN_REPARSE_POINT)`를 사용하며, 디렉터리에는 `FILE_FLAG_BACKUP_SEMANTICS`를 적용한다. 핸들 정보와 `GetFileType`을 통해 정션과 심볼릭 링크를 포함한 리파스 포인트, 소스 파일로 지정된 디렉터리, 비디스크 객체를 모두 거부한다. UNC/장치 네임스페이스, 상위 경로 순회(`..`), 대체 데이터 스트림도 거부한다. 소스 핸들은 읽기, 쓰기, 삭제 접근 권한을 공유하며, 임포터가 브라우저를 강제 종료하는 일은 없다.
- Windows 공유 및 잠금 위반은 타입이 지정된 `ImportError::SourceLocked`로 변환된다. API의 안전 안내 문구는 `Close <browser> and retry importing this browser data.`이다. 특히 Chromium이 독점 점유 중인 Cookies 파일은 우회하지 않는다. 온라인 백업에는 동일한 크기, 비지, 기한 한도가 적용된다. Windows VFS는 읽기 전용 SHM에서도 `OPEN_ALWAYS`를 사용하므로, SHM이 없는 기존 WAL은 SQLite가 열기 전에 거부한다. 콜드 복사 동일성 검증에는 볼륨 일련번호, 파일 인덱스, 크기, 최종 수정 시각, 100바이트 SQLite 헤더가 포함된다.
- Windows 스크래치 디렉터리는 OS 무작위 이름을 사용해 보호된 상속 가능 OWNER RIGHTS DACL(`D:P(A;OICI;FA;;;OW)`)과 함께 원자적으로 생성된다. 소스 데이터를 쓰기 전에 디렉터리와 새로 생성된 스냅샷의 DACL을 모두 검사하며, ACL 저장을 지원하지 않는 환경에서는 즉시 차단(fail-closed)된다.
- Windows 잔여 위험: 사전 점검 동안 상위 경로 핸들을 유지하고 리프를 연 뒤 핸들 동일성을 다시 확인한다. 이 검사는 정적 리파스 포인트를 거부하고 일부 동시 치환을 감지하지만, 핸들 기준의 원자적 순회는 아니다. `CreateFileW`와 이후 진행되는 SQLite의 경로 기반 재열기 사이에는 앞서 언급한 동일 사용자 네임스페이스 경쟁 상태가 여전히 존재하며, 삭제된 사이드카가 SQLite에 의해 재생성되는 현상도 포함된다. Windows 소스 리더를 지원한다고 해서 Windows 데몬이나 대상 브라우저 런타임을 지원한다는 뜻은 아니다. Unix와 마찬가지로 임포터 프로세스 내에 동일 소스를 향한 기존 쓰기 가능 SQLite 연결이 있다면 `readonly_shm`에도 불구하고 쓰기 가능 SHM 매핑을 재사용할 수 있다. 따라서 Windows 라이브 WAL 회귀 테스트는 가상 브라우저 작성기를 별도 프로세스에서 실행하고, 소스 데이터베이스, WAL, SHM의 정확한 바이트 단위 비교를 유지한다.
- Windows 검증 (2026-10-13): 네이티브 MSVC 환경에서 `cargo test -p openside-import --lib --locked -j 2 -- --test-threads=1 --show-output` 실행 시 70개 테스트를 통과했다. 무시 처리된 1개 하네스 항목은 부모 테스트를 통해 격리된 WAL 작성기로 명시 실행되었다. 정션 및 파일·디렉터리 심볼릭 링크 거부, 독점 소스 재시도 안내, 읽기·쓰기·삭제 공유, 소유자 전용 스크래치 DACL 및 정리, 2GiB 상한, 소스 스탬프, 콜드 WAL 복사, 라이브 WAL 바이트 보존, 가상 브라우저 파서 검증을 모두 수행했다. 초기의 동일 프로세스 WAL 픽스처는 공유 메모리 읽기 마크를 변경했으나, 외부 프로세스 픽스처는 바이트 일치 단언을 완화하지 않고도 해당 불일치를 해결한다. 실제 브라우저 프로필은 사용하지 않았다. 아울러 `OPENSIDE_XWIN=/tmp/openside-xwin`, `cargo +nightly-2026-08-08 xwin check -p openside-import --tests --locked -j 2 --target x86_64-pc-windows-msvc -Zbuild-std=std,panic_abort --target-dir /tmp/openside-xcheck`를 통한 크로스 컴파일도 통과했다. 이 검증은 임포터 컴포넌트만을 보증하며, 미지원 상태인 Windows 데몬이나 브라우저 런타임까지 보증하는 것은 아니다.

