# 동기화 — 설정·핀 탭·Vault 항목·메모리의 종단간 암호화 동기화 (B-9)

상태: **구현됨**. 코드: `crates/openside-sync`(키 유도·봉인·병합·백엔드 trait·WebDAV/디렉터리 백엔드·테스트용 가짜 WebDAV 서버), `crates/openside-vault/src/sync.rs`(항목 내보내기·가져오기·삭제), `crates/openside-memory/src/sync.rs`(태그로 고른 메모리 목록과 동기화 id), `crates/opensided/src/sync.rs`(`/v1/pinned-tabs`, `/v1/sync/*`), `crates/openside-proto/src/sync.rs`(API 타입), `crates/openside-cli/src/sync.rs`(`openside sync`), `crates/openside-config/src/sync.rs`(`[sync]`).

## 1. 범위
- openside 가 운영하는 서버는 없다. 기기마다 레코드를 암호화한 뒤, 사용자가 고른 저장소(WebDAV 공유 또는 다른 수단으로 동기화되는 디렉터리)에 올린다. 저장소는 `Backend` trait 하나로 추상화되어 있다.
- 동기화 대상(레코드 종류):

| 종류 | id | 내용 | 끄기 |
|---|---|---|---|
| `setting` | `policy` | 라이브 정책 문서(`GET/PUT /v1/policy` 와 같은 JSON) | 기본으로 꺼져 있다. `sync.settings = true` 일 때만 동기화한다 |
| `pinned_tab` | 정규화한 URL 의 SHA-256 앞 8바이트(hex) | `PinnedTab` JSON | — |
| `vault_item` | Vault 항목 id | `Vault::export_item` 이 만든 항목 JSON(아이디·비밀번호·TOTP 시드·패스키·허용 origin·항목별 정책) | — |
| `memory` | 처음 만들어진 기기에서의 메모리 id | 종류·내용·태그·보존 기간·URL·제목 | `sync.memory_tags = []` |

- 메모리는 `sync.memory_tags`(기본 `["sync"]`) 중 하나가 붙은 것만 동기화한다. 모든 기기의 `memory_tags` 가 같아야 한다. 받은 메모리에 이 기기의 태그가 하나도 없으면 적용하지 않고 `rejected` 로 센다(적용하면 다음 스냅샷에서 빠져 삭제로 전파되기 때문이다).
- 태그를 떼면 그 메모리는 이 기기의 로컬 데이터가 되고, 다른 기기에서는 삭제된다. 이후 다른 기기에서 오는 삭제나 수정은 이 메모리를 건드리지 않는다. 삭제는 받아들인 것으로 처리하고, 수정에는 더 새로운 삭제 표시로 답한다. 둘 다 `conflicts` 로 센다.
- 다른 기기에서 받은 메모리는 새 로컬 id 로 만들어진다. 동기화 id 와의 대응은 메모리 DB 의 `sync_ids` 표(동기화 id 기본 키, 로컬 id 유일)에 메모리를 만드는 것과 같은 트랜잭션으로 저장한다. 그래서 중간에 멈춰도 대응 없는 사본이 남지 않고, 같은 레코드를 다시 받으면 기존 메모리를 쓴다.
- 핀 탭은 데몬이 `<data_dir>/pinned-tabs.json` 에 보관하는 목록이다(`GET/POST /v1/pinned-tabs`, `DELETE /v1/pinned-tabs/{id}`). URL 은 http(s)만 받는다. 두 기기에서 같은 URL 을 핀하면 레코드 하나가 되고, 다시 핀하면 제목만 바뀐다. 이미 없는 탭을 해제해도 성공(204)이다.
- 브라우저 프로필·기록·대화·스킬·`config.toml` 은 동기화하지 않는다.

## 2. 키와 페어링
- 계정 id 는 무작위 16바이트다. 동기화 키는 `sync.passphrase_cmd` 출력(끝의 개행 제거)에서 Argon2id(m=19456 KiB, t=2, p=1)로 유도한다. salt 는 SHA-256(`openside-sync v1 salt\0` ‖ 계정 id)의 앞 16바이트다.
- 이 마스터 키에서 HMAC-SHA256 으로 용도별 하위 키를 만든다. `seal`(레코드 암호화), `ids`(레코드 키·다이제스트), `check`(앞 4바이트, 패스프레이즈 확인용).
- 키는 데몬 메모리에만 있고 디스크에는 쓰지 않는다. 데몬을 재시작하면 처음 `sync now` 때 `passphrase_cmd` 를 다시 실행한다.
- `openside sync setup` 은 계정을 새로 만들고 페어링 코드 `osync1-<base32(계정 id ‖ check)>` 를 출력한다. 다른 기기는 같은 패스프레이즈를 `passphrase_cmd` 로 설정한 뒤 `openside sync pair <코드>` 를 실행한다. 유도한 키의 check 가 코드와 다르면 `400 sync_wrong_passphrase` 로 거절하고, 아무것도 저장하거나 병합하지 않는다.
- 페어링 코드에는 계정 id 와 32비트 check 만 들어 있다. 코드만으로는 아무것도 복호화할 수 없고, 패스프레이즈가 함께 있어야 한다.

## 3. 레코드 형식
- 레코드 키(저장소가 보는 이름): HMAC(`ids`, `key\0` ‖ 종류 ‖ `\0` ‖ id)의 앞 16바이트(hex 32자). 저장소는 종류·Vault 항목 이름·URL·메모리 id 를 알 수 없다.
- blob: `0x01`(형식 버전) ‖ nonce 24바이트(봉인할 때마다 새 난수) ‖ XChaCha20-Poly1305 암호문과 태그. 연관 데이터(AD)는 `openside-sync v1\0` ‖ 형식 버전 ‖ 계정 id ‖ 레코드 키다. 그래서 blob 을 다른 레코드나 다른 계정으로 옮기면 인증에 실패한다.
- 평문 payload(JSON): `v`, `kind`, `id`, `stamp`(`lamport`, `deleted`, `device`), `body`(종류별 JSON 의 base64. 삭제 표시(tombstone)에는 없음). 복호화한 뒤 `kind`·`id` 로 다시 계산한 레코드 키가 blob 의 키와 같은지 확인하고, 다르면 거절한다.
- 받은 본문은 종류별로 다시 검증한 뒤 적용한다. 검증에 실패하면 `rejected` 로 세고, 로컬 데이터는 바뀌지 않는다.
  - 정책: `PUT /v1/policy` 와 같은 검증·저장·즉시 반영 경로를 탄다. 바뀌면 데몬 stderr 에 `live policy replaced by sync from another device` 를 남긴다.
  - Vault 항목: `PreparedImport::parse` 가 부수 효과 없이 형식(origin·TOTP 시드·패스키)과 항목 id 를 검증한다. 본문의 항목 id 가 레코드의 id 와 다르면 거절하며, 다른 항목은 건드리지 않는다. 그다음 새 비밀 값을 데몬의 출력 마스킹(redaction) 목록에 먼저 추가한다. 마지막으로 `Vault::commit_import` 가 감사 기록을 먼저 남기고 암호화된 Vault 파일에 저장한다. 감사 기록을 쓰지 못하면 아무것도 저장하지 않는다.
  - 메모리: 메모리 저장소의 비밀 값 검사를 거친다.
- 평문 본문 버퍼(복호화 결과, base64 본문, Vault 내보내기)와 키, WebDAV 비밀번호는 `Zeroizing` 으로 들고 있다가 놓을 때 지운다. 단, HTTP 클라이언트나 JSON 직렬화가 내부에서 만드는 사본까지 지운다고 보장하지는 않는다.
- 내보내기·가져오기·삭제는 Vault 감사 로그에 `get` 으로 남는다(detail `sync_export`·`sync_import`·`sync_delete`). 동기화할 때마다 모든 항목을 내보내 다이제스트를 계산하므로, 실행 한 번에 항목 수만큼 `sync_export` 줄이 생긴다.

## 4. 백엔드 (`Backend` trait)
- 연산은 세 가지다. `list`(레코드 키와 entity tag 목록), `get`(blob 과 entity tag), `put(key, if_match, blob)`. `put` 은 조건부 쓰기다. 기기가 마지막으로 본 entity tag 가 아직 그대로일 때만 쓴다. 처음 쓰는 레코드는 "아직 없음"이 조건이다. 조건이 깨지면 `Conflict` 를 돌려준다.
- WebDAV(`sync.webdav_url`): 계정마다 컬렉션 `<webdav_url>/<계정 id>/` 를 쓰고, 레코드는 `<레코드 키>.blob` 리소스다. `MKCOL`(setup·pair 때 컬렉션 생성, 이미 있으면 405 도 성공), `PROPFIND`(Depth 1, `getetag`), `GET`, `PUT` + `If-Match: "<etag>"` 또는 `If-None-Match: *` 를 쓴다. 412 는 충돌로 처리한다. PROPFIND 응답에서는 status 가 200 인 `propstat` 안의 `getetag` 만 쓴다(네임스페이스 접두어·속성은 상관없다). 약한 ETag(`W/"…"`)는 강한 비교에 쓸 수 없으므로 `sync_backend_unavailable` 로 거절한다. PUT 응답에 ETag 가 없으면 다음 PROPFIND 의 값을 쓴다. `webdav_username` 이 있으면 `webdav_password_cmd` 출력과 함께 HTTP Basic 으로 보낸다. 401·403 은 `sync_unauthorized` 가 된다. 리다이렉트는 따라가지 않는다.
  - 검증 범위: 테스트는 같은 의미(MKCOL·PROPFIND·조건부 PUT·강한 ETag·Basic 인증)를 구현한 가짜 서버(`openside_sync::fake_webdav`, 127.0.0.1)로만 했다. Nextcloud 같은 실제 WebDAV 서버와의 호환성은 확인하지 않았다(`unverified`).
- 디렉터리(`sync.dir`): `<dir>/<계정 id>/<레코드 키>.blob`(디렉터리 0700). entity tag 는 내용의 SHA-256 앞 16바이트다. 조건부 쓰기는 최선 노력이다. 확인과 교체 사이에 잠금이 없어서, 다른 프로세스나 다른 기기(동기화 도구 경유)가 그 사이에 덮어쓸 수 있다. 덮어써진 버전도 사라지지는 않는다. 그 버전을 가진 기기가 다음 실행에서 자기 stamp 가 더 높다는 것을 보고 다시 올린다.
- 두 백엔드 모두 저장소에는 레코드 키·blob·entity tag 만 남는다.

## 5. 동기화 한 번의 절차 (`POST /v1/sync/run`, `openside sync now`)
0. 실행 중에는 동기화하는 데이터의 로컬 쓰기를 모두 막는다. 정책 `PUT`, 메모리를 바꾸는 모든 작업(API 생성·수정·삭제, 만료 메모리를 먼저 지우는 이력 조회, 에이전트 도구, 방문 기록, 주기적인 만료 정리), Vault, 핀 탭이 대상이다. 그래서 스냅샷과 적용 사이에 로컬 데이터가 바뀌지 않는다. 그동안 들어온 쓰기는 실행이 끝날 때까지 기다렸다가 처리되고, 다음 실행에서 로컬 변경으로 올라간다. 메모리 쓰기의 잠금은 실제로 쓰는 작업 스레드가 들고 있다. 요청이 취소되어도 쓰기가 끝날 때까지 동기화는 시작하지 않는다. 실행도 별도 작업으로 돌기 때문에 클라이언트가 연결을 끊어도 중간에 멈추지 않는다.
1. 로컬 변경 감지: 지금 쓸 수 있는 종류의 현재 내용을, 마지막으로 동기화한 키 기반 다이제스트와 비교한다. Vault 는 잠금이 풀려 있을 때만, 설정은 `sync.settings` 일 때만, 메모리는 `memory_tags` 가 있을 때만 쓸 수 있다. 바뀐 레코드는 새 stamp(Lamport 증가)를 받고, 사라진 레코드는 삭제 표시를 받는다. 이 상태는 `<data_dir>/sync/state.json`(0600)에 저장하며, 레코드 내용과 키는 들어가지 않는다.
   - 미리 쓰기(write-ahead): 새 stamp 를 매긴 직후, 원격 counter 를 반영한 직후(적용 전), 매 push 직전에 상태를 디스크에 저장한다. 그래서 blob 을 올린 뒤 상태를 저장하기 전에 죽어도, 재시작한 기기가 이미 공개한 stamp 를 다른 내용에 다시 붙이지 않는다.
2. pull: `list` 로 받은 entity tag 가 마지막으로 본 것과 다른 레코드만 `get` 하고, 인증·복호화한 뒤 아래 규칙으로 비교한다. 원격 쪽이 이기면 로컬에 적용한다.
3. push: 로컬 쪽이 이긴 레코드를 봉인해 조건부 `put` 으로 올린다. 다른 기기가 먼저 썼으면 2번부터 다시 한다. 8회를 넘으면 `409 sync_busy` 다.
4. 결과는 개수만 돌려준다: `pushed`, `pulled`, `applied`, `conflicts`, `rejected`, `deferred`, `vault_available`.

## 6. 충돌 해결 규칙
레코드마다 last-writer-wins 레지스터 하나다. 두 버전은 stamp 로 비교하고, 순서는 다음과 같다.
1. `lamport` 가 큰 쪽이 이긴다. 기기는 새 stamp 를 그때까지 본 모든 stamp 보다 크게 매긴다. 따라서 다른 기기의 변경을 받은 뒤에 한 수정은 그 변경을 이긴다.
2. `lamport` 가 같으면(동시 수정) 삭제 표시가 수정을 이긴다. 한 기기에서 지운 레코드는 다른 기기의 동시 수정 때문에 되살아나지 않는다.
3. 그래도 같으면 `device` id 를 사전순으로 비교해 큰 쪽이 이긴다.
4. stamp 가 완전히 같은데 본문이 다르면(정상 동작에서는 생기지 않는다) 키 기반 본문 다이제스트가 큰 쪽이 이긴다.

이 순서는 전순서(total order)다. 레코드가 어떤 순서로 도착하든 모든 기기가 같은 최댓값에 이른다.
- 같은 레코드를 두 기기에서 동시에 고치면 한쪽 버전만 남는다. 충돌 사본은 만들지 않는다(도구와 정책이 Vault 항목을 id 로 찾기 때문이다). 아직 올리지 못한 로컬 수정이 원격 버전에 밀려 사라지면, 그 기기의 결과에서 `conflicts` 가 늘어난다.
- 정책 문서는 통째로 한 레코드다. 두 기기에서 동시에 고치면 필드를 합치지 않고 한쪽 문서가 이긴다.
- 삭제 표시는 지우지 않고 계속 보관한다. 늦게 동기화하는 기기에도 삭제가 전파되게 하기 위해서다. 정책 레코드의 삭제 표시는 적용할 것이 없으므로 무시하고, 다음 실행에서 로컬 정책을 다시 올린다.
- 지금 쓸 수 없는 종류(잠긴 Vault, 꺼진 설정 동기화)의 레코드는 비교하지도 적용하지도 않는다. 잠겨서 보이지 않는 항목을 삭제로 처리하지 않는다. 받은 원격 버전은 `deferred` 로 세고 entity tag 를 기록하지 않으므로, 다음 실행에서 다시 받는다.
- Lamport 상한(2^62)은 받을 때와 매길 때 같다. 그래서 어느 기기가 올린 stamp 든 다른 기기가 받아들일 수 있다. 상한을 넘는 원격 stamp 는 `rejected` 로 세고 counter 에도 반영하지 않는다.
  - 소진: 다음 stamp 가 상한을 넘게 되면 실행은 아무것도 주고받지 않고 `409 sync_counter_exhausted` 로 실패한다. 로컬 수정은 버리지 않고 `pending` 에 남는다. `GET /v1/sync` 와 `openside sync status` 의 `exhausted` 가 `true` 가 된다. 이 상태는 재시작해도 그대로다. 정상 사용으로는 닿지 않으므로 키를 가진 기기의 공격으로 보고, 새 계정을 만들어 다시 페어링한다.
- 이 규칙으로 수렴하려면 세 가지가 함께 지켜져야 한다. stamp 를 미리 쓰기로 저장해 같은 stamp 를 두 번 내지 않아야 하고(§5), 실행 중 로컬 쓰기를 막아야 하며(§5), 같은 레코드를 다시 적용해도 결과가 같아야 한다(메모리 `sync_ids`).
- 수렴은 proptest 로 검사한다(`any_interleaving_converges`). 두 기기에서 네 종류 레코드의 수정·삭제, Vault 잠금, 동기화, blob 일부를 올린 뒤 죽고 마지막 checkpoint 에서 재시작하는 동기화를 임의 순서로 섞어 실행한다. 그 뒤 양쪽이 각각 동기화하면 상태와 stamp 가 같아지고, 한 번 더 동기화해도 아무것도 바뀌지 않는다.

## 7. 위협 모델
- 저장소(WebDAV 서버, 공유 디렉터리)와 그 운영자는 신뢰하지 않는다. 이들은 레코드 종류·항목 이름·URL·제목·비밀 값·메모리 내용·정책 항목을 보지 못한다. 대신 메타데이터는 본다. 계정 id, 레코드 수, 레코드 키(같은 레코드는 늘 같은 키라서 변경 이력을 이어 볼 수 있다), blob 크기, entity tag, 쓰기 시각·빈도·접근 패턴이다. 크기와 패턴으로 종류를 추측할 수도 있다. 패딩이나 트래픽 숨김은 없다. 테스트는 저장소의 모든 파일(이름 포함)에서 정해 둔 평문 카나리를 찾아 0건임을 확인한다. 이것은 정해 둔 값에 대한 검사이며 일반적인 기밀성 증명은 아니다.
- 변조: blob 을 바꾸거나 다른 레코드·계정으로 옮기면 AEAD 인증에 실패한다. 이런 레코드는 `rejected` 로 세고 적용하지 않는다. 형식이 맞지 않는 본문(잘못된 origin, 잘못된 정책 등)도 같다.
- 잘못된 키: 다른 패스프레이즈로 유도한 키는 레코드 키도 다르고 blob 도 열지 못한다. 페어링은 check 불일치로 거절된다.
- 롤백·누락: 저장소가 예전 blob 을 돌려주거나 레코드를 숨길 수는 있다. 기기는 stamp 가 더 낮은 버전을 적용하지 않고 자기 버전을 다시 올린다. 이 보호는 기기에 남은 stamp 에서 나오며, AEAD 가 최신성을 보장하지는 않는다. 새로 페어링했거나 상태를 잃은 기기는 기준이 없으므로 되돌린 저장소를 그대로 받아들일 수 있다. 숨긴 레코드는 그동안 전파되지 않는다(가용성 공격은 막지 않는다).
- 오프라인 추측: 저장소는 blob 으로, 페어링 코드를 본 사람은 코드만으로 패스프레이즈를 오프라인에서 추측해 볼 수 있다. 코드에는 salt 재료(계정 id)와 32비트 check 가 들어 있어서 추측이 맞는지 확인할 수 있기 때문이다. 계정 id 는 무작위지만 패스프레이즈의 엔트로피를 보태지 않는다. check 는 오타 확인용이지 인증 수단이 아니다. Argon2id 비용은 추측을 늦출 뿐이므로 길고 추측하기 어려운 패스프레이즈를 쓰고, 페어링 코드는 공개하지 않는다.
- WebDAV 자격 증명은 저장소 접근만 허락하고 복호화 능력은 없다. 유출되면 레코드를 덮어쓰거나 지울 수 있지만, 덮어쓴 blob 은 인증에 실패해 적용되지 않는다. 지운 레코드는 그 레코드를 가진 기기가 다음에 동기화할 때 다시 올린다. 평문 http 는 loopback 주소에만 허용해 비밀번호가 네트워크에 노출되지 않게 한다.
- 정책 동기화: 켜면(`sync.settings = true`) 패스프레이즈를 가진 기기가 다른 기기의 정책(허용·거부 origin, 규칙, 기본 모드)을 바꿀 수 있고, 정책을 약하게 만드는 변경도 그대로 적용된다. 형식 검증만 하고 승인 절차는 없으며, 데몬 stderr 에 한 줄을 남길 뿐이다. 그래서 기본값은 꺼짐이다.
- 기기 침해(메모리의 키, 잠금이 풀린 Vault)는 범위 밖이다.

## 8. API·CLI·설정
- `GET /v1/sync`(read): `configured`, `backend`(`webdav`·`dir`), `set_up`, `unlocked`, `account_id`, `device_id`, `last_sync`, `records`, `pending`, `exhausted`.
- `POST /v1/sync/setup`(admin, 201), `POST /v1/sync/pair`(admin), `POST /v1/sync/run`(admin). 오류 코드: `sync_not_configured`·`sync_already_set_up`·`sync_not_set_up`·`sync_passphrase_unavailable`·`sync_password_unavailable`·`sync_busy`·`sync_counter_exhausted`(409), `invalid_pairing_code`·`sync_wrong_passphrase`(400), `sync_backend_unavailable`·`sync_unauthorized`(502).
- 핀 탭: `GET /v1/pinned-tabs`(read), `POST /v1/pinned-tabs`(admin, 201, `invalid_url`), `DELETE /v1/pinned-tabs/{id}`(admin, 204).
- CLI: `openside sync setup|pair <코드>|now|status`(`--json` 지원).
- 설정: [`[sync]`](../CONFIGURATION.md) 의 `webdav_url`(+ `webdav_username`·`webdav_password_cmd`) 또는 `dir`(둘 중 하나), `passphrase_cmd`, `settings`, `memory_tags`.

## 9. 테스트
- `crates/openside-sync/src/tests.rs`: 봉인·변조·잘못된 키, 규칙별 시나리오, Vault 잠금 시 보류, 가짜 WebDAV 서버(Basic 인증·변경된 것만 다시 받기·카나리 검사·잘못된 키로는 전부 `rejected`), 디렉터리 백엔드 카나리 검사, 저장소에서 지워진 blob 재업로드. 장애 주입: PUT 성공 직후 죽고 재시작해도 stamp 를 다시 쓰지 않음, 같은 stamp·다른 본문의 수렴, pull 과 push 사이에 끼어든 쓰기로 생기는 412 → 다시 pull·병합·재시도, 8회 소진 시 `Contention`, 범위를 넘는 Lamport 거절, 선택에서 빠진 로컬 데이터 보호. 실서버 모양의 multistatus 파싱(네임스페이스·404 propstat·약한 ETag 거절). 크래시 연산을 포함한 수렴 proptest.
- `crates/openside-vault/src/sync.rs`: 내보내기→다른 Vault 로 가져오기, 삭제 영속성, 감사 기록. id 가 다른 본문은 기존 항목을 건드리지 않고 거절. 감사 로그 쓰기에 실패하면 저장하지 않음(마스크는 그 전에 만들어짐).
- `crates/openside-memory/src/sync.rs`: 태그로 고른 메모리 목록. 받은 메모리와 동기화 id 가 재시작 뒤에도 남고 다시 받아도 중복되지 않음.
- `crates/opensided/src/tests/sync.rs`: 데몬 두 개와 가짜 WebDAV 서버 하나로 정책·핀 탭·Vault 항목·태그 메모리를 동기화하고, 태그 없는 메모리는 남는지, 삭제 표시 우선, 저장소 카나리 0건, 변조 blob 거절, 잘못된 패스프레이즈·WebDAV 비밀번호 거절을 확인한다. 동기화 중에 들어온 정책 PUT·메모리 생성이 실행이 끝날 때까지 기다렸다가 다음 실행에서 전파되는지, 태그를 뗀 메모리가 다른 기기의 동시 삭제에도 남는지도 확인한다.
- `crates/openside-e2e/tests/sync.rs`: 실제 데몬 바이너리 두 개와 CLI 로 같은 흐름을 검사하고, 저장소(파일 이름 포함)와 두 기기의 데이터 디렉터리·로그에 비밀 값·WebDAV 비밀번호 카나리가 없는지 확인한다.

## 10. 한계
- 동기화는 수동이다(`sync now`, API). 주기 실행은 없다.
- 핀 탭 목록은 아직 브라우저 탭 스트립과 연결되어 있지 않다. 목록을 저장하고 동기화하는 데까지만 한다.
- 실제 WebDAV 서버와의 호환성은 확인하지 않았다(§4).
- 디렉터리 백엔드의 조건부 쓰기는 최선 노력이다(§4).
- 계정 삭제, 패스프레이즈 변경(키 교체)은 아직 없다. 지금은 새 계정을 만들어 다시 페어링해야 한다.
