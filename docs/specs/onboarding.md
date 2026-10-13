# 첫 실행 안내 (Onboarding)

상태: **구현됨**. 관련: F-5. 코드: 웹 `web/src/onboarding/`, 데몬 `crates/opensided/src/onboarding.rs`·`crates/opensided/src/import_api.rs`, 프로토 `crates/openside-proto/src/onboarding.rs`, 탐색 `crates/openside-import/src/discover.rs`. 테스트: `web/src/onboarding/OnboardingPage.test.tsx`, `web/src/onboarding/OnboardingGate.test.tsx`, `crates/opensided/src/tests/onboarding.rs`, `crates/openside-import/src/tests.rs`, `crates/openside-e2e/tests/onboarding.rs`.

처음 방문한 사용자에게 에이전트 연결, 브라우저 데이터 가져오기, Vault 설정, 휴대폰 페어링까지의 초기 설정을 안내하는 웹 UI다. 모든 단계는 데몬의 공개 API 엔드포인트만 사용해 동작하며, 데몬이 안내 완료 여부를 기억한다.

## 1. 진행 절차
- 경로는 `/welcome`이며, 각 단계는 `?step=import|models|agents|pair|done` 쿼리로 관리하므로 뒤로 가거나 새로고침해도 현재 위치를 유지한다. 별도의 시작(환영) 화면이나 독립된 Vault 설정 단계는 두지 않는다.
- 진행 순서는 브라우저(이 컴퓨터에 이미 설치된 브라우저 데이터 가져오기) → Models → Agents → Phone → Done 순이다. 로그인되어 있는 웹사이트를 확보해야 에이전트를 곧바로 유용하게 쓸 수 있으므로 가져오기를 가장 먼저 배치한다.
- 첫 단계의 하단 버튼은 보조 형태의 `Skip for now`이며, 가져오기가 끝나면 `Continue`로 바뀐다. 이후 모든 단계는 `Skip this step`(실제 설정을 진행한 뒤에는 `Continue`)을 제공하고, `Back`을 누르면 이전 단계로 돌아가며, 상단 헤더의 `Skip setup`을 누르면 온보딩 안내 전체를 종료한다.
- 화면 어디에서도 사람에게 명령어를 직접 실행하라고 요구하지 않는다. 모든 절차는 GUI에서 완결되며, 터미널 대응 명령은 눈에 띄지 않는 "Or from a terminal" 안내 힌트(`web/src/pages/CliHint.tsx`)로만 표시될 수 있다.
- Vault 설정이나 잠금 해제는 비밀번호를 가져올 때 가져오기 단계 안에서 인라인(`VaultGate`)으로 처리하며, 비밀번호 없이 가져올 수도 있다.
- Done 단계에서는 Browser 데이터, Models, Agents, Phone 설정 결과를 요약해서 보여주며, 건너뛴 항목에는 `Skipped`가 표시된다.

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
- `PUT /v1/external-agents/{id}`는 커스텀 스킬 폴더를 추가하고, `DELETE`는 매핑을 해제하되 이미 설치된 파일은 그대로 둔다. 두 엔드포인트 모두 로컬 권한이 필요하며 `[skills.agents]`에 설정을 저장하고 즉시 반영된다. ID는 소문자, 숫자, 하이픈만 사용할 수 있고 기본 프리셋을 대체할 수 없다. 경로는 절대 경로이거나 `~/`로 시작해야 하며 부모 디렉터리 탐색(`..`)을 포함할 수 없고, HOME이나 XDG config/data 루트 내부로 확인되어야 한다. 설치 시 심볼릭 링크 이탈을 포함해 커스텀 대상을 다시 검증한다. 목록 조회 결과에는 `custom` 항목이 포함된다.

## 4. 브라우저 데이터 가져오기

- 가져오기는 온보딩에서 사람이 직접 진행하는 첫 번째 실질적 단계다. 현재 컴퓨터에 설치된 Chromium 계열 브라우저, Firefox 파생 브라우저, macOS의 Safari에서 모든 프로필을 탐지하며, 기본적으로 9개 범주를 모두 미리보기에 표시한다. 프로필 이름, 이메일, 로컬 아바타 ID는 Local State나 profiles.ini에서 읽어온다.
- `GET /v1/import/profiles`는 안전한 프로필 메타데이터와 종류별 지원 여부를 반환한다. `POST /v1/import/preview`는 `{profiles?, what?}` 요청을 받아 실제 데이터를 쓰지 않고 프로필별 항목 수, 경고, Vault 필요 플래그, 복원 가능한 탭 목록을 반환한다.
- `POST /v1/import`는 동일한 선택 항목(생략 시 탐지된 모든 프로필의 전체 데이터를 의미함)을 전달받아 202 `ImportJob`을 반환한다. `GET /v1/import/jobs/{id}`로 진행 상황과 요약을 폴링할 수 있으며, `/events`는 지속성 있는 재생 가능한 `import_progress` SSE를 제공한다.
- 가져오기 관련 모든 라우트는 단순 관리자 토큰만이 아니라 로컬 사람 권한을 반드시 요구한다. 데스크톱 런처가 소켓 부트스트랩 시 생성한 부팅별 시크릿과 로컬 출처가 검증된 사람/관리자 토큰을 재사용한다. 원격/릴레이 호출에는 403 `local_access_required`를 반환하며, 원본 경로 지정이나 셸 명령어 전달은 일절 허용하지 않는다.
- 저장된 비밀번호를 가져오려면 기존 Vault를 먼저 설정하거나 잠금 해제해야 한다(409 `vault_setup_required` / `vault_locked`). `POST /v1/import/passwords` 경로의 GUI 비밀번호 CSV 가져오기는 Safari나 암호 앱의 내보내기 파일 및 보호된 원본 형식을 지원하며, CSV 파일 자체는 절대 저장하지 않는다.
- 쿠키는 에이전트의 기본 브라우저 프로필로 들어가고, 비밀번호는 암호화된 Vault로만 들어가며, 비밀이 아닌 데이터 범주는 원본 브라우저 프로필 범위의 스냅샷으로 저장된다. 다시 가져와도 멱등성이 유지된다. 탭은 브라우저를 자동 실행하지 않고 복원 링크 형태로 보존되며, 확장 프로그램도 자동 설치되지 않고 이름과 스토어 링크 목록으로 보존된다.
- 키링이 잠겨 있거나, Firefox 기본 비밀번호로 보호되어 있거나, 지원하지 않는 암호화 방식이거나, 읽을 수 없는 범주가 있으면 GUI 문구로 종류별 경고를 띄우고 다른 범주는 그대로 가져온다. Safari 접근 권한이 없으면 시스템 설정의 전체 디스크 접근 권한을 명시적으로 요청한다. UI에서 CLI 작업은 일절 요구하지 않는다.
- 클라이언트 연결이 끊겨도 이미 접수된 작업은 취소되지 않는다. 데몬당 한 번에 하나의 쓰기 작업만 실행하며, 진행 상황과 최종 요약은 비공개로 안전하게 유지된다. 파일 경로, 플랫폼별 제약 사항, 픽스처는 [import.md](import.md)를 참고한다.

## 5. Vault 및 휴대전화
- **Vault**: 데몬의 백엔드를 사용해 가져오기 단계 내에서 인라인으로 설정하거나 잠금을 해제한다(OS 키링은 버튼 한 번, 암호 구문은 암호 입력과 확인 절차). Vault를 사용할 수 없으면 Vault 화면으로 연결되는 링크를 제공하고 비밀번호는 가져오기 대상에서 제외한다. 별도의 vault 설정 단계는 두지 않는다.
- **Phone**: Remote access의 QR 흐름(`ConnectPhone`)을 재사용하며, `GET /v1/devices`를 통해 새 기기 연결을 감지한다. 데스크톱 컴퓨터 외의 브라우저에서 접속한 경우에는 페어링을 컴퓨터에서 진행해야 한다는 안내만 표시하며 명령어를 노출하지 않는다. 관리자 스코프가 없는 기기에는 "Ask for access"를 표시한다(8절 참고).

## 6. 권한과 보안 결정
- **브라우저 데이터 가져오기 권한**: 브라우저 데이터에는 인증 세션과 저장된 비밀번호가 포함된다. 가져오기 작업에는 기존 로컬 사람 권한 검증 체계(소켓 연결 또는 출처가 검증된 로컬 사람/관리자 토큰과 현재 실행 시크릿)가 필요하며, 원격 관리자 토큰 단독으로는 권한이 부족하다. 임의의 원본 경로/명령어나 비밀 정보가 담긴 출력은 일절 허용하지 않는다. 에이전트 스킬 설치는 별도로 검토된 관리자 스코프와 번들 콘텐츠 제한을 그대로 유지한다.
- **에이전트 엔드포인트 경로로 `/v1/external-agents` 사용(근거)**: 추후 openside 자체 에이전트 기능이 추가될 때 엔드포인트 이름이 충돌하지 않도록 외부 에이전트 경로를 분리했다.
- **온보딩 완료 상태를 데몬에서 보관(근거)**: 브라우저 로컬 스토리지 대신 데몬 파일시스템에 상태를 보관함으로써, 여러 기기에서 접속하더라도 데몬당 한 번만 초기 안내가 실행되도록 보장한다.
- **CLI 동등성 및 GUI 우선 원칙**: 온보딩 가이드의 모든 작업은 CLI에서도 동일하게 지원하지만(`openside skills install`, `openside import`, `openside pair`), 웹 UI는 특정 작업의 수행 방법으로 명령어를 전면에 내세우지 않으며 보조 힌트로만 보여준다.

## 7. 테스트
- `web/src/onboarding/OnboardingPage.test.tsx`: 온보딩 각 단계별 UI 렌더링, 사용자 인터랙션, 상태 전이 검증(vitest).
- `web/src/onboarding/OnboardingGate.test.tsx`: 초기 접속 시 온보딩 진입 조건 판별, 스코프 검사, 리다이렉트 흐름 검증(vitest).
- `crates/opensided/src/tests/onboarding.rs`: 데몬 온보딩 상태 영속화(`onboarding.json`), 접근 스코프 제한 검증.
- `crates/opensided/src/tests/onboarding.rs` 는 위 상태 검증 외에 에이전트 감지, 스킬 설치(409 충돌과 `force`), 가져오기 API 의 스코프 검증과 id 기반 프로필 제약도 다룬다.
- `crates/openside-import/src/tests.rs`: 플랫폼별 브라우저 표준 프로필 경로 자동 탐색 로직 검증.
- `crates/openside-e2e/tests/onboarding.rs`: 임시 데몬과 모의 Chrome·Firefox 프로필 환경에서 헤드리스 Chromium을 이용해 전 단계를 완주하는 E2E 검증(390x844 모바일 및 1440x900 데스크톱 해상도, 라이트·다크 테마, 단계 건너뛰기 및 재진입 흐름 포함).

## 8. 권한 요청
- 페어링된 사람의 기기에서 특정 화면에 필요한 스코프가 없을 때(기존의 "관리자 스코프 필요" 막다른 화면 대신) **Ask for access**를 띄운다. 이 버튼을 누르면 `POST /v1/devices/me/access-requests {scopes}`를 호출하고(read 스코프, `person_device` 전용, 기기당 대기 중인 요청은 1개로 제한되며 메모리상에서 10분간 유효), 스코프가 부여될 때까지 `GET /v1/devices/me`를 폴링한다.
- openside가 실행 중인 컴퓨터의 웹 UI는 `GET /v1/access-requests`를 폴링하여(관리자 권한) **Allow**와 **Decline** 버튼이 있는 플로팅 카드를 표시한다. `POST /v1/access-requests/{id}/approve`는 로컬 권한(부팅별 로컬 시크릿, 페어링과 동일한 검증 조건)을 추가로 요구하므로 원격 관리자는 승인할 수 없으며, `.../deny`는 관리자 권한만 있으면 된다. 요청을 승인하면 기기에 해당 스코프가 병합되며, 요청·승인·거절 이력은 감사 로그로 남는다(`access_requested`, `access_granted`, `access_declined`).
- 에이전트 클라이언트는 접근 권한을 요청할 수 없다(403 `person_approval_required`).
- Tests: `crates/opensided/src/tests/access_requests.rs`, `web/src/pages/ScopeNotice.test.tsx`, `web/src/access/AccessRequestDock.test.tsx`.
