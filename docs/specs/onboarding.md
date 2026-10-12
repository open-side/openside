# 첫 실행 안내 (Onboarding)

상태: **구현됨**. 관련: F-5. 코드: 웹 `web/src/onboarding/`, 데몬 `crates/opensided/src/onboarding.rs`·`crates/opensided/src/import_api.rs`, 프로토 `crates/openside-proto/src/onboarding.rs`, 탐색 `crates/openside-import/src/discover.rs`. 테스트: `web/src/onboarding/OnboardingPage.test.tsx`, `web/src/onboarding/OnboardingGate.test.tsx`, `crates/opensided/src/tests/onboarding.rs`, `crates/openside-import/src/tests.rs`, `crates/openside-e2e/tests/onboarding.rs`.

처음 방문한 사용자에게 에이전트 연결, 브라우저 데이터 가져오기, Vault 설정, 휴대폰 페어링까지의 초기 설정을 안내하는 웹 UI다. 모든 단계는 데몬의 공개 API 엔드포인트만 사용해 동작하며, 데몬이 안내 완료 여부를 기억한다.

## 1. 흐름
- 경로는 `/welcome`이다. 단계는 `?step=welcome|agents|import|vault|pair|done` 쿼리 파라미터로 유지되므로 브라우저 뒤로 가기나 새로고침을 해도 현재 자리를 잃지 않는다.
- 안내가 열려 있는 동안 좌측 대화 목록은 숨겨지고, 모바일과 데스크톱 모두 공통 진한 글래스(strong-glass) 스타일의 집중형 전체 화면 카드로 표시된다.
- 전체 단계는 Welcome (개요) → Agents → Browser data → Vault → Phone → Done 순으로 진행된다.
- Welcome과 Done을 제외한 모든 단계는 `Skip this step` 버튼으로 건너뛸 수 있다. 해당 단계에서 실제 설정 작업을 마치면 버튼 레이블이 `Continue`로 바뀐다. `Back`을 누르면 이전 단계로 돌아가며, 상단의 `Skip setup`을 누르면 모든 과정을 한 번에 건너뛴다.
- 마지막 Done 단계에서는 완료한 작업(스킬이 설치된 에이전트, 가져온 프로필, Vault 설정, 페어링된 휴대폰)을 정리해 보여주고, 건너뛴 항목은 `Skipped`로 표시한다. `Open openside`를 누르면 초기 설정이 끝나고 메인 화면으로 진입한다.

## 2. 열리는 조건과 상태
- 안내는 데몬의 온보딩 상태가 `pending`일 때 `/`에 처음 접근하면 자동으로 한 번 열린다. 단, 현재 기기의 토큰에 `admin` 스코프가 없는 것이 확실한 경우에는 자동으로 열리지 않는다(토큰을 직접 붙여넣는 등 스코프를 미리 알 수 없는 상태라면 그대로 진입해 데몬에 확인한다).
- 대화 상세 화면으로 연결되는 딥링크나 기타 경로로 접근할 때는 사용자의 작업을 가로막지 않으며 안내를 자동으로 띄우지 않는다.
- 언제든 설정(Settings) 메뉴의 `Setup guide`를 눌러 다시 열 수 있다.
- 상태 조회 API: `GET /v1/onboarding` (`read` 스코프)은 `{state: pending|completed|skipped, updated_at}`을 반환한다.
- 상태 갱신 API: `PUT /v1/onboarding` (`admin` 스코프)은 `{state: completed|skipped}`를 받는다. `pending` 값은 400 `invalid_value`로 거절하므로, 안내를 다시 열더라도 완료 상태가 초기 상태로 되돌아가지 않는다.
- 상태는 `<data>/onboarding.json`에 원자적(atomic) 0600 파일 쓰기로 저장된다(`{state, updated_at}`). 파일이 없거나 손상되었으면 `pending`으로 간주한다(파일 손상 시 안내를 한 번 더 보여주는 것은 무해하나, 영원히 안내가 가려지는 오작동은 치명적이기 때문이다).
- 상단 `Skip setup`이나 마지막 단계의 `Open openside`는 상태가 여전히 `pending`일 때만 데몬에 상태를 기록한다. 따라서 설정에서 안내를 다시 열었다가 닫더라도 기존의 `completed` 상태가 `skipped`로 덮어써지지 않는다.
- 데몬이 권한 부족으로 요청을 거부하거나(403) 저장에 실패하면, 브라우저 로컬 스토리지(`openside.onboarding.dismissed`)에 상태를 기록해 방문할 때마다 안내가 다시 뜨지 않도록 막는다. 403 외의 저장 실패가 발생하면 토스트 메시지로도 사용자에게 알린다.
- 모든 기기가 데몬의 상태를 확인하므로, 어느 한 기기에서 설정을 마치면 다른 기기에서도 안내가 다시 뜨지 않는다.

## 3. 에이전트 감지와 스킬 설치
- `GET /v1/external-agents` (`admin` 스코프)는 기본 에이전트인 `claude`(Claude Code), `codex`(Codex), `omp`(omp)와 `config.toml`의 `[skills.agents]` 항목을 모두 나열한다.
- 각 항목 필드: `id`, `label`, `detected`, `skill_installed`, `skills_dir`.
- `detected`는 데몬의 `PATH`에 해당 에이전트 실행 파일이 있거나, 에이전트 스킬 디렉터리의 부모 디렉터리가 디스크에 존재할 때 true가 된다.
- `skills_dir`는 `openside skills install`과 동일한 탐색 규칙을 따른다(에이전트별 기본 디렉터리, `CLAUDE_CONFIG_DIR`·`CODEX_HOME`·`OMP_CODING_AGENT_DIR` 등의 환경 변수, `[skills.agents]` 설정 순으로 결정).
- `skill_installed`는 `<skills_dir>/openside-browser/SKILL.md` 파일이 실제로 존재할 때 true가 된다.
- `POST /v1/external-agents/{id}/browser-skill` (`admin` 스코프, 바디 `{force?: bool}`)은 내장 번들인 `openside-browser` 스킬을 설치하고 201 `{agent, skill, path, files}`를 반환한다. 이미 설치되어 있고 `force`가 false이면 기존 파일을 보존한 채 409 `exists`를 낸다. 목록에 없는 식별자는 404 `unknown_agent`, 디렉터리를 알 수 없으면 400 `skills_dir_unknown`으로 거절한다.
- 감지되지 않은 에이전트는 `Not found` 문구와 함께 `Install anyway` 버튼을 제공해 수동 설치를 지원한다. 안내가 열려 있는 도중 사용자가 터미널에서 직접 스킬을 설치한 경우에도 오류 없이 설치 완료 상태로 갱신된다.
- 스킬 설치 내역은 `audit.jsonl`에 기록된다(`tool`: `external_agent`, `decision`: `skill_installed`, 기기 id 및 에이전트 id 포함).

## 4. 가져오기
- `GET /v1/import/profiles` (`admin` 스코프)는 표준 경로에 있는 브라우저 프로필을 다시 탐색한다.
  - Linux: Chrome은 `$XDG_CONFIG_HOME` 또는 `~/.config` 아래 `google-chrome`, Chromium은 `~/.config/chromium` 및 snap 경로, Firefox는 `~/.mozilla/firefox` 및 snap·flatpak 경로.
  - macOS: 운영체제 표준 라이브러리 디렉터리.
  - Windows: 아직 자동 탐색을 지원하지 않는다.
- Chrome 프로필 이름은 `Local State`에서, Firefox는 `profiles.ini`에서 파싱하며, 심볼릭 링크 디렉터리는 보안상 건너뛴다.
- 각 항목 필드: `id`(`<브라우저>:<디렉터리명>`, 예: `chrome:Default`), `browser`, `name`, `available`(`history`, `bookmarks`, `cookies` 불리언 맵).
- `cookies_writable`은 시스템에서 Chromium 호환 실행 파일이 발견되어 쿠키를 쓸 수 있을 때만 true가 된다.
- `POST /v1/import` (`admin` 스코프)는 `{profile, what?, dry_run}`을 입력받는다.
  - `profile`은 탐색 목록에서 반환된 id만 지정할 수 있으며 절대 경로는 전달할 수 없다. 임의 경로를 통한 파일 탐색이나 읽기를 방지하기 위함이며, 등록되지 않은 id는 400 `unknown_profile`로 거절한다.
  - 가져오기 대상은 항상 openside의 기본 프로필(`<data>/browser-profile`)로 고정된다.
  - `what`을 생략하면 프로필이 가진 모든 범주를 가져오고 없는 파일은 건너뛴다. 범주를 명시했을 때 해당 파일이 없으면 오류가 발생하며, 빈 배열을 넘기면 400 `invalid_value`가 된다.
  - 응답은 CLI와 동일한 보고서 형식이다. 범주별 `read`, `imported`, `skipped`, `rejected`(호스트와 이름만 포함)와 소스 파일이 없을 때의 고정 `note` 문구로 구성되며, 쿠키 값이나 실제 파일 경로는 응답에 절대 노출되지 않는다.
  - `dry_run: true`는 파일 읽기와 복호화까지만 수행하고 디스크에 아무것도 쓰지 않으며 브라우저 프로필도 생성하지 않는다.
- UI에서는 동일한 대상(프로필 및 범주 선택)에 대해 `Preview`를 먼저 실행해야만 `Import` 버튼이 활성화된다. 프로필이나 범주를 바꾸면 기존 미리보기 결과는 즉시 무효화된다.
- 발생 가능한 오류: 422 `source_unreadable`·`source_locked`·`source_corrupt`·`source_unsupported`·`keyring_password_required`, 409 `profile_in_use`(에이전트 브라우저가 해당 프로필로 실행 중임)·`import_in_progress`, 503 `browser_unavailable`, 500 `import_failed`(상세 원인은 데몬 로그에만 기록).
- 키링 암호화(v11) 쿠키는 웹 API를 통해 가져올 수 없다. 네트워크 요청으로 키링 암호 확인 명령을 받아 실행하는 것은 원격 코드 실행(RCE) 위험을 초래하기 때문이다. 안내 화면은 이 제약을 사용자에게 명확히 안내하고 `Leave cookies out` 옵션을 제공한다. 키링 암호화 쿠키를 가져오려면 CLI의 `openside import --keyring-password-cmd`를 사용해야 한다.
- 가져오기 작업은 한 번에 하나만 실행된다. 네트워크 연결이 끊겨도 백그라운드에서 끝까지 실행되므로 쿠키가 불완전하게 반영되는 문제가 없다.
- 모든 미리보기와 가져오기는 감사 로그 `audit.jsonl`에 기록된다(`tool`: `import`, `decision`: `previewed` 또는 `imported`, 기기 id, 소스 id, 범주별 개수 포함. 쿠키 값은 제외).

## 5. Vault와 휴대폰 연결
- **Vault 단계**: `GET /v1/vault`로 상태를 읽어 `Not set up` / `Locked` / `Ready` / `Unavailable` 중 하나를 표시한다. `/vault` 링크를 누르면 안내 화면 위에 기존 Vault 설정 대화상자가 모달로 열리고, 닫으면 다시 이 단계로 복귀한다. 대화상자가 닫히는 이벤트를 직접 감지할 수 없으므로 화면은 2.5초마다 상태를 주기적으로 다시 확인한다. Vault가 잠겨 있거나 해제된 상태 모두 설정이 완료된 것으로 취급한다.
- **휴대폰 단계**: `POST /v1/pairing` 엔드포인트는 로컬 소켓 전용이므로 웹 브라우저에서는 페어링 코드를 직접 발급할 수 없다. 따라서 화면에 `openside pair --name "My phone"` 명령과 복사 버튼을 표시하고, `GET /v1/devices`를 3초마다 폴링한다. 이 단계가 열린 시점의 기기 목록에 없던 새 기기가 감지되면 연결된 휴대폰으로 판별하여 화면과 마지막 요약에 기기 이름을 표시한다. 또한 휴대폰과 컴퓨터가 동일한 로컬 네트워크에 있거나 Tailscale 같은 사설 네트워크로 연결되어 있어야 한다는 점을 함께 안내한다.

## 6. 권한과 보안 결정
- **가져오기와 스킬 설치에 TCP 통신 및 `admin` 스코프 허용(근거)**: 웹 UI는 로컬 루프백 TCP와 베어러 토큰을 통해 데몬과 통신하므로, 이를 로컬 유닉스 소켓 전용으로 제한하면 첫 실행 안내 전체가 동작할 수 없다. 또한 `admin` 스코프는 이미 Vault, 루틴, 시스템 설정 등 핵심 기능을 관리할 권한을 포함한다. 보안 위험을 막기 위해 스킬 내용은 데몬에 고정 번들된 파일만 쓰고 대상 디렉터리도 사전 설정이나 환경 변수에서만 가져오며, 등록된 에이전트 id만 허용한다. 가져오기 역시 자동 탐색된 프로필 id만 전달받고 키링 명령 수신은 원천 차단하며, 두 작업 모두 민감한 데이터 값 없이 감사 로그를 남긴다.
- **에이전트 엔드포인트 경로로 `/v1/external-agents` 사용(근거)**: 추후 openside 자체 에이전트 기능이 추가될 때 엔드포인트 이름이 충돌하지 않도록 외부 에이전트 경로를 분리했다.
- **온보딩 완료 상태를 데몬에서 보관(근거)**: 브라우저 로컬 스토리지 대신 데몬 파일시스템에 상태를 보관함으로써, 여러 기기에서 접속하더라도 데몬당 한 번만 초기 안내가 실행되도록 보장한다.
- **CLI 기능과의 1:1 대응 유지(근거)**: 첫 실행 안내의 모든 기능은 CLI 명령어(`openside skills install`, `openside import`, `openside pair`)로도 동일하게 수행할 수 있으며 웹 전용 기능을 따로 두지 않는다.

## 7. 테스트
- `web/src/onboarding/OnboardingPage.test.tsx`: 온보딩 각 단계별 UI 렌더링, 사용자 인터랙션, 상태 전이 검증(vitest).
- `web/src/onboarding/OnboardingGate.test.tsx`: 초기 접속 시 온보딩 진입 조건 판별, 스코프 검사, 리다이렉트 흐름 검증(vitest).
- `crates/opensided/src/tests/onboarding.rs`: 데몬 온보딩 상태 영속화(`onboarding.json`), 접근 스코프 제한 검증.
- `crates/opensided/src/tests/onboarding.rs` 는 위 상태 검증 외에 에이전트 감지, 스킬 설치(409 충돌과 `force`), 가져오기 API 의 스코프 검증과 id 기반 프로필 제약도 다룬다.
- `crates/openside-import/src/tests.rs`: 플랫폼별 브라우저 표준 프로필 경로 자동 탐색 로직 검증.
- `crates/openside-e2e/tests/onboarding.rs`: 임시 데몬과 모의 Chrome·Firefox 프로필 환경에서 헤드리스 Chromium을 이용해 전 단계를 완주하는 E2E 검증(390x844 모바일 및 1440x900 데스크톱 해상도, 라이트·다크 테마, 단계 건너뛰기 및 재진입 흐름 포함).
