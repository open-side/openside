# Vault — 암호화 자격 증명, 패스키, 정책, 감사

상태: **구현됨**. 관련: C-1~C-6, D-6. 코드: `crates/openside-vault`(파일 형식·정책·감사·패스키·잠금 백엔드), `crates/opensided/src/tools/vault.rs`(`vault_login`·`vault_otp`·`vault_passkey`, 기동 시 잠금 해제), `crates/opensided/src/vault_api.rs`(`/v1/vault/*`), `crates/openside-proto/src/vault.rs`(API 타입), `crates/openside-cli/src/vault.rs`(`openside vault`), `crates/openside-config/src/lib.rs`(`[vault]`).

## 1. 파일 형식
- 헤더: `OSVAULT\0`(8바이트), 버전(u32, big-endian), salt 16바이트, XChaCha20-Poly1305 nonce 24바이트. 헤더 52바이트 전체가 AEAD 연관 데이터다. 본문은 항목 전체(메타데이터, 패스키, 항목별 정책)를 담은 JSON 을 암호화한 것이다.
- 버전 1(`unlock = "passphrase"`): 파일 키 = Argon2id(m=19456 KiB, t=2, p=1)(passphrase, salt). passphrase는 사용자가 `openside vault init`/`openside vault unlock` 시 터미널 프롬프트(입력 숨김)나 웹 Vault 페이지에서 입력하거나, 데몬 시작 시 `[vault].passphrase_cmd` 실행 결과로 전달한다.
- 버전 2(`unlock = "secret-service"` 또는 `"native"`, 키링 환경의 `"auto"`): 파일 키 = HKDF-SHA256(키링의 32바이트 래핑 키, salt, info `openside-vault v2 file key`). 래핑 키는 OS 키링에만 존재하며, 래핑 키와 파일 키 모두 디스크에 기록되지 않는다.
- 파일과 감사 로그는 0600 으로 만든다. 파일은 16 MiB 를 넘지 못하고 원자적으로 교체한다. 알 수 없는 버전과 인증 실패는 평문 진단 없이 거절한다.
- 항목은 `POST /v1/vault/items`(`openside vault add` 및 웹 Vault 페이지에서 호출), 라이브러리 API(`Vault::add_item`, `Vault::put`), 또는 기기 동기화로 추가한다. 요청 본문의 민감 정보는 데몬으로만 전달하며, 응답에는 메타데이터만 담는다. 데몬은 어떤 툴도 해당 값을 참조하기 전에 출력 redaction 대상에 새 항목 값을 등록한다. 동기화로 수신한 항목은 `PreparedImport::parse`(포맷 및 항목 ID 검증, 부작용 없음) → 마스킹 등록 → `Vault::commit_import`(감사 기록을 먼저 작성한 뒤 저장) 순서로 처리한다. 감사 기록 작성에 실패하면 아무것도 저장하지 않는다. 자세한 내용은 [`sync.md`](sync.md) §3을 참고한다.
- 버전 1과 버전 2 사이의 변환(마이그레이션)은 지원하지 않는다. 설정한 잠금 해제 방식이 파일 버전과 일치하지 않으면 볼트를 열지 않고 `format_mismatch` 사유를 반환한다.

## 2. 잠금 백엔드 (`PlatformLock`)
기본값인 `unlock = "auto"`는 OS에 맞춰 방식을 선택한다. 볼트 파일이 이미 있으면 파일 포맷을 따른다. 즉 버전 1은 passphrase를, 버전 2는 해당 OS의 키링을 쓴다. 파일이 아직 없다면 OS 키링에 접근할 수 있을 때(Linux: 세션 D-Bus에서 `org.freedesktop.secrets` 소유 프로세스가 존재하거나 D-Bus 활성화가 가능한 경우, macOS: Keychain, Windows: Credential Manager) 키링을 쓰고, 불가능하면 passphrase를 쓴다. 키는 평문으로 저장되지 않으며, 키링 기반 볼트가 예고 없이 passphrase 방식으로 대체되는 일은 없다.

| `[vault].unlock` | 플랫폼 | 래핑 키 위치 | 확인 |
|---|---|---|---|
| `auto` (기본값) | 전체 | 위 규칙에 따름 | 단위 테스트(`VaultUnlockSetting::resolve`) |
| `passphrase` | 전체 | 없음 (버전 1) | 단위 및 E2E 테스트 |
| `secret-service` | Linux | Secret Service 항목(속성 `application=openside`, `openside-vault=<경로 SHA-256 앞 16바이트 hex>`). 없으면 기본 컬렉션에 만든다 | 비공개 `dbus-daemon` 위 가짜 Secret Service (`crates/openside-vault/tests/secret_service.rs`) |
| `native` | macOS·Windows | Keychain·Credential Manager(`keyring` 크레이트, 서비스 `openside-vault`) | Windows(`x86_64-pc-windows-msvc`)는 `tools/check-platforms.py` 컴파일 검사 통과. macOS 는 SDK 가 없어 컴파일 검사를 하지 못했다. 둘 다 실행은 미검증(`unverified`) |

- 키링이 잠겨 있고 잠금 해제에 프롬프트 입력이 필요하면 볼트를 열지 않는다. `GET /v1/vault`는 `state=locked`와 `reason=keyring_locked`를 반환하고, `openside doctor`에도 동일하게 표시되며 볼트 툴은 노출되지 않는다. 컴퓨터에서 키링을 잠금 해제한 뒤 `openside vault unlock`을 실행하거나 웹 Vault 페이지에서 Unlock을 누른다.
- `while_unlocked` 판정은 호출마다 키링의 실제 잠금 상태를 확인하고(Secret Service 는 항목이 해제된 목록에 있는지, `native` 는 래핑 키를 읽을 수 있는지), 확인하지 못하면 잠긴 것으로 본다. 잠긴 것을 한 번 보면 Vault 를 다시 열 때까지 잠금 상태로 둔다.
- 확인하는 것은 openside 가 래핑 키와 파일 키를 디스크에 쓰지 않는다는 점이다. 실제 OS 키링이 자기 저장소를 어떻게 암호화하는지, 하드웨어로 보호되는지는 검증하지 않는다.

### 설정과 잠금 해제
- `[vault].path`를 지정하지 않으면 볼트 파일 경로는 `<data_dir>/vault.db`이다. 파일이 없으면 `uninitialized` 상태가 되며 볼트 툴은 노출되지 않는다.
- 키링 백엔드: `openside vault init`, 빈 본문(`{}`)의 `POST /v1/vault/setup`, 또는 첫 `openside vault add`(`POST /v1/vault/items`) 실행 시 버전 2 파일이 생성된다. 별도 입력은 요구하지 않는다. 래핑 키는 키링의 기본 컬렉션에 생성된다.
- Passphrase 백엔드: `openside vault init` 실행 시 터미널 프롬프트에서 passphrase(8자 이상)를 숨김 상태로 두 번 입력받으며, 웹 Vault 페이지에서도 동일하게 처리한다. 분실한 passphrase는 복구할 수 없다. `passphrase_cmd`를 설정하지 않으면 데몬을 시작할 때마다 볼트가 잠긴다. 이때는 `openside vault unlock`이나 웹 Vault 페이지(모바일 접속 가능)에서 잠금을 해제한다.
- `openside vault lock`(`POST /v1/vault/lock`)을 호출하면 데몬 메모리에서 복호화된 볼트를 파기하며, 다음 잠금 해제 전까지 볼트 툴이 비활성화된다.
- `GET /v1/vault` 상태값: `uninitialized`, `locked`, `unlocked`, `unavailable`. `reason`은 `keyring_unreachable`, `keyring_locked`, `keyring_key_mismatch`, `passphrase_cmd_failed`, `unsupported_platform`, `format_mismatch`, `unreadable`, `probe_timeout` 중 하나이다.
- `openside doctor`는 현재 사용하는 잠금 해제 방식과 다음 단계 안내(`openside vault init`, `openside vault unlock`, `openside vault add`)를 표시한다. 파일 헤더 확인과 키링 접근 가능 여부만 검사하며, 볼트를 직접 잠금 해제하거나 `passphrase_cmd`를 실행하지는 않는다.

## 3. 접근 정책
- 값: `always`(파일이 복호화돼 있으면 내줌), `while_unlocked`(백엔드 잠금도 풀려 있어야 함), `never`(내주지 않음).
- 전역 기본값은 `[vault].default_policy`(기본 `while_unlocked`)이고 기동 때 적용한다. 항목별 재정의는 암호화된 파일 안에 저장하며 전역 기본값보다 우선한다.
- 판정 순서: 항목 존재 → 정책 → 현재 탭 origin 이 항목의 허용 origin 과 정확히 일치. origin 비교는 scheme·host·port 를 정규화해서 한다. IP 리터럴과 유사 도메인은 거절한다. 패스키는 RP ID 가 origin 의 host 와 같거나 점이 들어 있는 상위 도메인일 때만 쓴다.
- 거절하면 툴은 `denied` 와 함께 `vault_<이유>` 를 돌려준다. 이유는 `locked`, `unknown_site`, `policy_never`, `platform_locked`, `origin_mismatch`, `rp_mismatch`, `no_passkey` 중 하나다. 이때 페이지에는 아무 값도 들어가지 않는다.
- 데몬 정책(`openside-policy`)의 `VaultUse` 판정은 이와 별개로 먼저 적용된다. `read-only` 는 Deny, `guard`·`full` 은 Allow 다. `submit_ref` 가 결제·삭제 키워드에 맞으면 `Sensitive`(Ask)가 추가된다.

### TOTP 매개변수

TOTP 팩터는 RFC 6238에 따라 base32 시드와 함께 `digits`(6–8), `period`(1–86400초), `algorithm`(`SHA1`/`SHA256`/`SHA512`) 정보를 저장한다. `Totp::parse`는 `otpauth://totp/...?secret=&digits=&period=&algorithm=` 형식의 URI(issuer와 label은 무시하며 `hotp`, secret 누락, 중복되거나 잘못된 매개변수는 거부) 또는 6자리 / 30초 / SHA-1 기본값을 사용하는 순수 base32 시드를 받는다.

기본값과 다른 매개변수만 `totp_params`에 기록하고, 기본값은 별도로 기록하지 않아 이전 빌드에서도 설정 파일을 문제없이 읽을 수 있도록 유지한다.

**마이그레이션 안내**: 본 변경 사항 이전에 저장된 항목에는 `totp_params`가 없으며, 이제 30초 주기와 SHA-1 알고리즘 기반의 6자리 코드를 생성한다. 이전 빌드는 항상 8자리 코드를 생성했으나, 일반적인 인증기 앱 규격을 따르는 실제 웹사이트에서는 이를 지원하지 않았다.

`request_otp`와 `register_authenticator`는 볼트 API인 `register_totp`와 `totp_site_for`를 사용한다. `totp_site_for`는 어떤 값도 노출하지 않고 특정 origin에 등록된 TOTP 항목이 없는지, 하나인지, 또는 여러 개인지만 보고한다. `register_totp`는 오직 생성 작업만 수행한다. 사용자 이름과 비밀번호가 비어 있고 기본 정책이 적용된, 해당 origin에 정확히 바인딩된 새로운 TOTP 전용 항목 `otp-<host>[-<port>]`를 생성한다. origin을 허용하는 기존 항목에 이미 TOTP가 등록되어 있으면 `already_registered`로 거부되고, 정책상 금지되어 있으면 `never`로 거부된다. 감사 액션은 `otp_register`이며 결과는 `allowed`, `denied`, `failed`, `registered`로 기록된다. 새 항목이 아직 생성되지 않은 상태이므로 결정 기록에는 대상 사이트가 `unknown-site`로 표기된다. 항목은 저장되었으나 `registered` 기록을 남기지 못한 경우, 변경 사항이 실제로 반영되었음을 호출 측에 보고한다.

## 4. 감사
- 위치: `<data_dir>/vault-audit.jsonl`(append 전용, 0600). 같은 로컬 계정이 고치는 것까지 막지는 않는다.
- `vault_login`(`fill`), `vault_otp`(`otp`), `vault_passkey`(`passkey_register`·`passkey_login`), 항목 목록(`get`), 정책 변경(`policy_update`)을 모두 기록한다. 판정 기록(`allowed`·`denied`)은 콜백 실행 전에 남기고, 끝나면 결과 기록(`filled`·`fill-failed`·`completed`·`failed`·`updated`)을 하나 더 남긴다. 툴이 Vault 콜백에 들어가기 전에 실패한 호출(origin 재확인 실패, 브라우저 오류, 잘못된 인자)과 취소된 호출은 결과 기록이 없다.
- 정책 변경과 목록 조회는 잠긴 Vault 나 없는 항목에 대한 요청도 `denied`(`locked`·`unknown_item`)로 남긴다. 정책 변경은 판정 기록을 먼저 쓰고, 쓸 수 없으면 정책은 바뀌지 않는다. 변경이 저장된 뒤 결과 기록만 실패하면 `500 policy_applied_audit_incomplete` 를 돌려주며, 이때 변경은 이미 적용된 상태다.
- 항목 추가와 삭제는 `item_add`, `item_delete`로 기록한다. 판정 기록(`allowed`, 또는 `locked`·`item_exists`·`unknown_item`에 따른 `denied`)을 먼저 남기며, 기록 작성에 실패하면 변경 사항을 반영하지 않는다. 이후 `added`, `deleted`, `failed` 결과 기록이 이어진다. 항목 삭제 시 `deleted` 기록의 대상은 `unknown-site`로 표기되므로, 앞서 남긴 `allowed` 기록에 해당 항목 ID를 포함한다.
- 수명주기 단계도 함께 기록한다: `setup`(`completed`, 또는 오류 코드가 포함된 `failed`), `unlock`(`unlocked`; `wrong_passphrase`가 포함된 `denied`; 오류 코드가 포함된 `failed`), `lock`(`locked`). 이 기록의 항목은 `*`, origin은 `-`로 표기하며 값은 일절 포함하지 않는다. 감사 기록을 남길 수 없는 잠금 해제 요청은 아예 수행하지 않는다(`500 audit_unavailable`). 잠금은 언제나 실행하며, 기록 작성에 실패하면 `500 vault_locked_audit_incomplete`로 응답한다. 설정(setup) 도중 기록 작성에 실패하더라도 볼트는 설정 및 잠금 해제 상태를 유지하며 `500 vault_set_up_audit_incomplete`로 응답한다. 횟수 제한(throttle)에 걸린 잠금 해제 시도는 기록하지 않으므로 무차별 대입 공격으로 로그가 넘쳐나지 않는다.
- 필드 목록: 시간(초), 호출자(도구 호출 및 시작 시 데몬 자체의 잠금 해제는 `opensided`, API 호출 및 인증기 등록은 호출하거나 승인한 기기를 가리키며 페어링된 기기는 `device:<id>`, 로컬 소켓은 `local`), 작업(action), 항목 ID, 정규화된 origin, 결과, 세션 ID, tool_call_id, 세부사항(거절 사유 또는 새 정책). 값 자체는 절대 기록하지 않으며, 각 기록은 디스크에 쓰기 직전에 현재 볼트에 보관된 값들과 한 번 더 대조하여 마스킹한다.

## 5. API·CLI (admin)
| 경로 | 설명 |
|---|---|
| `GET /v1/vault` | `{state, backend, reason, default_policy}` 반환. 민감 정보 제외 |
| `POST /v1/vault/setup` | 볼트를 생성한다. 암호 기반 볼트: `{"passphrase":"…"}` (8자 이상). 키링 기반 볼트: `{}`. 오류: `409 vault_already_set_up`, `400 passphrase_required`·`weak_passphrase`·`passphrase_not_used`, `503 keyring_unavailable`·`vault_unsupported`, `500 vault_set_up_audit_incomplete`(생성 및 잠금 해제는 완료되었으나 감사 기록 누락). |
| `POST /v1/vault/unlock` | setup과 요청 본문 동일. 오류: `409 vault_not_set_up`, `400 wrong_passphrase`·`passphrase_required`, `503 keyring_unavailable`, `500 audit_unavailable`(잠금 상태 유지), 초 단위 `Retry-After` 헤더를 포함한 `429 too_many_attempts`. 각 호출자(페어링된 기기 또는 로컬 소켓)는 암호를 5회까지 틀릴 수 있으며, 초과 시 30초 동안 대기해야 한다. 이후 틀릴 때마다 대기 시간이 2배씩 늘어나 최대 15분까지 증가하며, 올바른 암호를 입력하면 대기 상태가 초기화된다. 페어링된 모든 기기에는 15분당 총 20회의 실패 제한이 공유 적용된다. 로컬 소켓은 이 공유 제한에서 제외되므로 컴퓨터 앞에 있는 사용자는 언제든 잠금을 해제할 수 있다. 호출자당 최대 1개, 페어링된 기기 전체를 통틀어 최대 2개의 시도만 동시에 실행할 수 있다. 횟수 제한에 걸린 시도는 암호 검증 전에 거부하며 감사 기록도 남기지 않는다. |
| `POST /v1/vault/lock` | 메모리에서 복호화된 볼트를 제거하고 현재 상태를 반환한다. 잠금 해제 중에 확인된 값을 가리는 출력 마스킹 필터는 데몬을 다시 시작할 때까지 유지되는데, 잠금 해제 상태에서 읽은 페이지나 로그에 해당 값이 여전히 남아 출력될 수 있기 때문이다. 동일한 볼트를 다시 해제하더라도 필터가 중복 추가되지는 않는다. 오류: `500 vault_locked_audit_incomplete`(잠금은 완료되었으나 감사 기록 작성 실패). |
| `POST /v1/vault/items` | `{id, origins[], username?, password?, totp?, policy?}`. `totp`는 `otpauth://` URI 또는 base32 설정 키 형식이다. `password`나 `totp` 중 적어도 하나는 지정해야 한다. 항목 메타데이터만 담아 `201`을 반환한다. 항목을 저장하기 전에 모든 출력에서 새 값을 마스킹한다. 첫 번째 항목을 추가하면 키링 볼트가 자동으로 생성된다. 암호 기반 볼트는 미리 생성하거나 잠금을 해제해 두어야 한다(`409 vault_unavailable`). 오류: `400 invalid_origin`·`invalid_totp`·`empty_item`·`invalid_item`, `409 item_exists`, `500 item_added_audit_incomplete`(항목은 정상 저장됨). |
| `DELETE /v1/vault/items/{id}` | `204` 반환. 오류: `404 unknown_item`, `409 vault_unavailable`, `500 item_deleted_audit_incomplete` (항목은 삭제됨) |
| `GET /v1/vault/items` | 항목 ID, 허용 origin, 재정의 정책, 적용 정책, TOTP 유무, 패스키 RP ID. 값은 없다. Vault 가 없으면 `available=false` |
| `PATCH /v1/vault/items/{id}` | `{"policy":"always"\|"while_unlocked"\|"never"\|"default"}`. `default` 는 재정의를 지운다. 없는 항목 `404 unknown_item`, Vault 없음 `409 vault_unavailable`, 감사 판정 기록 실패 `500`(정책은 그대로), 저장 뒤 결과 기록 실패 `500 policy_applied_audit_incomplete`(정책은 적용됨) |
| `GET /v1/vault/audit?since=&item=&session=&limit=` | 오래된 순. `since` 는 RFC 3339, `limit` 기본 200·최대 1000(가장 최근 N개) |

민감 정보를 전달하는 요청 본문(`setup`, `unlock`, `items` POST)에 알 수 없는 필드가 있거나 잘못된 JSON 형식이 포함되면 `400 invalid_value`로 거부하며, 오류 응답에 입력값을 그대로 노출하지 않는다. 모든 볼트 라우트는 `admin` 스코프가 필요하다. 모바일 기기에서 볼트를 관리하려면 `openside pair --scope admin`으로 페어링해야 한다. 해당 스코프가 없으면 웹 Vault 페이지에 필요한 조치 방법이 표시된다.

CLI: `openside vault status|init|unlock|lock`, `openside vault add <id> --origin URL [--origin URL…] [--login] [--totp] [--policy always|while_unlocked|never|default]`, `openside vault rm <id>`, `openside vault items`, `openside vault policy <id> <always|while_unlocked|never|default>`, `openside vault audit [--since T] [--item ID] [--session S] [--limit N]`. `add`는 username(표시됨), password 및 인증자 키(숨김) 입력을 순서대로 요구하며, 표준 입력 파이프 사용 시 각 프롬프트에 대응하는 값을 한 줄씩 읽는다. 민감 정보는 argv로 전달되지 않는다. `--json` 플래그 사용 시 API 응답을 그대로 출력한다.

## 6. 패스키 (`vault_passkey`)
- 인자: `tab_id`, `site`, `action`(`register`|`login`), `submit_ref`(페이지의 만들기·로그인 버튼).
- 데몬이 전용 탭에 CDP 가상 인증기를 붙이고 `submit_ref` 와 같은 버튼을 누른다(아래). 도구 호출 시작 때 탭 origin 을 다시 확인한다.
  - `register`: 인증기에 생긴 자격 증명(ES256 개인 키, credential ID, user handle, 서명 카운터)을 읽어 항목에 암호화해 저장한다. 다른 RP ID 의 자격 증명이 섞여 있으면 `rp_mismatch` 로 거절한다.
  - `login`: origin 에 맞는 패스키만 인증기에 넣고, 서명 카운터가 오르면 성공으로 보고 새 카운터를 저장한다.
- 의식은 에이전트의 탭이 아니라 그 세션이 소유하는 전용 새 탭에서 한다(같은 브라우저 프로필이므로 쿠키·로그인 상태를 함께 쓴다). 에이전트의 탭에는 인증기를 붙이지도, 손대지도 않는다. 순서는 다음과 같다.
  1. 이 코드가 띄운 브라우저인지 확인한다(아래 "다시 붙은 브라우저"). 아니면 거절한다.
  2. 빈 탭(`about:blank`)을 만든다.
  3. 그 탭에 가드(`Browser::guard_documents`)를 **첫 탐색보다 먼저** 세운다. 가드는 최상위뿐 아니라 하위 프레임의 문서 요청도 승인된 origin 이 아니면 요청 전에 막는다(리다이렉트 단계마다).
  4. 에이전트 탭이 보여 주는 URL 을 이 탭에서 불러온다. 이 탭의 모든 문서는 가드를 거쳤다.
  5. `Page.getFrameTree` 로 모든 프레임의 origin 을 확인한다. 최상위가 승인된 origin 과 다르거나 다른 origin 의 프레임이 하나라도 있으면(교차 사이트 서드파티 프레임 포함) 인증기를 붙이기 전에 거절한다. 불투명 origin(샌드박스·`data:`) 프레임은 WebAuthn 이 거절하므로 허용한다.
  6. 인증기 설치(ctap2, internal, resident key, UV, 자동 사용자 확인) → 프레임 확인 → `submit_ref` 와 같은 역할·이름의 버튼을 이 탭의 스냅샷에서 찾아 누르기(정확히 하나여야 하고, 에이전트 탭에서도 그 역할·이름이 하나여야 한다) → 폴링마다 프레임 확인.
  7. 인증기를 지우고 전용 탭을 닫는다.
  - `register`: 인증기에 생긴 자격 증명(ES256 개인 키, credential ID, user handle, 서명 카운터)을 읽어 항목에 암호화해 저장한다. 다른 RP ID 의 자격 증명이 섞여 있으면 `rp_mismatch` 로 거절한다.
  - `login`: origin 에 맞는 패스키만 인증기에 넣고, 서명 카운터가 오르면 성공으로 보고 새 카운터를 저장한다.
- 전용 탭을 쓰는 이유: 같은 탭에서는 가드가 서기 전에 이미 허용된 탐색의 응답이 인증기 설치 뒤에 올라오거나, 렌더러에 이미 넘어간 커밋이 있을 수 있어서 시간을 두는 방식으로는 막을 수 없었다. 전용 탭은 가드가 선 뒤에 처음 탐색하므로 이전에 진행 중이던 탐색·기록 복원·프리렌더가 없고, 에이전트 탭에서 늦게 올라온 문서는 인증기가 없는 다른 타깃에 있다. 실제 Chromium 에서 에이전트 탭의 같은 RP ID 문서가 응답을 인증기 설치 뒤에야 받아 올라왔을 때(그 문서가 어서션을 요청하는 것도 확인했다) 어서션을 받지 못하는 것을 확인했다.
- 비용: 전용 탭은 RP 페이지를 새로 불러온다. 에이전트 탭의 화면 상태(SPA 의 메모리 상태, 입력 중인 폼, POST 로 도착한 결과 페이지처럼 URL 만으로 다시 만들 수 없는 것)는 전용 탭으로 넘어가지 않으므로, 그런 페이지에서는 버튼을 찾지 못해 의식이 실패할 수 있다. 의식으로 로그인되었더라도 에이전트 탭은 그대로이니 다시 불러와 새 스냅샷을 받아야 한다. 의식이 끝나기 전까지 전용 탭이 백그라운드 탭 하나로 보인다.
- 네트워크를 거치지 않는 경로와 프리렌더: 전용 탭에는 되돌아갈 기록이 없고(`history.back()` 은 빈 페이지로 간다), 프리렌더는 데몬이 브라우저를 띄울 때 프로필 설정(`net.network_prediction_options` = 2, "페이지 미리 로드" 끔)을 써 넣어 막는다. `--disable-features=Prerender2` 는 이 Chromium 에서 효과가 없어 쓰지 않는다. 브라우저 전체의 bfcache 를 끄는 방법은 `browser_repl` 의 같은 탭 탐색(`enter-submits`)을 깨뜨려 쓰지 않는다.
- 다시 붙은 브라우저: 띄울 때 `--openside-isolation=1` 표식(Chromium 은 모르는 스위치를 무시한다)을 명령줄에 넣고, 다시 붙을 때는 실행 중인 프로세스가 보고하는 명령줄(`SystemInfo.getInfo` 의 `commandLine`)에서 표식을 확인한다(`Browser::documents_isolated`). 표식이 없는 브라우저(예전 빌드가 남긴 분리된 브라우저)에서는 프리렌더를 막을 수 없으므로 패스키 의식을 거절한다. 실행 중인 브라우저의 설정은 바깥에서 고칠 수 없으므로 그 브라우저를 닫고 새로 띄워야 한다.
- 인증기의 수명은 호출자가 취소할 수 없는 별도 태스크가 맡는다. 전체 제한은 15초이고 정리 단계마다 5초다. 호출이 취소되거나, 시간이 지나거나, 명령이 실패해도 인증기를 지우고 전용 탭을 닫는다. 정리는 단계적이다: ① 인증기 제거 확인 → ② 전용 탭 닫기(`Target.closeTarget` 성공 확인; 인증기 제거를 확인했어도 시도한다) → ③ 제거도 탭 닫기도 확인하지 못하면 데몬이 띄운 브라우저 프로세스를 죽이고(`Browser::terminate`, 그룹에 SIGKILL, 안 되면 pid 에) 프로세스가 사라진 것을 확인한다(자식이면 거둔 종료 상태, 다시 붙은 브라우저면 pid 가 시스템에서 사라짐). 신호 전송 오류나 "그런 프로세스 없음" 응답, 이미 끊긴 CDP 연결만으로는 확인으로 치지 않는다. 브라우저가 죽으면 모든 세션의 탭이 사라지고 컨트롤러가 다음 사용 때 다시 띄운다. 어느 것도 확인하지 못하면 등록으로 얻은 키가 있어도 성공으로 보고하지 않고 실패로 돌려주며, 가드는 풀지 않고 2초마다 탭 닫기·브라우저 종료를 다시 시도하다가 확인되면 푼다. 모델이 받는 결과는 `{"status":"registered"|"signed_in"|"failed"}` 뿐이다.
- WebAuthn CDP 명령은 `Browser::webauthn_command` 로만 보낸다. `browser_repl` 의 허용 명령 목록에는 없다.
- 개인 키는 데몬의 CDP 소켓으로만 오가지만, 인증기가 있는 동안 Chromium 도 키를 가진다. 데몬이 읽은 자격 증명 목록의 문자열은 쓰고 나서 지우지만, 전송 계층 버퍼와 Chromium 쪽 복사본은 데몬이 지울 수 없다.
- 등록 직후 새 개인 키를 레지스트리 redactor 에 넣는다. 모델 출력·이벤트·티켓·감사의 모든 경로에서 가려진다.

## 7. 값이 나가지 않는다는 보장 (D-6)
- 값은 Rust 호스트 콜백 안에서만 브라우저로 간다. 툴 결과는 상태 문자열뿐이다.
- 값을 채운 문서에서는 `browser_screenshot` 과 `browser_repl` 을 거부하고, 데몬 재시작 뒤에는 `browser_snapshot` 도 거부한다(`docs/09-tool-design.md`).
- 패치된 브라우저에서는 게이트웨이가 연결되어 있을 때 `vault_login`과 `vault_otp`가 CDP 대신 네이티브 게이트웨이(`autofill.fields`/`autofill.fill`)를 통해 입력값을 채운다. 데몬에서 인증된 게이트웨이를 거쳐 브라우저로 값이 전달되고 브라우저가 이를 자체적으로 채우므로, 에이전트의 CDP 명령, 툴 실행 결과, 이벤트, 감사 로그에는 비밀값이 전혀 남지 않는다. 게이트웨이가 연결된 상태에서 네이티브 자동 완성이 실패하면 `fill_failed`를 반환하며 CDP로 되돌아가지 않는다. 순정 Chromium은 앞에서 설명한 기존 CDP 방식을 그대로 유지한다. 값을 채운 뒤 페이지 자체나 권한을 가진 인스펙터가 해당 필드를 읽는 것은 여전히 가능하며, 이를 막아준다고 보장하지는 않는다. 자세한 사양: `browser/specs/gateway-protocol.md` §11, 테스트: `crates/opensided/src/tests/vault.rs::patched_vault_login_uses_native_autofill_without_cdp_secret_values`.
- `opensided` 테스트 `full_access_has_no_tool_or_api_path_returning_vault_values` 가 `full` 모드에서 노출된 모든 툴을 값이 채워진 탭에서 호출하고, 스트리밍이 아닌 모든 GET 경로를 호출해 결과·이벤트·데이터 디렉터리에 값이 없는지 확인한다. 페이지·파일을 읽을 수 있는 툴은 빈 인자가 아니라 유효한 인자로 부른다: `read`(vault 파일·감사 파일·`/proc/self/environ`), `bash`(vault·감사 파일 `cat`, `env`, 환경 변수, 데이터 디렉터리 전체 `cat`), `browser_repl` 세 가지 읽기, `browser_act`, `browser_snapshot`, `browser_screenshot`, `memory_search`(쓰기 뒤에도 다시), `memory_write`, `delegate`, `skill`. `delegate` 는 데몬 테스트의 가짜 런타임이 서브에이전트를 실제로 돌리지 않으므로 인자 검증과 승인 경로만 확인하고, 실제 서브에이전트의 읽기는 위 e2e 테스트가 확인한다.

## 8. 테스트
- `crates/openside-vault/src/tests.rs`: 형식·정책·감사·패스키·TOTP(RFC 6238 벡터)·키링 vault 가 키를 디스크에 쓰지 않음.
- `crates/openside-vault/tests/secret_service.rs`: 가짜 Secret Service 로 래핑 키 생성·재사용, 그리고 Vault 가 열린 채로 키링이 잠기면 `while_unlocked` 항목이 거절되는지(`always` 항목은 계속 쓸 수 있음) 확인한다.
- `crates/opensided/src/tests/vault.rs`: 정책 거절, 감사(세션·tool_call_id), API, 리다이렉트 피싱 거절, D-6(모든 툴과 모든 GET 경로. 스트리밍 경로는 일부만 읽어 검사한다).
- `crates/openside-cli/src/tests/vault.rs`: CLI 요청·출력.
- `crates/opensided/src/tests/vault.rs::a_paired_admin_device_manages_the_vault_lifecycle_and_items_without_values_coming_back`: `admin` 권한을 가진 페어링 기기 토큰으로 TCP 통신하여 암호 문구 볼트를 구성하고(누락되거나 짧은 암호 문구 및 알 수 없는 필드는 거부되며 오류에 입력값이 노출되지 않음), 로그인 및 인증기를 추가하고, 중복 ID·잘못된 출처(origin)·잘못된 키를 거부하며, 잠금 및 잠금 해제(잘못된 암호 문구 포함)를 수행하고, 항목 삭제 후 `item_add`/`item_delete` 감사 기록을 확인한다. `read` 전용 토큰은 `403`을 받으며, 어떤 응답·감사 로그·볼트 파일에도 저장된 값이나 암호 문구가 포함되지 않는지 확인한다.
- `crates/opensided/src/tests/vault.rs::wrong_passphrases_are_throttled_per_device_and_across_devices_but_never_lock_out_local`: 기기별로 암호를 5회 틀리면 `Retry-After`가 포함된 429 응답을 받고 대기 시간이 2배씩 늘어나며, 올바른 암호를 입력하면 초기화된다. 호출자당 1개, 기기 전체에서 최대 2개의 시도만 동시 실행되고, 기기 전체 공유 제한인 20회 실패가 적용되지만 로컬 소켓은 언제나 잠금 해제가 가능하며, 횟수 제한에 걸린 시도는 기록하지 않는다.
- `crates/opensided/src/tests/vault.rs::an_item_saved_without_its_completion_record_is_already_masked`: 완료 기록 작성에 실패(`500 item_added_audit_incomplete`)하더라도 항목은 정상 저장되며 해당 값은 이미 마스킹되어 있다.
- `crates/opensided/src/tests/otp.rs::an_approval_whose_commit_failed_is_reported_as_registered_after_a_restart`: 승인으로 인증 수단을 저장한 뒤 티켓 커밋에 실패하고 데몬이 재시작되더라도, 명시적 거절 시 티켓이 `registered` 상태로 종료된다.
- `crates/opensided/src/tests/otp.rs::recovery_follows_the_intent_operation_through_every_terminal_path`: 같은 origin에 다른 주체가 등록한 수단이 있어도 거절 응답은 `declined`로 유지된다. 만료 시에는 티켓 고유의 작업 ID로 저장된 등록 정보를 보고하며, 볼트가 잠긴 상태에서 세션이 중지되면 `registration_indeterminate`로 종료된다.
- `crates/opensided/src/tests/policy.rs::an_ask_browser_call_waiting_for_the_ticket_gate_does_not_hold_the_browser_gate`: Ask 정책이 적용된 브라우저 호출은 티켓 게이트를 기다리기 전에 브라우저 게이트를 먼저 해제한다.
- `crates/opensided/src/tests/policy.rs::unchecked_ticket_targets_are_redacted_like_any_caller_text`: 브라우저가 실행 중이지 않을 때 볼트 값을 담고 있는 비보안 티켓의 `tab_id`, `origin`, `field_ref`는 티켓과 이벤트 로그에서 모두 마스킹된다.
- `crates/openside-cli/src/tests/vault.rs::vault_init_add_and_rm_take_values_from_stdin_and_never_print_them`: `init` 시 두 암호 문구가 다르면 요청을 보내지 않고, `add` 시 사용자 이름, 비밀번호, 인증기 키를 표준 입력(stdin)에서 한 줄씩 읽어 요청 본문으로만 전송하며, 어떤 명령도 값을 화면에 출력하지 않는지 확인한다.
- `crates/openside-e2e/tests/web_admin.rs::paired_phone_adds_an_authenticator_to_the_vault`: 실제 데몬이 빌드된 웹 앱을 `admin` 권한으로 페어링된 390x844 모바일 화면(헤드리스 크로미엄)에 제공한다. 모바일 화면에서 암호 문구 볼트를 설정하고 설정 키로 인증기를 추가하며, 폼이 가로 스크롤 없이 화면에 맞고 페이지에 키가 노출되지 않으며 항목에 일회용 인증 코드가 정상 생성되는지 확인한다.
- `crates/openside-config/src/tests.rs::vault_unlock_auto_keeps_the_file_method_and_never_falls_back_silently`: `auto` 모드가 기존 파일의 방식을 유지하고, 새 볼트를 만들 때는 OS 키링에 접근할 수 있을 때만 키링 방식을, 그렇지 않으면 암호 문구 방식을 선택하며, 키링을 쓸 수 없을 때 대체 방식으로 조용히 넘어가지 않고 키링 파일을 거부하는지 확인한다.
- `crates/openside-e2e/tests/vault.rs`: loopback 모의 RP 가 attestation `none` 과 ES256 서명을 직접 검증하는 등록·로그인, 유사 origin·리다이렉트 거절, 개인 키 canary.
- `crates/openside-e2e/tests/vault.rs::password_and_otp_fill_reach_no_surface_and_every_value_read_is_blocked`: 실제 Chromium 과 실제 Python 사이드카(스크립트 모델)로 `vault_login`·`vault_otp` 를 채운다. 페이지가 보고하는 값 길이와 `type=password` 로 채워졌음을 확인한 뒤, 채운 탭에서 `browser_snapshot`·`browser_screenshot`·`browser_repl`(값 읽기·뒤집기·`page.content()`·`evaluate`)·`browser_act`·`bash`·`read`·`memory_search` 를 호출하고, 같은 읽기를 시도하는 서브에이전트(`delegate`)도 돌린다. 스크린샷과 REPL 은 거절되어야 하고, 사이드카가 저장한 모델 입력(스냅샷), 이벤트 로그, 데이터 디렉터리, 데몬·사이드카 stderr 어디에도 비밀번호·사용자 이름·TOTP 시드(원문, 뒤집은 것, base64)가 없어야 한다. 일회용 코드는 값이 짧고 이벤트 ID 와 구분되지 않아 검사하지 않는다. 쉘 샌드박스가 vault 와 데이터 디렉터리를 아예 숨기기 때문에 `bash` 의 파일 읽기는 이 테스트에서 의미가 없고, 샌드박스 없는 경우는 D-6 테스트가 맡는다.
- `crates/opensided/src/tools/vault/ceremony.rs`(단위): 취소·시간 초과·명령 실패·제거 실패·origin 이탈에서도 인증기를 지우는지, 다른 origin 의 프레임이 있으면 설치 전에 거절하고 도중에 생기면 중단하는지, 제거·탭 닫기·브라우저 종료가 모두 실패하거나 멈추면 성공을 보고하지 않고 가드를 유지하며 복구되면 푸는지를 가짜 브라우저로 확인한다.
- `crates/opensided/src/tests/vault.rs::passkey_authenticator_is_bound_to_the_origin_and_removed_when_the_call_is_dropped`: 실제 Chromium 에서, 클릭이 같은 RP ID 의 다른 origin 으로 이동하려 하면 요청 전에 막히고 어서션이 나오지 않는다. 의식이 끝나면 전용 탭이 닫히고, 에이전트 탭에서는 인증기가 쓰이지 않는다. 호출이 취소되면 인증기가 사라지고 전용 탭이 닫힌다.
- `crates/opensided/src/tests/vault.rs::no_other_document_can_use_the_authenticator_through_history_or_frames`: 실제 Chromium 에서 (a) 같은 RP ID 의 다른 origin 이 에이전트 탭의 기록에 있는 경우(전용 탭에는 기록이 없어 `history.back()` 은 페이지를 떠나게 하고 의식은 거절된다), (b) 승인된 페이지가 의식 중에 `allow="publickey-credentials-get *"` iframe 을 넣는 경우(가드가 요청을 막는다), (c) 마크업에 다른 origin iframe 이 있는 페이지(전용 탭에서는 그 iframe 이 요청조차 되지 않는다), (d) 에이전트 탭이 같은 RP ID 의 다른 origin 으로 간 탐색이 도구의 사전 점검 뒤에 시작되고(테스트가 vault 잠금을 쥐고 있는 동안 시작해 시점을 고정한다) 서버가 응답을 인증기 설치 뒤, 전용 탭의 버튼 클릭 때에야 보내는 경우(그 문서는 에이전트 탭에 실제로 올라와 어서션을 요청하지만 받지 못하고, 의식은 성공한다), (e) 에이전트 탭이 `speculationrules` 로 다른 origin 문서를 프리렌더하고 전용 탭이 그 주소로 이동하려는 경우(프리렌더 요청이 없고 이동은 가드가 막는다)에서 경쟁 origin 의 수집 기록에 `:ok` 가 없다. 프리렌더 끄기 설정을 빼면 (e) 가 프리렌더 요청을 잡아 이 테스트가 실패한다(빼 보고 확인했다). (d) 는 인증기를 에이전트 탭에 붙이던 이전 구조의 같은 시나리오에서, 탐색 취소를 빼면 의식이 실패하는 것으로 확인했다.
- `crates/openside-browser/tests/chromium.rs::a_reattached_browser_reports_whether_it_was_started_with_the_document_isolation`: 이 코드가 띄운 분리 브라우저는 다시 붙어도 `documents_isolated` 가 참이고, 표식 없이 띄운 Chromium 은 거짓이며, 그 브라우저도 `terminate` 로 끝나는 것이 프로세스로 확인된다.
- `crates/openside-browser/src/browser.rs`(단위): `preloading_is_disabled_in_the_profile_without_losing_other_preferences`(프리렌더 설정을 쓰되 다른 설정은 보존), `termination_is_confirmed_by_the_process_and_not_by_signals`(신호 전송 실패는 오류, "그런 프로세스 없음"이나 성공 응답이 와도 프로세스가 살아 있으면 확인하지 않고, 실제로 죽이면 거둔 종료 상태로 확인한다).
- `crates/opensided/src/tools/vault/ceremony.rs`(단위)에는 위 외에 전용 탭의 순서(열기 → 가드 → 불러오기 → 프레임 확인 → 인증기 설치, 인증기 제거 → 탭 닫기)와, 열기·가드·불러오기가 실패하거나 불러온 뒤 다른 origin 이면 설치 없이 거절하고 열린 탭을 닫는지, 격리 표식이 없는 브라우저에서 탭도 만들기 전에 거절하는지를 확인하는 테스트가 있다.
- `crates/opensided/src/tests/vault.rs::the_last_resort_of_a_failed_teardown_kills_the_browser_and_the_controller_relaunches_it`: `Browser::terminate` 가 실제 Chromium 을 죽이고 프로세스가 사라진 것을 확인하며, 컨트롤러가 다음 호출에서 새 브라우저를 띄운다.
