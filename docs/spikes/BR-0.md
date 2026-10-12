# BR-0. ungoogled-chromium 핀 빌드 + P1 첫 패치

결론: **가능**. 이 머신(Arch, 32코어, 60GB RAM, sudo 없음)에서 ungoogled-chromium-portablelinux `155.0.8059.39-1` 을 Docker 없이 네이티브로 빌드했고, P1 첫 패치(`browser/patches/0001-openside-p1-tab-ownership-hook.patch`)를 적용한 재빌드와 동작 스모크도 통과했다.

## 1. 기준
| 항목 | 값 |
| --- | --- |
| 베이스 | `ungoogled-software/ungoogled-chromium-portablelinux` 태그 `155.0.8059.39-1` (commit `025d99db1673a21992534551598b73ca79360762`) |
| Chromium 소스 | `chromium-155.0.8059.39-lite.tar.xz` (commondatastorage.googleapis.com, 해시 검증 통과) |
| 근거 | chromiumdash Linux Stable = `155.0.8059.39` (2026-10-11 확인) |
| 작업장 | `$OPENSIDE_BROWSER_BUILD_DIR`(저장소 외부 경로이며 `portablelinux/`와 `tools/`를 포함함). 빌드 구동기인 `br0.sh`와 `br0-guarded.sh`는 로컬 스크립트였으므로 저장소에 포함되지 않음. |
| GN 인자 | ungoogled `flags.gn` + portablelinux `flags.linux.gn` + `target_cpu = "x64"`. `is_official_build=true`, `is_debug=false`, `symbol_level=0`, `use_sysroot=true` |
| 가드 | `systemd-run --user --scope -p MemoryMax=24G -p CPUWeight=20 nice -n 19 ionice -c3`, `ninja -j14`, MemAvailable < 8GiB 에서 SIGSTOP / > 12GiB 에서 SIGCONT |

이 머신에서 portablelinux 절차를 그대로 쓰지 못한 지점과 대체(자세한 내용은 `browser/BUILD.md`):
- gperf: Chromium CIPD `infra/3pp/tools/gperf/linux-amd64` `version:3@3.2`.
- esbuild: GitHub `evanw/esbuild` `v0.28.2` 를 `GOPROXY=off` 로 로컬 빌드.
- devtools `error TS9025`: `fixups` 단계에서 두 매개변수에 `| undefined` 명시.

## 2. 전체 빌드
| 시도 | 구간 | 결과 |
| --- | --- | --- |
| 1 | 15:14:26~15:14:32 | `.../usr/bin/esbuild ... missing` 으로 즉시 실패 → esbuild 대체 |
| 2 | 15:15:36~15:24:59 | `[18414/57115]` 에서 `error TS9025` 2건으로 실패 → `fixups` 추가 |
| 3 | 15:32:02~16:57:06 | `[38702/38702] LINK ./chrome`, `=== [guard] ... exit=0` |

- 3차 시도는 시도 2에서 끝난 단계들을 이어받았다. 컴파일 시간은 2차 9분 23초 + 3차 85분 4초 = 약 94분이다. 3차 중 감시자가 MemAvailable 부족으로 두 번 SIGSTOP 했다(15:40:17~15:45:32, 15:45:47~15:47:32, 합계 약 7분).
- 산출물: `out/Default/chrome` 531,621,048 바이트, `out/Default/chromedriver` 34,850,640 바이트. 작업장 디스크 사용량 `25G`(`du -sh`, 상한 150GB).

## 3. 버전과 헤드리스 스모크
```text
$ out/Default/chrome --version
Chromium 155.0.8059.39
$ out/Default/chromedriver --version
ChromeDriver 155.0.8059.39 (3ff7ac5a9224be9156d7f8703a06e22890aafd34-refs/branch-heads/8059@{#935})
```
`127.0.0.1` 임시 HTTP 서버(`python3 -m http.server`, 빈 포트)와 `/tmp` 임시 프로필로 실행했다.
```text
$ chrome --headless=new --no-first-run --user-data-dir=/tmp/br0-smoke.LFJF/profile3 --dump-dom http://127.0.0.1:55533/index.html
<html><head><title>openside-br0-smoke</title></head><body><h1 id="h">hello-br0</h1></body></html>
exit=0
$ chrome --headless=new --user-data-dir=/tmp/br0-smoke.LFJF/profile4 --screenshot=shot.png --window-size=1440,900 http://127.0.0.1:55533/index.html
shot_exit=0
shot.png: PNG image data, 1440 x 900, 8-bit/color RGB, non-interlaced
```
스크린샷에 `hello-br0` 제목이 그려진 것을 눈으로 확인했다. 첫 시도에서 포트 18765 가 다른 프로세스에 이미 잡혀 있어 404 페이지를 받았고, 빈 포트를 골라 다시 실행했다.

## 4. 증분 빌드 시간
| 측정 | 명령 | 결과 |
| --- | --- | --- |
| 변경 없음 | `ninja -C out/Default chrome` | `ninja: no work to do.`, 약 12.8초(매니페스트 확인 시간) |
| 파일 1개 touch | `touch chrome/browser/devtools/protocol/target_handler.cc` 후 가드 아래 `ninja -j14 chrome` | `[1/3] CXX ... target_handler.o` → `[3/3] LINK ./chrome`, **80초** |
| P1 적용 | `OPENSIDE_NINJA_JOBS=14 browser/build.sh <src>` | GN 재생성 + 새 `.cc` 2개 + `target_handler.cc` + 링크, exit 0, **95초** |

링크(`chrome` 약 530MB)가 증분 시간 대부분을 차지한다.

## 5. P1 첫 패치 적용과 동작 확인
`browser/apply.sh` 로그(`browser/build.sh` 가 호출):
```text
checking file chrome/browser/BUILD.gn
checking file chrome/browser/devtools/BUILD.gn
checking file chrome/browser/devtools/protocol/target_handler.cc
patching file chrome/browser/BUILD.gn
patching file chrome/browser/devtools/BUILD.gn
patching file chrome/browser/devtools/protocol/target_handler.cc
Applied openside overlay: aa694c101017a4a44a6c68b43c76c6f3d046aa1ff65778c586cd197d5d824543
[0/1] Regenerating ninja files
[1/55] CXX obj/chrome/browser/openside/openside/focus_arbiter.o
[3/7] CXX obj/chrome/browser/openside/openside/tab_ownership.o
[4/7] CXX obj/chrome/browser/devtools/devtools/target_handler.o
[6/7] LINK ./chrome
```
재빌드 뒤 `ninja: no work to do.` 와 `Chromium 155.0.8059.39` 를 다시 확인했다.

동작 스모크: 일회용 Node 스크립트로 패치된 `chrome` 을 `--headless=new`, `/tmp` 임시 프로필, `--remote-debugging-port=0` 으로 띄운다. `127.0.0.1` 페이지에 CDP `Target.createTarget` 을 세 번 호출해, 표식 플래그를 줬을 때와 안 줬을 때를 비교했다.
```text
marker=false normal=ok targetId=53F3ECDB… | hidden=ok targetId=752A42B1… | minimized=ok targetId=9CD5AD4B…
marker=true normal=ok targetId=4437ED15… | hidden=error: openside agent targets cannot use the untagged hidden path | minimized=error: openside agent targets cannot change window state
```
- 표식 없는 일반 실행은 원래 Chromium 동작과 같다(세 호출 모두 성공).
- `--openside-agent-session-id=smoke-session` 으로 띄운 전용 브라우저에서는 일반 탭 생성이 성공하고(훅이 `TabOwnership` 을 붙이는 경로), 태그를 피하는 `hidden` 경로와 창 상태 변경은 거부된다.
- 종료 후 임시 프로필 디렉토리와 chrome 프로세스가 남지 않았다(`ls`·`pgrep` 결과 0).

## 6. 확인하지 않은 것
- `background=true`/`focus=false` 강제가 실제 데스크톱 포커스를 지키는지: 헤드리스만 실행해서 `unverified(no-desktop-test)`.
- 생성된 `WebContents` 에 붙은 `TabOwnership` 값 자체: 이를 밖으로 읽는 경로(게이트웨이 소켓)가 아직 없어 `unverified(not-implemented)`. 위 스모크는 훅 경로가 컴파일되고 크래시 없이 실행되며 거부 분기가 동작함을 보여준다.
- C++ 단위 테스트(`unit_tests`/`browser_tests`): 대상 빌드를 하지 않아 미실행.
- 다른 머신에서의 재현.
