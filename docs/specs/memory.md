# 기억 (Memory) — 로컬 메모와 브라우징 기록

상태: **구현됨**. 관련: A-11. 코드: `crates/openside-memory`(저장소·검색·시크릿 검사·임베딩), `crates/opensided/src/memory.rs`(데몬 연결·API·purge), `crates/opensided/src/tools/memory.rs`(에이전트 도구·기록 색인), `crates/openside-proto/src/memory.rs`(API 타입), `crates/openside-cli/src/memory.rs`(`openside memory`), `crates/openside-config/src/memory.rs`(`[memory]`, doctor).

## 1. 저장
- 데이터는 `<data_dir>/memory.db` 하나에 저장한다(SQLite, 권한 0600). 표는 `memories`(본문), FTS5 `memories_fts`(제목·본문·태그 전문 검색), `memory_versions`(변경 이력)다.
- 종류(`kind`): `note`(사용자·에이전트가 남긴 메모), `history`(에이전트가 본 웹 페이지).
- 한도: 본문 1–16384바이트, 제목 512바이트, URL 2048바이트, 태그 32개(각 1–64자).
- `PRAGMA secure_delete` 와 FTS5 `secure-delete` 를 켜 두어, 지운 내용이 DB 파일의 빈 페이지에 남지 않는다.
- `Memory.owner`는 공유 메모의 경우 null이고 개별 소유 메모는 해당 에이전트 ID다. 기존 데이터베이스는 메모를 다시 쓰지 않고 nullable owner 열을 추가한다. 에이전트 검색은 순위 매기기 및 제한을 적용하기 전에 본인 소유와 공유 메모(own-plus-shared) 범위를 먼저 적용한다. teammate의 쓰기는 소유 메모가 되고 chief의 쓰기는 공유 메모가 되며, teammate는 공유 쓰기를 요청할 수 없다. `GET /v1/memory?owner=<id>|shared`로 소유권을 필터링한다. 생명주기 이전(lifecycle transfer)은 멱등한 compare-and-set을 사용하며, 무관한 이후 소유자를 덮어쓰지 않는다.

## 2. 검색과 순위
- 어휘 검색: 질의에서 영숫자 단어만 뽑아 `"단어"*` 접두 검색을 OR 로 묶고 FTS5 bm25 순으로 정렬한다. 질의가 비어 있으면 최근 수정 순으로 나열한다.
- 벡터 검색: 로컬 임베딩 모델이 있으면 저장할 때 본문(제목·태그 포함)을 임베딩해 두고, 질의 벡터와 코사인 유사도를 계산한다. 유사도 0.3 이상만 후보로 쓴다.
- 하이브리드 순위: 두 목록에서 각각 상위 50개를 뽑아 RRF(reciprocal rank fusion, `k=60`)로 합친다. 점수는 `Σ 1/(60 + 순위)` 이고, 결과마다 `matched`(`lexical`·`vector`·`both`·`recent`)를 붙인다. 최대 100개.
- 모델이 없거나 읽을 수 없으면 `mode=lexical_only` 로 어휘 검색만 한다. 모델을 나중에 넣고 데몬을 다시 시작하면 기존 기억도 임베딩을 채운다(backfill).

## 3. 임베딩 모델
- 형식: model2vec 계열 정적 임베딩. `tokenizer.json`(WordPiece만 지원) + `model.safetensors`(텐서 `embeddings` 또는 2차원 텐서 하나, F32/F16/BF16). 토큰 벡터 평균 후 정규화하고, UNK 토큰은 뺀다.
- 지원하는 토크나이저 설정: normalizer 는 없음·`BertNormalizer`·`Lowercase`, pre-tokenizer 는 없음·`BertPreTokenizer`. 그 밖의 종류와 `strip_accents=true`, `handle_chinese_chars=false` 는 로드할 때 거절한다. `strip_accents` 를 비워 둔 lowercase 모델도 악센트는 지우지 않으므로, 악센트가 붙은 글자는 학습 때와 다른 토큰이 될 수 있다.
- 위치: `memory.embedding.model_dir`(기본 `<data_dir>/memory/model`). 데몬을 `--data-dir` 로 띄우면 CLI 와 경로가 달라지므로 `model_dir` 을 직접 지정한다.
- 받기: `openside memory model fetch <owner/name> [--revision R] [--endpoint URL]`. 데몬 없이 실행되는 유일한 다운로드 경로다. 두 파일을 `.<이름>.generations/` 아래 임시 디렉터리에 받아 실제로 로드해 보고, 통과하면 바꾸지 않는 세대 디렉터리(`gen-*`)로 확정한다. `model_dir` 은 현재 세대를 가리키는 심볼릭 링크이고, 새 링크를 rename 한 번으로 바꿔 끼운다. 로더는 `model_dir` 을 한 번만 해석한 뒤 두 파일을 같은 세대에서 읽으므로, 설치 중에 로드해도 서로 다른 모델의 토크나이저와 가중치가 섞이지 않는다. 다운로드·검증이 실패하면 현재 모델은 그대로다. 교체된 세대는 60초가 지난 뒤 다음 설치 때 지운다.
- 설치는 `.<이름>.lock` 으로 하나씩 진행한다. 링크가 아닌 일반 디렉터리였던 `model_dir`(직접 넣은 모델)을 처음 바꿀 때만 rename 이 두 번 필요하다. 이때는 `.<이름>.journal` 에 새 세대를 먼저 기록하고, 중간에 중단되면 다음 `model fetch`(다운로드에 실패해도)가 시작할 때 기록된 세대로 링크를 마저 만든다. 그 사이에는 모델이 없어 어휘 검색만 한다. 심볼릭 링크를 쓰므로 Unix 전용이다.
- `--endpoint`(`OPENSIDE_HF_ENDPOINT`)와 리다이렉트로 거치는 모든 주소는 https 여야 한다. http 는 loopback 테스트 서버에만 허용한다. 리다이렉트는 최대 10번.
- `GET /v1/memory/model`은 다운로드 작업, 설치된 모델, 재시작 필요 여부를 보고한다. 관리자는 `POST /v1/memory/model/download` 및 `/cancel`로 다운로드를 시작하거나 취소할 수 있으며, 대체 엔드포인트·리포지토리·리비전을 사용하려면 로컬 권한이 필요하다. 데몬과 CLI는 `openside-model-fetch`, 체크섬 검증, egress 검사, 원자적 설치 로직을 공유한다. 암묵적으로 시작되는 다운로드는 없다. `GET /v1/doctor`는 터미널 명령을 요구하는 대신 Memory 다운로드 버튼을 제공한다.
- 네트워크: `openside-memory` 크레이트는 HTTP·TLS·소켓 크레이트에 의존하지 않는다(`tests/no_network.rs` 가 `cargo metadata` 로 확인). 저장·검색·삭제·purge 중에 소켓이 열리지 않는 것도 Linux `/proc/self/fd` 로 확인한다.
- 상태: `openside doctor` 의 `memory.embeddings` 항목은 파일이 있는지만 본다(파일이 있어도 실제로 쓸 수 있는지는 판단하지 않는다). 실제 동작 모드는 `GET /v1/memory/health`(로드 실패 이유 `problem`, 다음 단계 `next_step`, 모델 ID, 개수)로 확인한다.

## 4. 보존 기간
- `retention`: `permanent`, `days_90`, `days_30`, or `hours_24`. The configured default is unchanged. Notes tagged `scratch` always expire 24 hours after their last write, even if another retention was requested.
- 만료 시각은 마지막 쓰기(생성·수정)부터 센다. 만료된 기억은 purge 작업을 기다리지 않는다: 조회·검색에서 바로 빠지고, 이력 조회·수정·삭제·기록 색인은 먼저 만료된 기억을 purge 한 뒤 처리한다. 그래서 만료 뒤 같은 URL 을 다시 방문하면 이전 요약 없이 버전 1 부터 새로 시작한다.
- 데몬은 시작할 때와 이후 1시간마다 만료된 기억을 지운다(purge). 이력에는 `op=purge`, `actor=retention` 으로 남는다.

## 5. 변경 이력
- 생성·수정·삭제·purge 마다 `memory_versions` 에 버전 번호, 시각(`at`), 주체(`actor`), 작업(`op`), 그 시점의 본문·태그·보존 기간을 남긴다. 읽기·수정·버전 증가·이력 기록은 한 트랜잭션에서 처리하므로, 동시에 다른 필드를 고쳐도 변경이 사라지거나 버전이 겹치지 않는다. 같은 URL 의 첫 방문이 동시에 와도 행은 하나다.
- 주체: API·CLI 는 `api`, 에이전트 도구는 `agent:<session_id>`, 브라우징 기록은 `browser_history:<session_id>`, purge 는 `retention`. 주체는 영숫자와 `: _ - . @` 로 된 128자 이하 식별자만 받는다.
- 삭제와 purge 는 본문 행과 검색 색인을 지우고, 이전 버전들의 본문·태그도 비운다. 이력에는 언제 누가 무엇을 했는지만 남는다. 데이터베이스는 열 때마다 `secure_delete`, FTS5 `secure-delete`, `journal_mode=DELETE` 를 다시 설정하므로, 지운 내용은 DB 파일과 저널에도 남지 않는다.

## 6. 시크릿 차단
저장(생성·수정·기록 색인) 전에 본문·태그·제목·URL·주체와, 색인·임베딩에 쓰는 합친 텍스트(제목+본문+태그)를 검사해, 걸리면 `secret_detected` 로 거절하고 아무것도 쓰지 않는다. 기록 색인은 600자로 자르기 전의 전체 페이지 텍스트를 검사한다. 오류 응답과 로그에는 값을 넣지 않는다.
- 형태 검사: PEM 개인 키, JWT, `sk-`·Stripe·AWS(`AKIA`/`ASIA`)·GitHub(`gh*_`, `github_pat_`)·Slack(`xox`)·Google(`AIza`)·GitLab(`glpat-`)·Hugging Face(`hf_`)·npm 토큰, `Bearer` 토큰, `password=`·`passwd`·`pwd`·`passphrase`·`비밀번호` 형태의 값(JSON 처럼 따옴표로 감싼 키·값 포함), `api_key`/`token`/`secret` 대입, Luhn 검사를 통과하는 13–19자리 카드 번호.
- 정규화: 원문 외에 퍼센트 인코딩, 백슬래시·JSON 이스케이프(`\uXXXX`), HTML 엔티티를 한 번 푼 형태와, 보이지 않는 문자(zero-width, soft hyphen 등)를 지운 형태도 검사한다. base64, 여러 번 겹친 인코딩, 여러 번에 나눠 쓴 값은 잡지 못한다.
- 금고 값: 데몬의 `Redactor` 가 아는 값(잠금 해제된 금고 항목)은 형태와 관계없이, 위 정규화 형태까지 포함해 거절한다. 그 값이 기억을 쓴 뒤에 알려졌다면 API 와 `memory_search` 결과에서 가린다.

## 7. 브라우징 기록
- Only substantive accessibility captures create or refresh visited-page `history` records (`days_30`). Navigation without a capture and title-only pages do not create records or overwrite an existing summary.
- Captures with protected fields or recognized sign-in/password forms are not indexed. Navigation, links, buttons, forms, headings, banners, footers, sidebars and advertising subtrees are excluded. Eligible prose must contain at least 80 alphanumeric Unicode characters and must not be outweighed by excluded chrome; the stored summary is capped at 600 characters. Page history never receives profile, event-log or scratch tags.
- 건너뛰는 경우: `memory.history=false`, 비공개(incognito) 세션(상태를 확인할 수 없는 세션도 비공개로 본다), 격리 브라우저 공간(`browser_space: "isolated"`) 세션, 시크릿 입력이 들어간 문서(복구된 문서 포함, 문서를 확인할 수 없을 때도), http(s) 가 아닌 주소, 페이지 텍스트·제목이 시크릿 검사에 걸린 페이지. 시크릿 입력 표시는 문서 ID 에 묶여 있고, 색인 직전에 문서 ID 가 바뀌었으면 저장하지 않는다.
- 비공개 세션은 만들 때 정한다: `POST /v1/sessions {"incognito": true}`. 값은 `session.created` 이벤트와 세션 상태에 저장되어 데몬을 다시 시작해도 유지되고, 바꿀 수 없다. `Session.incognito` 로 보인다. 이 값을 보고 세션 전용 임시 브라우저 프로필을 쓰고 금고 사용을 막는다(B-10, `docs/09-tool-design.md` §3).

## 8. 에이전트 도구
| 도구 | 등급 | 인자 | 결과 |
| --- | --- | --- | --- |
| `memory_search` | Read(모든 모드에서 보임) | `query`, `limit?`(1–20, 기본 5), `kind?` | `mode`, `results[]`(`id`, `kind`, `content`, `title`, `url`, `tags`, `updated_at`, `matched`, `source`, `untrusted`) |
| `memory_write` | Write(read-only 모드에서 숨김) | `content`, `tags?`, `retention?` | `id`, `stored`, `retention`, `expires_at`. 시크릿이면 `denied`/`secret_detected` |

All recall is data-only: both notes and browsing history carry `untrusted=true` and `authority=data_only`, with their source and owner preserved. Recalled text is never instruction authority. Results pass through daemon redaction. Secret-looking `memory_write` arguments are recorded as `[REDACTED]`.

## 9. API / CLI
권한: 조회는 `read`, 생성·수정·삭제는 `admin`.

| 메서드·경로 | 설명 | CLI |
| --- | --- | --- |
| `GET /v1/memory?q=&kind=&limit=` | 검색(`MemorySearchResponse`), `q` 가 없으면 최근 순, 기본 20개 | `memory search [QUERY] [--kind note\|history] [--limit N]` |
| `POST /v1/memory` | 메모 생성(201) | `memory add TEXT [--tag T]... [--retention R]` |
| `GET /v1/memory/health` | 검색 모드·모델 상태 | — |
| `GET /v1/memory/{id}` | 조회 | `memory show ID` |
| `PATCH /v1/memory/{id}` | 본문·태그·보존 기간 수정 | `memory edit ID [--content T] [--tag T]... [--clear-tags] [--retention R]` |
| `DELETE /v1/memory/{id}` | 삭제(삭제된 `Memory` 를 200 으로 돌려준다) | `memory rm ID` |
| `GET /v1/memory/{id}/history` | 변경 이력(`MemoryHistory`), 삭제된 기억도 조회 가능 | `memory history ID` |

오류 코드: `secret_detected`(400), `invalid_value`(400), `not_found`(404), `internal_error`(500).

## 10. Bounded prompt context and consolidation

Explicit note tags define three context layers without migrating or reclassifying existing notes or browser history:

- `profile`: at most 100 facts, within 4000 Unicode characters including the provenance envelope. The profile is frozen in the persisted prompt epoch for chief and Task prompts. Ordinary turns do not replace the cached prefix.
- `event_log`: dated notes updated within the last 30 days, selected by lexical relevance before recency, with deterministic identifier tie-breaking. At most 30 logs enter the latest-message context.
- `scratch`: short-lived notes with a mandatory 24-hour expiry. Logs and scratch notes share a separate 6000-character rendered budget. Oversized records are skipped, not partially exposed or granted deletion authority.

Ownership and expiry are filtered before ranking. Sessions without an agent identity see shared notes only, never all owners. Browser/page indexing cannot set these reserved tags; person edits, explicit memory tools, and consolidation record their writer provenance.

The daemon seeds profile context when opening a session. Latest-message recall refreshes only the volatile tail; Tasks inherit the frozen profile and receive task-relevant tail recall. Prompt snapshots survive restart via the prompt-epoch snapshot. Expired scratch notes are absent immediately, before the hourly purge and after restart.

After a completed, settled chief turn, a separate tool-free pass uses the configured front model, falling back to the reasoning model. It receives at most 12000 explicit-owner-message characters plus bounded recall, has one model request, a 2048-output-token ceiling and a 20-second deadline. Tool/page output, provenance-wrapped third-party messages and assistant guesses are excluded from this extraction input. Account names and advertising text seen on a page are not evidence of owner identity or preferences; recalled logs cannot supply new profile facts. Failed or invalid extraction leaves memory unchanged and never changes the visible reply.

Each pass receives a daemon-issued opaque scope bound to its session, owner, and the actually shown `(id, version)` pairs. SQLite atomically checks every edit before applying any: unseen identifiers, expired scopes, stale versions, ownership changes, duplicate edits, and secrets reject the whole pass. A concurrent person edit wins. A successfully consumed scope replays as a no-op, including after daemon restart. Additions are bounded to 20, edits to 100; existing secret screening applies to all writes. No free-form model deletion tool exists.

`__memory_context` and `__memory_consolidate` are private runtime RPCs, not registered model tools. Model proxies refuse `__*` calls; the daemon refuses these names in registrations and rejects private RPCs from subagents or incognito sessions. Internal RPC arguments/results are not recorded as tool events, so rejected extraction content cannot enter persistent event logs.
