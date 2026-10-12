# 브라우저 데이터 가져오기 (Import)

상태: **구현됨**. 관련: B-1. 코드: `crates/openside-import`(읽기·복호화·쓰기), `crates/openside-cli/src/import.rs`(`openside import`). 테스트: `crates/openside-import/src/tests.rs`, `crates/openside-cli/src/tests/import.rs`, `crates/openside-e2e/tests/import.rs`.

Chrome·Chromium·Firefox 프로필 디렉터리에서 기록(history)·북마크·쿠키를 읽어 openside 브라우저 프로필 하나로 옮긴다. 데몬 없이 실행된다.

## 1. 사용법
```
openside import --from <chrome|chromium|firefox>:<프로필 디렉터리> --into <프로필 이름> \
  [--what history,bookmarks,cookies] [--dry-run] [--keyring-password-cmd '<명령>'] [--config PATH]
```
- `--from` 의 디렉터리는 브라우저 프로필 자체다(Chrome 의 `Default`, Firefox 의 `xxxx.default-release`). 이 디렉터리 안의 `History`, `Bookmarks`, `Network/Cookies`(없으면 `Cookies`), `places.sqlite`, `cookies.sqlite` 를 읽는다.
- `--into` 이름은 영소문자·숫자·`_`·`-` 1–64자, 영소문자나 숫자로 시작한다.
- `--what` 을 생략하면 소스에 있는 범주를 모두 가져오고, 없는 파일은 건너뛰었다고 알린다(하나도 없으면 오류). 명시하면 없는 파일은 오류다.
- `--dry-run` 은 읽고 복호화까지 해서 개수만 보고한다. 아무것도 쓰지 않고 브라우저도 띄우지 않는다.
- `--json` 은 보고서를 JSON 한 줄로 낸다. 쿠키 값과 키링 비밀번호는 어떤 출력에도 나오지 않는다.

## 2. 프로필과 저장 위치
프로필 이름은 데몬 data 디렉터리 아래 다음 위치로 대응한다.

| 대상 | 위치 |
|---|---|
| 브라우저 user-data-dir | `<data>/browser-profiles/<이름>`. 이름이 `default` 이면 데몬의 기존 `<data>/browser-profile` |
| 가져온 기록 | `<data>/browser-imports/<이름>/history.<소스>.jsonl` (방문 한 건이 한 줄: `url`, `title`, `visited_at_ms`) |
| 가져온 북마크 | `<data>/browser-imports/<이름>/bookmarks.<소스>.json` (`bookmarks[]`: `url`, `title`, `folder[]`, `added_at_ms`) |

`<소스>` 는 `chrome`, `chromium`, `firefox`. 같은 소스를 다시 가져오면 파일을 통째로 원자적으로 바꾸므로 중복이 쌓이지 않는다(쿠키도 같은 이름·도메인·경로는 덮어쓴다). 가져오기는 한 프로필만 건드린다: 프로필 A 로 가져와도 B 의 디렉터리·쿠키·기록·북마크는 바뀌지 않는다.

결정(근거):
- **쿠키**는 대상 프로필의 Chromium 을 헤드리스로 띄워 CDP `Storage.setCookies` 로 넣고 정상 종료해 디스크에 반영한다. 암호화는 대상 Chromium 이 자기 키로 하므로 프로필이 그 브라우저에서 그대로 유효하다. 쿠키 DB 를 직접 쓰지 않는다. 쿠키 범주가 포함되고 `--dry-run` 이 아니면, 소스에 쿠키가 없더라도 읽기 전에 대상 프로필이 사용 중인지 먼저 확인한다. 디버깅 포트 없이 떠 있는 Chromium 도 잡도록 `SingletonLock`(`<호스트>-<pid>` 링크, 살아 있는 pid 만 인정)과 `DevToolsActivePort` 를 모두 보고, 사용 중이면 `profile ... is in use by a running browser` 로 거절한다. 기록·북마크만 가져오려면 `--what history,bookmarks` 로 이 검사를 피한다. 브라우저 실행 파일은 `browser.executable`/`OPENSIDE_BROWSER`/PATH 순으로 찾는다.
- **기록**은 메모리의 `history` 색인에 넣지 않는다. 그 색인은 프로필 구분이 없어 A 로 가져온 기록이 B 에서도 검색되고, 30일 뒤 만료되며, 에이전트가 검색한다. 프로필별 JSON-lines 로 따로 둔다. http/https 주소만 가져온다(`chrome://`, `about:` 등은 건너뜀).
- **북마크**는 Chromium 의 `Bookmarks` 파일에 합치지 않는다. Chromium 이 체크섬으로 검증하고 다시 쓰는 파일이기 때문이다. `javascript:` 북마크릿(대소문자 무관)과 Firefox 의 `place:` 질의는 건너뛰고 건너뛴 개수에 센다. Firefox 의 `tags________` 아래 항목은 태그 연결일 뿐이라 북마크로 세지 않는다.
- 세션 쿠키(만료 없음)는 대상 브라우저가 가져오기 직후 종료되므로 사라진다. 그래서 가져온 시각부터 7일짜리 지속 쿠키로 바꿔 넣는다. 이미 만료된 쿠키는 건너뛴다.
- 파티션 쿠키(Chrome `top_frame_site_key`)와 Firefox 컨테이너·파티션 쿠키(`originAttributes`)는 대상 프로필에 대응하는 격리 컨텍스트가 없어 건너뛰고 개수만 센다. `Secure` 가 없는 `SameSite=None` 쿠키는 Chromium 이 거절하므로 `SameSite` 속성을 뺀다. 그래도 Chromium 이 거절한 쿠키는 건너뛴 개수에 더하고 호스트와 이름만 보고서에 나온다.

## 3. 소스를 읽는 방법
- 소스 SQLite 파일은 `-wal`·`-shm`·`-journal` 과 함께 비공개 임시 디렉터리로 복사한 뒤 복사본을 연다. 복사본은 소스 권한과 무관하게 새로 만든 0600 파일이라, 읽기 전용이거나 world-readable 인 소스도 처리하고 SQLite 가 WAL·저널을 복구할 수 있다. 브라우저가 실행 중이어서 잠겨 있거나 체크포인트 전 WAL 에 쓴 내용이 있어도 읽을 수 있고, 소스 파일은 수정하지 않는다(테스트가 가져오기 전후 파일 내용을 비교한다). 실행 중인 브라우저가 복사 도중 `-wal`·`-journal` 을 지우면 없는 것으로 본다. 소스 파일이 심볼릭 링크이거나 일반 파일이 아니면 따라가지 않고 읽기 오류로 거절하며, 접근 권한 오류는 `찾을 수 없음` 이 아니라 읽기 오류로 보고한다. 임시 복사본은 끝나면 지운다.
- 모든 범주를 먼저 읽고 복호화한 다음에야 대상에 쓴다. 기록은 읽는 동안 임시 파일에 모았다가, 쿠키 적용까지 끝난 뒤에야 프로필 저장소(`browser-imports/<이름>`)를 만들고 옮긴다. 읽기·복호화·사용 중 검사에서 실패하면 대상에는 아무것도 생기지 않는다. 저장소 디렉터리는 0700, 파일은 `O_EXCL` 0600 임시 파일을 `sync` 한 뒤 `rename` 으로 바꾸므로 고정 이름 임시 파일이나 심볼릭 링크를 따라가지 않고 동시 가져오기가 서로 덮지 않는다.
- Chrome 시간(1601년 기준 마이크로초)과 Firefox 시간(마이크로초)은 Unix 밀리초로, Firefox 쿠키 만료는 초 또는 밀리초(값이 1e11 이상이면 밀리초)를 초로 맞춘다.

### Chrome/Chromium 쿠키 복호화 (Linux)
`encrypted_value` 의 앞 3바이트가 암호화 방식이다.

| 접두 | 키 | 동작 |
|---|---|---|
| `v10` | PBKDF2-HMAC-SHA1(`peanuts`, `saltysalt`, 1회, 16바이트), AES-128-CBC, IV 는 공백 16개, PKCS#7 | 자동 복호화 |
| `v11` | 같은 파생, 비밀번호는 데스크톱 키링의 `Chrome Safe Storage`/`Chromium Safe Storage` | `--keyring-password-cmd` 로 비밀번호를 주어야 복호화 |
| 그 외(`v20` 등) | — | 오류 `unknown encryption version` |

- `--keyring-password-cmd` 는 셸 없이 직접 실행하는 명령줄이다(공백과 따옴표로만 나눈다). 표준 출력 끝의 개행을 뗀 값이 비밀번호이고, 30초 안에 끝나야 한다. `v11` 쿠키를 만났을 때만 실행한다. 명령은 사용자가 정하며(예: `secret-tool` 로 `Chrome Safe Storage` 항목을 조회하는 명령), 테스트는 키링에 닿지 않는다. Vault 의 플랫폼 잠금 추상화는 쓰지 않는다. 브라우저 키링 항목과 Vault 래핑 키는 별개이기 때문이다.
- 쿠키 DB 스키마 버전(`meta.version`, 정수·문자열 저장 모두 읽는다)이 24 이상이면 복호화한 값 앞에 호스트의 SHA-256(32바이트)이 붙어 있다. 이 값이 맞지 않거나 복호화가 실패하면, 키링 키가 아직 어떤 쿠키도 복호화하지 못한 상태에서는 틀린 비밀번호(`v11`)로 보고, 이미 하나라도 성공했거나 `v10` 이면 손상으로 본다. 암호문 길이가 AES 블록(16바이트) 배수가 아니면 비밀번호와 무관하게 손상이다.
- 키링 비밀번호, 파생 키, 복호화 중간 버퍼, 쿠키 값은 메모리에서 지운다(`zeroize`, best effort). 키링 명령은 별도 프로세스 그룹으로 실행하고 제한 시간을 넘기면 그룹 전체를 종료한다. 명령이 끝난 뒤에도 자식이 표준 출력을 붙잡고 있으면 제한 시간 안에 실패한다.
- 복호화할 수 없는 쿠키가 하나라도 있으면 쿠키 범주 전체가 오류로 끝난다. 오류에는 호스트와 쿠키 이름만 나온다.

플랫폼: 키 파생은 실행 중인 OS 를 따른다. macOS 는 Keychain 비밀번호와 1003회 반복으로 파생하는 경로가 컴파일되지만(`cfg(target_os = "macos")`) 실행해 보지 못했다(`unverified`). Windows(DPAPI·AES-GCM) 쿠키 복호화는 지원하지 않는다. 자세한 내용은 [PLATFORMS.md](../PLATFORMS.md).

## 4. 오류 메시지
| 상황 | 메시지(요지) |
|---|---|
| `--from` 형식 오류 | `invalid source ...: expected <chrome\|chromium\|firefox>:<profile directory>` |
| 프로필 디렉터리·파일 없음 | `Chrome history not found at <경로>: is this a Chrome profile directory?` |
| 읽기 권한 없음 | `cannot read history at <경로>: <OS 오류>` |
| 잠긴 DB | `history database <경로> is locked; close Chrome and import again` |
| 손상·SQLite/JSON 아님 | `history database <경로> is corrupt or not a SQLite file: ...` |
| 알 수 없는 스키마 | `unsupported Chrome history format in <경로>: table `visits` is missing` (열이 없을 때도 같은 형식) |
| 알 수 없는 쿠키 암호화 | `cookie `<이름>` for <호스트> uses unknown encryption version `v20` ...` |
| `v11` 인데 비밀번호 없음 | `... encrypted with the OS keyring key (v11); pass --keyring-password-cmd ...` |
| 틀린 키링 비밀번호 | `the keyring password is wrong: it does not decrypt cookie ...` |
| 손상된 `v10` 쿠키 | `cookie ... is corrupt: its encrypted value does not decrypt` |
| 키링 명령 실패 | `keyring password command failed: ...` |
| 대상 프로필 사용 중 | `openside profile `<이름>` is in use by a running browser; stop it before importing cookies` |
| 쿠키를 쓸 브라우저 없음 | `importing cookies needs a Chromium-compatible browser ...` |

## 5. 테스트와 안전
- 모든 테스트는 합성 fixture 로만 돌린다. `openside-import` 의 `testing` 기능이 실제 테이블 정의(Chrome `History`·`Cookies`, Firefox `places.sqlite`·`cookies.sqlite`)와 실제 `v10`/`v11` 암호화로 프로필을 만든다. 사용자 프로필은 읽지 않으며 테스트는 HOME 과 XDG 디렉터리를 임시 디렉터리로 바꾼다.
- E2E(`crates/openside-e2e/tests/import.rs`)는 `openside import` 로 Chrome·Firefox fixture 를 프로필 `alpha`·`beta` 에 가져온 뒤, 각 프로필로 에이전트 브라우저를 띄워 127.0.0.1 페이지가 받는 `Cookie` 헤더를 확인한다. 브라우저가 없으면 이유를 출력하고 건너뛴다.

## 6. 범위 밖
- Safari ZIP 가져오기, Windows 쿠키 복호화, 비밀번호·자동완성·확장 프로그램 가져오기.
- 가져온 기록을 에이전트가 검색하거나 사용하는 경로. 이 문서는 데이터를 프로필에 옮기는 데까지를 다룬다.
- 실행 중인 데몬의 브라우저에 대한 쿠키 가져오기(프로필 사용 중으로 거절한다).

**웹 UI 가져오기.** 웹 첫 실행 안내는 데몬 API `GET /v1/import/profiles`와 `POST /v1/import`(`admin` 스코프)를 통해 동일한 가져오기 엔진을 실행한다. 이 방식은 표준 위치에서 탐색된 프로필 id만 지정할 수 있으며 openside 기본 프로필로만 데이터를 가져온다. 네트워크 API로는 키링 비밀번호 실행 명령을 전달받을 수 없으므로 키링 암호화(v11) 쿠키는 가져올 수 없다. 자세한 내용은 `docs/specs/onboarding.md`를 참고한다.
