# 첫 실행 안내 (Onboarding)

상태: **구현됨**. 관련: F-5. 코드: 웹 `web/src/onboarding/`, 데몬 `crates/opensided/src/onboarding.rs`·`crates/opensided/src/import_api.rs`, 프로토 `crates/openside-proto/src/onboarding.rs`, 탐색 `crates/openside-import/src/discover.rs`. 테스트: `web/src/onboarding/OnboardingPage.test.tsx`, `web/src/onboarding/OnboardingGate.test.tsx`, `crates/opensided/src/tests/onboarding.rs`, `crates/openside-import/src/tests.rs`, `crates/openside-e2e/tests/onboarding.rs`.

처음 방문한 사용자에게 환영 화면부터 다른 브라우저 데이터 가져오기, 암호화된 Vault로 비밀번호 가져오기, AI 구독 연결, 마무리 설정까지의 초기 설정 과정을 안내하는 웹 UI다. 모든 단계는 데몬의 공개 API 엔드포인트만 사용해 동작하며, 데몬이 안내 완료 여부를 기억한다.

## 1. 진행 절차
- 경로는 `/welcome`이며, 각 단계는 `?step=welcome|import|passwords|subscription|done` 쿼리로 관리한다(`step`이 없거나 알 수 없는 값이면 `welcome`을 표시한다). 따라서 뒤로 가거나 새로고침해도 현재 위치를 유지하며, `Back`을 누르면 URL을 통해 이전 단계로 돌아간다.
- 진행 순서는 Aside의 첫 실행 창을 단계별로 따른다: Welcome → Import from another browser → Passwords → AI subscription → All set. 안내 창 역시 Aside와 마찬가지로 화면 중앙에 902×562 크기로 배치한다(화면 너비가 1024px 미만일 때는 화면을 가득 채우며 분할 레이아웃의 일러스트는 숨긴다).
- 레이아웃은 두 가지다. Welcome과 Passwords 단계는 왼쪽에 문구와 하단 바를 두고 오른쪽에 일러스트를 배치하는 `split` 레이아웃을 쓰고, Import·AI subscription·All set 단계는 제목과 설명, 본문을 가운데로 정렬하는 `centered` 레이아웃을 쓴다.
- 하단 바는 왼쪽에 `‹ Back`, 가운데에 단계 표시 점(pager), 오른쪽에 선택적인 고스트 버튼 `Skip`과 엔터 기호가 붙은 검은색 주 버튼(예: `Import ↵`, `Connect ↵`, `Continue ↵`)을 배치한다.
- 단계 표시 점은 Import, Passwords, AI subscription 세 단계에만 점 3개로 표시하며, 현재 단계는 더 넓은 검은색 캡슐 형태로 나타낸다. Welcome과 All set은 단계 수에서 제외한다. Aside는 점 4개를 표시하지만 추가 단계가 확인되지 않아 openside에서는 실제 진행하는 3단계만 센다(의도된 차이).
- Welcome: openside 마크와 이름, "An AI assistant that works in your browser." 헤드라인을 보여주고, 하단에는 전체 너비의 `Get started`(다음 단계로 이동) 버튼과 테두리 형태의 전체 너비 `Skip setup`(안내를 종료하고 `skipped`로 기록) 버튼을 둔다. Aside는 이 자리에서 이메일·Google 로그인을 받지만, 계정이 없는 openside는 두 버튼으로 이를 대신한다(의도된 차이). 오른쪽에는 어시스턴트가 작동하는 미리보기 화면을 보여준다.
- Import from another browser: 이 컴퓨터에서 찾은 로컬 브라우저 프로필을 하나의 목록으로 보여주고(브랜드 타일, "Chrome · Personal", 항목 수, 체크박스 포함, 기본 전체 선택), 데이터를 실제로 쓰기 전에 종류별 미리보기와 `Choose what to bring` 펼침 메뉴를 제공한다. 비밀번호는 별도 단계에서 다루므로 여기서는 제외한다. 하단 바에는 `Skip`과 `Import ↵`를 두며, 가져오기가 끝나면 결과를 보여주고 하단 버튼을 `Continue ↵`로 바꾼다.
- Passwords ("Let openside fill in your passwords"): 가져오기 시트(5절 참고)를 여는 `Import credentials`("From a browser or another password manager") 한 줄만 표시한다. 외부 비밀번호 관리자를 실시간으로 연결하는 기능이 없는 openside에서는 Aside의 두 번째 행(실시간 비밀번호 관리자 연결)을 제공하지 않는다(의도된 차이). 하단 바에는 `Skip`과 `Import ↵`를 두며, 가져오기가 끝나면 "{count} passwords added to the vault." 상태 문구와 함께 하단 버튼을 `Continue ↵`로 바꾼다.
- AI subscription ("Choose your AI subscription"): Settings › Models와 동일한 구성을 사용해 ChatGPT 및 Claude 로그인 카드(ChatGPT 기본 선택)와 `Show more options`(기타 로그인 방식 및 "Use your own API key")를 제공한다. `Connect ↵`를 누르면 로그인을 진행하고 연결이 완료되면 All set 단계로 넘어간다. 고스트 버튼은 평소에는 `Skip`이며, 이미 연결된 모델이 있으면 `Continue`로 표시한다. 자체 기본 요금제가 없는 openside의 특성상 이 단계도 건너뛸 수 있도록 유지한다(Aside와의 의도된 차이).
- All set ("You’re all set!"): `Customize appearance`(Settings › Appearance)와 `Set up the openside CLI`(Settings › Developers) 두 가지 바로가기 행을 제공하며, 어느 쪽이든 누르면 안내를 완료 처리하고 해당 설정 페이지를 연다. 그 아래에는 기본으로 체크된 옵션으로 `Set openside as your default browser`를 두고, 브라우저가 앱 설치 프롬프트를 제공할 때만 `Add openside to your app launcher`를 함께 표시한다(설치 프롬프트는 `Get started`를 누를 때 열린다). 데이터를 전혀 수집하지 않는 openside에서는 Aside의 "Help improve" 옵션을 두지 않는다. `Get started ↵`를 누르면 안내를 마친다(`completed`).
- 기본 브라우저 설정: 브라우저 엔진에서 직접 운영체제의 기본 브라우저를 바꿀 수는 없으므로, 해당 옵션을 체크한 채 `Get started`를 누르면 먼저 "Make openside your default browser?" 대화상자(`Not now` / `Open settings`)를 띄운다. `Open settings`를 누르면 사용자의 브라우저를 통해 브라우저 자체 설정 페이지(`chrome://settings`)를 열어 직접 Make default를 선택할 수 있게 안내하며, 두 버튼 모두 누르면 온보딩 안내를 완료 처리한다. Aside는 여기서 운영체제 네이티브 프롬프트를 띄우지만, openside는 이를 본뜬 앱 내 대화상자를 표시한다(의도된 차이).
- 화면 어디에서도 사람에게 명령어를 직접 실행하라고 요구하지 않는다. 모든 절차는 GUI에서 완결되며, 터미널 대응 명령은 눈에 띄지 않는 "Or from a terminal" 안내 힌트(`web/src/pages/CliHint.tsx`)로만 표시될 수 있다.
- 안내에서 제외된 항목: 코딩 에이전트 감지 및 브라우저 스킬 설치, 휴대폰 페어링, 어시스턴트 이름 지정은 더 이상 온보딩 단계에 포함하지 않는다. 에이전트 설정과 휴대폰 페어링은 Settings › Developers에서 제공하며, 어시스턴트 이름은 데몬이 부여한 이름을 그대로 쓴다(`openside team rename`으로 변경 가능).

## 2. 열리는 조건과 상태
- 안내는 데몬의 온보딩 상태가 `pending`일 때 `/`에 처음 접근하면 자동으로 한 번 열린다. 단, 현재 기기의 토큰에 `admin` 스코프가 없는 것이 확실한 경우에는 자동으로 열리지 않는다(토큰을 직접 붙여넣는 등 스코프를 미리 알 수 없는 상태라면 그대로 진입해 데몬에 확인한다).
- 대화 상세 화면으로 연결되는 딥링크나 기타 경로로 접근할 때는 사용자의 작업을 가로막지 않으며 안내를 자동으로 띄우지 않는다.
- 언제든 Settings › General의 `Run setup again`을 눌러 다시 열 수 있다.
- 상태 조회 API: `GET /v1/onboarding` (`read` 스코프)은 `{state: pending|completed|skipped, updated_at}`을 반환한다.
- 상태 갱신 API: `PUT /v1/onboarding` (`admin` 스코프)은 `{state: completed|skipped}`를 받으며, `pending` 값은 400 `invalid_value`로 거절한다. 비교 후 설정(compare-and-set) 방식으로 동작해 저장된 상태가 `pending`일 때만 기록하며, 데몬이 읽기·검사·쓰기 과정을 직렬화한다. 안내가 이미 종료된 상태라면 아무것도 쓰지 않고 이미 기록된 결과를 담은 `{code, message, state, updated_at}`와 함께 409 `onboarding_already_set`을 반환하므로, 두 기기에서 동시에 안내를 마치더라도 정확히 하나의 결과만 유지된다.
- 상태는 `<data>/onboarding.json`에 원자적(atomic) 0600 파일 쓰기로 저장된다(`{state, updated_at}`). 파일이 없거나 손상되었으면 `pending`으로 간주한다(파일 손상 시 안내를 한 번 더 보여주는 것은 무해하나, 영원히 안내가 가려지는 오작동은 치명적이기 때문이다).
- Welcome 단계의 `Skip setup`이나 All set 단계의 `Get started`(또는 바로가기 행)는 먼저 `GET /v1/onboarding`으로 상태를 읽고, 조회에 성공해 `pending`임이 확인될 때만 상태를 기록한다. 따라서 Settings에서 안내를 다시 열었다가 나가더라도 기존의 `completed` 상태가 `skipped`로 바뀌지 않는다. 쓰기 요청에 409 `onboarding_already_set` 응답이 오면 다른 기기에서 이미 결과를 기록한 것이므로, 토스트 메시지를 띄우거나 로컬 닫기 플래그를 남기지 않고 안내가 끝난 것으로 처리한다.
- 상태 조회나 쓰기가 권한 부족(403)으로 거절되면 데몬에 기록하지 않고 현재 브라우저의 로컬 스토리지(`openside.onboarding.dismissed`)에만 닫힘 표시를 남겨, 방문할 때마다 안내가 다시 열리지 않게 한다. 그 밖의 조회나 쓰기 실패가 발생해도 마찬가지로 데몬에는 아무것도 쓰지 않고 로컬에서만 안내를 닫으며, 토스트 메시지를 띄운다.
- 모든 기기가 데몬의 상태를 확인하므로, 어느 한 기기에서 설정을 마치면 다른 기기에서도 안내가 다시 뜨지 않는다.

## 3. 에이전트 감지와 스킬 설치
외부 에이전트 API는 그대로 유지되지만, 이제 온보딩 안내가 아니라 Settings › Developers의 에이전트 목록에서 사용한다.
- `GET /v1/external-agents` (`admin` 스코프)는 기본 에이전트인 `claude`(Claude Code), `codex`(Codex), `omp`(omp)와 `config.toml`의 `[skills.agents]` 항목을 모두 나열한다.
- 각 항목 필드: `id`, `label`, `detected`, `skill_installed`, `skills_dir`.
- `detected`는 데몬의 `PATH`에 해당 에이전트 실행 파일이 있거나, 에이전트 스킬 디렉터리의 부모 디렉터리가 디스크에 존재할 때 true가 된다.
- `skills_dir`는 `openside skills install`과 동일한 탐색 규칙을 따른다(에이전트별 기본 디렉터리, `CLAUDE_CONFIG_DIR`·`CODEX_HOME`·`OMP_CODING_AGENT_DIR` 등의 환경 변수, `[skills.agents]` 설정 순으로 결정).
- `skill_installed`는 `<skills_dir>/openside-browser/SKILL.md` 파일이 실제로 존재할 때 true가 된다.
- `POST /v1/external-agents/{id}/browser-skill` (`admin` 스코프, 바디 `{force?: bool}`)은 내장 번들인 `openside-browser` 스킬을 설치하고 201 `{agent, skill, path, files}`를 반환한다. 로컬 권한(없으면 403 `local_access_required`)과 사용자 본인 권한(MCP 클라이언트와 에이전트 토큰은 403 `person_access_required`)도 필요하다. 이미 설치되어 있고 `force`가 false이면 기존 파일을 보존한 채 409 `exists`를 낸다. 목록에 없는 식별자는 404 `unknown_agent`, 디렉터리를 알 수 없으면 400 `skills_dir_unknown`으로 거절한다. 파일을 쓰기 직전에는 대상 폴더(`CLAUDE_CONFIG_DIR`, `CODEX_HOME`, `OMP_CODING_AGENT_DIR`로 지정된 경우를 포함한 기본 프리셋도 동일)가 홈 폴더 하위이고 홈 폴더 아래에 심볼릭 링크를 거치지 않으며 openside 데이터 폴더 바깥에 있는지 검사하고, 이를 벗어나면 아무것도 쓰지 않은 채 400 `invalid_skills_dir`를 반환한다.
- 감지되지 않은 에이전트는 `Not found` 문구와 함께 `Install anyway` 버튼을 제공해 수동 설치를 지원한다. 안내가 열려 있는 도중 사용자가 터미널에서 직접 스킬을 설치한 경우에도 오류 없이 설치 완료 상태로 갱신된다.
- 스킬 설치 내역은 `audit.jsonl`에 기록된다(`tool`: `external_agent`, `decision`: `skill_installed`, 기기 id 및 에이전트 id 포함).
- `PUT /v1/external-agents/{id}`는 커스텀 스킬 폴더를 추가하고, `DELETE`는 매핑을 해제하되 이미 설치된 파일은 그대로 둔다. 두 엔드포인트 모두 로컬 권한이 필요하며 `[skills.agents]`에 설정을 저장하고 즉시 반영된다. ID는 소문자, 숫자, 하이픈만 사용할 수 있고 기본 프리셋을 대체할 수 없다. 경로는 절대 경로이거나 `~/`로 시작해야 하며 부모 디렉터리 탐색(`..`)을 포함할 수 없고, HOME이나 XDG config/data 루트 내부로 확인되어야 한다. 설치 시 심볼릭 링크 이탈을 포함해 커스텀 대상을 다시 검증한다. 목록 조회 결과에는 `custom` 항목이 포함된다.

## 4. 브라우저 데이터 가져오기

- 브라우저 가져오기는 Welcome 다음으로 진행하는 첫 번째 실질적 단계다. 현재 컴퓨터에 설치된 Chromium 계열 브라우저, Firefox 파생 브라우저, macOS의 Safari에서 모든 프로필을 탐지하며, 프로필 이름, 이메일, 로컬 아바타 ID는 Local State나 profiles.ini에서 읽어온다. 온보딩 안내에서는 비밀번호가 별도 단계로 분리되어 있으므로 비밀번호를 제외한 모든 범주를 가져오며, Settings › General에서 실행할 때는 기존과 동일하게 비밀번호를 포함해 가져오고 인라인으로 Vault를 설정하거나 잠금 해제한다. CSV 비밀번호 가져오기는 브라우저 데이터 가져오기에 포함되지 않으며, 비밀번호 시트(5절)와 Settings › Passwords에서 제공한다.
- `GET /v1/import/profiles`는 안전한 프로필 메타데이터와 종류별 지원 여부를 반환한다. `POST /v1/import/preview`는 `{profiles?, what?}` 요청을 받아 실제 데이터를 쓰지 않고 프로필별 항목 수, 경고, Vault 필요 플래그, 복원 가능한 탭 목록을 반환한다.
- `POST /v1/import`는 동일한 선택 항목(생략 시 탐지된 모든 프로필의 전체 데이터를 의미함)을 전달받아 202 `ImportJob`을 반환한다. `GET /v1/import/jobs/{id}`로 진행 상황과 요약을 폴링할 수 있으며, `/events`는 지속성 있는 재생 가능한 `import_progress` SSE를 제공한다.
- 가져오기 관련 모든 라우트는 단순 관리자 토큰만이 아니라 로컬 사람 권한을 반드시 요구한다. 데스크톱 런처가 소켓 부트스트랩 시 생성한 부팅별 시크릿과 로컬 출처가 검증된 사람/관리자 토큰을 재사용한다. 원격/릴레이 호출에는 403 `local_access_required`를 반환하며, 원본 경로 지정이나 셸 명령어 전달은 일절 허용하지 않는다.
- 저장된 비밀번호를 가져오려면 기존 Vault를 먼저 설정하거나 잠금 해제해야 한다(409 `vault_setup_required` / `vault_locked`). `POST /v1/import/passwords` 경로의 GUI 비밀번호 CSV 가져오기는 Safari나 암호 앱의 내보내기 파일 및 보호된 원본 형식을 지원하며, CSV 파일 자체는 절대 저장하지 않는다.
- 쿠키는 에이전트의 기본 브라우저 프로필로 들어가고, 비밀번호는 암호화된 Vault로만 들어가며, 비밀이 아닌 데이터 범주는 원본 브라우저 프로필 범위의 스냅샷으로 저장된다. 다시 가져와도 멱등성이 유지된다. 탭은 브라우저를 자동 실행하지 않고 복원 링크 형태로 보존되며, 확장 프로그램도 자동 설치되지 않고 이름과 스토어 링크 목록으로 보존된다.
- 키링이 잠겨 있거나, Firefox 기본 비밀번호로 보호되어 있거나, 지원하지 않는 암호화 방식이거나, 읽을 수 없는 범주가 있으면 GUI 문구로 종류별 경고를 띄우고 다른 범주는 그대로 가져온다. Safari 접근 권한이 없으면 시스템 설정의 전체 디스크 접근 권한을 명시적으로 요청한다. UI에서 CLI 작업은 일절 요구하지 않는다.
- 클라이언트 연결이 끊겨도 이미 접수된 작업은 취소되지 않는다. 데몬당 한 번에 하나의 쓰기 작업만 실행하며, 진행 상황과 최종 요약은 비공개로 안전하게 유지된다. 파일 경로, 플랫폼별 제약 사항, 픽스처는 [import.md](import.md)를 참고한다.

## 5. Vault 및 비밀번호 가져오기
- 비밀번호 시트는 기존 가져오기 단계 안에 있던 인라인 CSV 가져오기와 인라인 Vault 게이트를 대체한다. 어둡고 흐리게 처리된 안내 창 위에 뜨는 모달(640×476, 모서리 반경 24) 형태이며, 왼쪽 상단에는 `openside Vault` 크럼을, 가운데에는 현재 경로(breadcrumb)를 표시한다. 가져오기가 진행 중이 아닐 때만 Escape 키, 바깥 영역 클릭, `Not now`로 닫을 수 있으며, 가져오는 중에는 작업이 완료되거나 실패할 때까지 시트가 열린 상태로 유지된다.
- 출처 선택(Pick): "Import credentials to openside Vault" 제목과 "Choose the password manager you are exporting from." 설명 아래 두 그룹을 표시한다. `Detected on this device`에는 이 컴퓨터에서 저장된 비밀번호가 확인된 브라우저 프로필(`GET /v1/import/profiles`의 `available.passwords`)을 나열하고, `Other password managers` 펼침 목록에는 Bitwarden, 1Password, LastPass, Apple Passwords, "Another app (CSV)"를 나열한다. 항목을 선택하기 전까지 `Next ↵` 버튼은 비활성화된다.
- 방식 선택(Method, 감지된 브라우저를 고른 경우에만 표시): "Import {Brand} passwords" 화면에서 두 가지 방식을 제시한다. `Import directly (Recommended)`는 해당 브라우저가 이 컴퓨터에 저장한 비밀번호를 직접 읽어오며(선택한 프로필에 대해 `what: ["passwords"]`로 `POST /v1/import`를 호출한 뒤 `GET /v1/import/jobs/{id}`를 폴링함, 컴퓨터에서 접근 권한을 요청할 수 있음), `Import .csv file`은 브라우저에서 내보낸 파일을 직접 선택해 가져온다(내보낸 파일은 암호화되어 있지 않으므로 가져온 뒤 삭제하도록 안내함).
- 파일 내보내기 및 선택(Export, CSV 경로): "Export from {Source}" 화면에 해당 앱의 내보내기 절차와 메뉴 일러스트를 함께 보여준다. `Choose file ↵`을 누르면 브라우저에서 파일을 읽어 `POST /v1/import/passwords`로 전송하며, CSV 파일 자체는 절대 저장하지 않는다.
- Vault: 가져오기 요청에 409 `vault_setup_required` 또는 `vault_locked` 응답이 오면 시트 안에서 바로 Vault 설정·잠금 해제 양식(`VaultGate`: OS 키링은 버튼 한 번, 암호 구문은 암호 입력 및 확인 절차)을 보여주고, 완료되면 대기 중이던 가져오기 작업을 자동으로 다시 시도한다. 별도의 Vault 설정 단계는 두지 않는다.
- 오류는 시트 안에서 처리한다. 원격 기기에는 로컬 전용 안내(403 `local_access_required`)를, 권한 스코프가 없는 기기에는 "Ask for access"(8절 참고)를, 그 외의 경우에는 오류 안내 문구를 시트 내부에 표시한다.
- 가져오기에 성공하면 시트가 닫히고, 단계 화면에 추가된 비밀번호 개수를 표시한다.
- 휴대폰 페어링(`ConnectPhone`)과 에이전트 설정은 더 이상 온보딩 안내에 포함되지 않으며, Settings › Developers에서 제공한다.

## 6. 권한과 보안 결정
- **브라우저 데이터 가져오기 권한**: 브라우저 데이터에는 인증 세션과 저장된 비밀번호가 포함된다. 가져오기 작업에는 기존 로컬 사람 권한 검증 체계(소켓 연결 또는 출처가 검증된 로컬 사람/관리자 토큰과 현재 실행 시크릿)가 필요하며, 원격 관리자 토큰 단독으로는 권한이 부족하다. 임의의 원본 경로/명령어나 비밀 정보가 담긴 출력은 일절 허용하지 않는다. 에이전트 스킬 설치는 별도로 검토된 관리자 스코프와 번들 콘텐츠 제한을 그대로 유지한다.
- **에이전트 엔드포인트 경로로 `/v1/external-agents` 사용(근거)**: 추후 openside 자체 에이전트 기능이 추가될 때 엔드포인트 이름이 충돌하지 않도록 외부 에이전트 경로를 분리했다.
- **온보딩 완료 상태를 데몬에서 보관(근거)**: 브라우저 로컬 스토리지 대신 데몬 파일시스템에 상태를 보관함으로써, 여러 기기에서 접속하더라도 데몬당 한 번만 초기 안내가 실행되도록 보장한다.
- **CLI 동등성 및 GUI 우선 원칙**: 온보딩 안내의 모든 작업은 CLI에서도 동일하게 지원하며(`openside import` 및 Vault·비밀번호 관련 CLI 명령), `openside skills install`과 `openside pair`는 온보딩 안내가 아니라 Settings › Developers의 CLI 대응 명령으로 유지된다. 웹 UI는 특정 작업의 수행 방법으로 명령어를 전면에 내세우지 않으며 보조 힌트로만 보여준다.

## 7. 테스트
- `web/src/onboarding/OnboardingPage.test.tsx`: 각 단계 렌더링, 단계 이동(`Back`, 알 수 없는 `step` 처리), 건너뛰기·완료 상태 기록, 브라우저 가져오기, 비밀번호 시트(직접 가져오기, CSV, 시트 내 Vault 설정·잠금 해제, 오류 처리), AI 구독 연결, All set 및 기본 브라우저 설정 안내 대화상자 검증(vitest).
- `web/src/onboarding/OnboardingGate.test.tsx`: 초기 접속 시 온보딩 진입 조건 판별, 스코프 검사, 리다이렉트 흐름 검증(vitest).
- `crates/opensided/src/tests/onboarding.rs`: 데몬 온보딩 상태 영속화(`onboarding.json`), 접근 스코프 제한, 비교 후 설정(compare-and-set) 검증. 동시에 들어온 두 PUT 요청 중 정확히 하나의 결과만 유지되고, 결과가 기록된 뒤의 PUT 요청은 409를 받으며 파일을 변경하지 않는지 확인한다.
- `crates/opensided/src/tests/onboarding.rs` 는 위 상태 검증 외에 에이전트 감지, 스킬 설치(409 충돌과 `force`), 가져오기 API 의 스코프 검증과 id 기반 프로필 제약도 다룬다.
- `crates/openside-import/src/tests.rs`: 플랫폼별 브라우저 표준 프로필 경로 자동 탐색 로직 검증.
- `crates/openside-e2e/tests/onboarding.rs`: 임시 데몬과 모의 Chrome·Firefox 프로필 환경에서 헤드리스 Chromium으로 Welcome → Import(모의 Chrome·Firefox 프로필 실제 가져오기) → Passwords(시트 열기·닫기 및 단계 건너뛰기) → AI subscription(건너뛰기) → All set → 기본 브라우저 설정 대화상자(`Not now`) → 홈 화면으로 이어지는 전체 흐름, Welcome에서 건너뛰기, Settings › General의 `Run setup again`을 통한 재진입을 검증하는 E2E 테스트(390x844 모바일 및 1440x900 데스크톱 해상도, 라이트·다크 테마 포함).

## 8. 권한 요청
- 페어링된 사람의 기기에서 특정 화면에 필요한 스코프가 없을 때(기존의 "관리자 스코프 필요" 막다른 화면 대신) **Ask for access**를 띄운다. 이 버튼을 누르면 `POST /v1/devices/me/access-requests {scopes}`를 호출하고(read 스코프, `person_device` 전용, 기기당 대기 중인 요청은 1개로 제한되며 메모리상에서 10분간 유효), 스코프가 부여될 때까지 `GET /v1/devices/me`를 폴링한다.
- openside가 실행 중인 컴퓨터의 웹 UI는 `GET /v1/access-requests`를 폴링하여(관리자 권한) **Allow**와 **Decline** 버튼이 있는 플로팅 카드를 표시한다. `POST /v1/access-requests/{id}/approve`는 로컬 권한(부팅별 로컬 시크릿, 페어링과 동일한 검증 조건)을 추가로 요구하므로 원격 관리자는 승인할 수 없으며, `.../deny`는 관리자 권한만 있으면 된다. 요청을 승인하면 기기에 해당 스코프가 병합되며, 요청·승인·거절 이력은 감사 로그로 남는다(`access_requested`, `access_granted`, `access_declined`).
- 에이전트 클라이언트는 접근 권한을 요청할 수 없다(403 `person_approval_required`).
- Tests: `crates/opensided/src/tests/access_requests.rs`, `web/src/pages/ScopeNotice.test.tsx`, `web/src/access/AccessRequestDock.test.tsx`.
