# openside-agent

openside의 모델 실행 엔진인 Python Pydantic AI 사이드카(sidecar) 컴포넌트다.

## 아키텍처 개요

openside는 세션 관리, 보안 정책, 볼트(vault), 브라우저 제어, 티켓, 도구 실행을 Rust 데몬에서 전담한다. Python 사이드카는 모델 루프를 구동하며, 역방향 RPC(reverse-RPC) 프록시를 통해 데몬 측 도구를 호출한다.

## 설치 및 실행

저장소 루트 디렉터리에서 의존성을 설치한다.

```bash
uv sync --project agent
```

### 사이드카 실행

일반적으로 Rust 데몬이 환경변수(`OPENSIDE_SIDECAR_SOCKET`, `OPENSIDE_SIDECAR_NONCE`)를 주입하여 사이드카 프로세스를 직접 시작한다. 수동 실행 명령은 다음과 같다.

```bash
uv run --project agent python -m openside_agent
```

### 진단 도구 (doctor)

환경 설정 및 인증 소스 상태를 진단한다. 설정 JSON은 `OPENSIDE_DOCTOR_CONFIG` 환경변수로 전달한다.

```bash
OPENSIDE_DOCTOR_CONFIG='{"credentials":{},"models":{}}' uv run --project agent python -m openside_agent doctor --json
```

## 프로바이더 인증 소스 우선순위

데몬이 계정 선택, 명시적 풀 신뢰, 스티키 어피니티, 사용량·차단 상태, 관리 토큰 갱신을 전담한다. 사이드카는 외부 요청 시도마다 비공개 접근 자격 증명을 1개만 발급받아 사용하고 정제된 결과만 보고하며, 자격 증명 홈 디렉터리, 브로커 스냅샷, 리프레시 토큰은 일절 읽지 않는다. 관리 대상 로컬 OAuth 상태는 암호화된 Vault 및 비공개 CLI 홈에 정식 보관하며, 이는 메모리 재사용만을 허용하던 기존 방식을 의도적으로 대체한다. 기본 사용자 홈은 계속 외부 읽기 전용 상태를 유지한다. 자세한 내용은 `../docs/specs/models-accounts.md`를 참고한다.

### Request credentials

비공개 데몬 리스는 `providers/credential_rpc.py`, `request_model.py`, `request_auth.py`에 구현되어 있다. 외부 요청을 시도할 때마다 `credential_request`를 전송하고, 전달받은 `auth`를 해당 프로바이더 원격 오리진에서만 사용한 뒤 `credential_report`를 전송한다. 계정 선택, 스티키 어피니티, 리비전 관리, 갱신, 사용 자격 판정은 데몬이 전담하며, Python 측에는 소스 캐스케이드, 브로커 클라이언트, 자격 증명 홈 리더, 사이드카 핀 저장소가 없다.

구체적인 통신 규격은 [`sidecar-protocol.md` §6.1](../docs/specs/sidecar-protocol.md)에 정의되어 있다. 시도 횟수는 0부터 63까지 0 기반 번호로 매긴다. 분류 결과는 `success`, `auth`, `rate_limit`, `quota`, `policy`, `transient`, `cancelled`이며, 데몬은 이에 대해 `complete`, `refresh`, `rotate`, `stop` 중 하나로 응답한다. 자격 증명을 다시 획득하기 전에 실패 확인 응답이 먼저 완료되어야 한다. 스트림이 노출된 이후에는 요청을 재실행하지 않으며, 취소 보고는 단방향 전송만 수행하므로 종료 처리가 중단된 디스패처를 기다리지 않는다.

기본 fail-closed 정책은 암묵적인 소스 다운그레이드를 금지한다. G-2 규칙은 사람이 명시적으로 구성한 프로바이더 풀 내부에서만 여러 소스 간 교차 전환을 허용한다. 명시적 모델 폴백은 이와 별도로 유지되며, `AuthUnavailable`은 결코 `FallbackModel`의 폴백 조건으로 쓰이지 않는다. 계정 풀 구성과 계정별 외부 로그인 명령 관리는 데몬의 소관이다. [`models-accounts.md`](../docs/specs/models-accounts.md)를 참고한다.

### SDK authentication

지속형 SDK 클라이언트는 자리표시자 키만 보관한다. 요청 범위 컨텍스트가 임시 API 키와 접근 토큰을 HTTP 인증 단계에 공급하며, 필요할 때는 Codex용 비공개 업스트림 계정 ID도 함께 전달한다. `openai_client=` 주입은 Codex SDK 자체의 자격 증명 파일 읽기 및 갱신 수명주기를 비활성화한다. SDK 차원의 자격 증명 재시도는 꺼져 있다. Claude Code 셰이핑은 Anthropic API 키가 아닌 `claude_code` 리스에만 적용되며, 버전은 `OPENSIDE_CLAUDE_CODE_VERSION` 또는 `credentials.anthropic.claude_code_version`으로 설정할 수 있다.

자체 소유한 가변 JSONL 및 토큰 버퍼는 요청 시도가 끝날 때 초기화(wipe)하며 주입된 헤더도 제거한다. `StreamWriter.write()`에는 불변 사본을 전달하는데, `drain()`은 낮은 워터마크에 도달할 때까지만 대기할 뿐 완전한 플러시 확인을 보장하지 않기 때문이다. CPython, JSON, SDK, 전송 계층의 불변 사본은 신뢰성 있게 지울 수 없으므로, 프로세스 메모리가 완전히 지워진다고 보장하지는 않는다. 토큰, 프로바이더 원문 응답 본문, 예외 텍스트는 보고서, 상태 스냅샷, 공개 진단 출력, 로그에 결코 남기지 않는다.

### Credential-free diagnostics

`python -m openside_agent models --json`은 선별된 SDK 색인 `{providers:[{provider,models:[{id,label}]}]}`만 반환한다. 설정값과 계정에서 보고된 모델을 병합하고 계정 가용성, 풀, 사용량을 덧붙이는 작업은 Rust가 수행한다. `accounts`와 `doctor`는 데몬이 제공하는 정제된 `credentials._diagnostics.accounts` 메타데이터를 활용할 수 있다. 단독 진단 도구는 CLI 설치 여부만 감지하며 로그인 준비 상태는 알 수 없음으로 둔다. CLI 인증 파일, 브로커 스냅샷, 환경변수 키 값, 자격 증명 명령 출력은 읽지 않는다.


## 핵심 파이썬 API

모델 식별자는 고정 ID 없이 사용자가 명시한 `provider:model` 형식을 따르며, 대체 모델(fallback) 목록 역시 사용자가 직접 설정한다.

```python
async def build_model(
    model_id: str,
    fallback: list[str],
    credentials: Mapping[str, Any],
    *,
    credential_rpc: CredentialRpc,
    subagent: str | None = None,
) -> Model: ...


def doctor_report(
    credentials: Mapping[str, Any],
    models: Mapping[str, Any],
) -> dict: ...
```

`Model`과 `Agent` 컨텍스트는 세션 동안 열어 두고 세션이 종료될 때 닫는다. 모델 인스턴스가 자체 HTTP 클라이언트를 소유한다. 런타임의 `Session.credential_exchange`는 모델에 노출되는 도구 호출과 별개로 자격 증명 응답을 다중화한다. 요청 간에 자격 증명 스냅샷을 캐시하지 않으며, 사이드카 상태로 직렬화되는 자격 증명 어피니티도 없다. `pins` 키가 포함된 과거 상태는 해당 키를 버리고 디코딩한다.

## Structured results with `present`

`present`는 데몬이 제공하는 카탈로그와 기존 리버스 RPC 프록시를 통해서만 노출되며, Python Card 스키마, 새 ToolCall 변형, 와이어 프로토콜 버전 변경은 추가되지 않는다. 런타임 지침은 허용 카탈로그에 `present`가 포함되어 있을 때 단일, 프론트, chief, teammate, 위임 에이전트에 적용되며, 기존 팀 역할, 페르소나, 로스터 지침은 그대로 유지된다.

상품·장소 목록, 비교표, 주문·결제 요약, 양식 요약, 연결 의도에는 한정된 비대화형 카드를 사용한다. 카탈로그 스키마와 [`cards.md`](../docs/specs/cards.md)의 제한 기준을 준수한다. 도구는 `{card_id}`만 반환하며, 실제 카드를 화면에 표시하는 작업은 데몬이 맡는다. 카드 전체 내용이나 Markdown 표를 중복 출력하지 말고 짧은 말풍선으로 안전한 문맥만 제시한다.

예를 들어 비교 결과는 문자열 및 통화 타입 셀이 들어간 `table`로 제시한 뒤 “Here is the option I found.”라고 안내한다. 주문 요약은 `kv_rows`와 정수 통화 최소 단위를 담은 `order_summary`로 제시한 뒤 “Review the total; no order has been placed.”라고 안내한다. 런타임 지침에 포함된 실행 가능한 비교 예제는 `tests/test_present.py`에서 생성된 데몬 카탈로그를 기준으로 검증한다.

로그인, OTP, 페이지 양식 입력 요청에는 `request_login`, `request_otp`, `request_form`을 사용하며 임의로 꾸며낸 대화형 카드 표현은 사용하지 않는다. 자격 증명과 결제 필드 값은 데몬의 검증된 페이지로 직접 전달해야 하며, `present`나 일반 텍스트에 넣어서는 안 된다. 보안 처리 결과에는 민감한 비밀값 대신 안전한 상태 메타데이터만 담긴다. 데몬의 거절 응답은 기존 모델 재시도로 전달되며, 초안을 수정하더라도 티켓 권한을 임의로 생성할 수는 없다.

```bash
uv run --project agent pytest agent/tests -q
/tmp/openside-heavy-run cargo test -p openside-e2e --test cards cards_present_scripted_roundtrip -j 2 -- --test-threads=1
```

Python 테스트는 독립적이고 격리된 환경을 유지한다. 생성된 `docs/specs/profiles/*.open.json` 카탈로그 스냅샷을 불러오고, 스크립트 기반 모델과 가짜 JSONL 데몬을 사용한다. 카탈로그와 스키마 노출 여부, card-id 전용 반환값, 잘못된 형식이나 권한 초과 거절에 따른 재시도, 보안 상태 전용 반환값, 와이어 및 상태 버전 불변성을 검증한다. 별도의 Rust `cards_present_scripted_roundtrip` 테스트는 기존 스크립트 프로바이더 하네스를 사용해 실제 데몬과 Python 사이드카를 구동하며, 브라우저나 프로바이더 요청 없이 실제 등록 거절과 수정된 카드 표시 과정을 검증한다.

## 테스트 및 검증 명령어

`agent` 디렉터리에서 실행한다.

```bash
uv run ruff check src/openside_agent/providers tests/providers
uv run ruff format --check src/openside_agent/providers tests/providers
uv run pyright src/openside_agent/providers
uv run pytest tests/providers tests/test_credential_rpc.py tests/test_protocol.py -q
```

결정론적 테스트는 임시 `HOME`, `XDG`, CLI 홈 디렉터리를 두고 프로바이더 자격 증명을 비운 상태에서 가짜 데몬과 가짜 HTTP 전송 계층만 사용한다. 429 Retry-After 로테이션, 401 무효화, 턴 간 어피니티 유지, 64회 시도 상한, 타입화된 자원 소진, 단방향 취소, 부분 스트림 재전송 금지, 카나리 토큰 마스킹, 느린 수신자 환경의 대용량(>64 KiB) JSONL 프레임 처리를 테스트한다. 실제 프로바이더 호출 및 브로커 토큰 스모크 테스트는 더 이상 사이드카 진단 경로에 포함되지 않는다.

## Task escalation and history compaction

Worker instructions use known context, authorized connections/APIs, exposed web
search, authorized browser access, then a human question. An absent connector can
be skipped; a broken connector, expired authentication, or denial cannot be
bypassed through a browser or another execution tool. Browser structure precedes
pixels. Security challenges stay human-only. This policy adds no tools and is not
installed on the front agent.

History compaction uses the current session model, not a separate routing policy.
The default history budget is 64,000 estimated tokens. Text and structural
metadata use UTF-8 bytes divided by three (rounded up), counting the current
instructions once rather than their replay copies in earlier requests. Each image
or binary media part reserves 1,600 tokens instead of counting its encoded payload.
Set `compaction_max_tokens` in the
session/model agent configuration to override it. `compaction_max_messages` is an
optional legacy trigger; short conversations are not compacted merely for
reaching 100 messages. The normal tail is 20 messages (`compaction_keep_messages`);
when that tail exceeds the size budget, the last two messages remain verbatim,
extended backward as necessary to preserve tool-call/result pairs. The first user
goal also remains verbatim. A currently indispensable oversized tail is retained,
not silently truncated; the daemon's Task spill path owns large-result storage.

After a settled turn, idle compaction waits 250 ms and skips known queued work,
pending approvals, and active children. Incoming work cancels only the speculative
summary, never a host tool. A history fingerprint, history identity, prompt epoch,
and generation compare-and-set discard stale summaries. Committed compaction
invalidates browser snapshot bases and persists a new continuation snapshot;
opaque spill artifacts retain their independent daemon retention policy. The
prompt epoch stays fixed. Failed idle summaries leave the original history intact.

## Prompt epochs and cache diagnostics

Each parent and Task agent assembles a prompt epoch before creating its Agent.
The epoch stores canonical name-sorted daemon tool definitions and separate base
system, persona, skill-index, policy, and memory-profile sections in the opaque
daemon snapshot. Resume reuses these exact bytes, even after deployment changes;
removed tools still face daemon-side authorization at execution time. New sessions
create new epochs. Explicit `PromptEpoch.assemble(..., transition=True)` is the
reset boundary; ordinary turns, deployment, and history compaction do not reset it.
Legacy snapshots without an epoch establish one on their next open.

Live roster and selected memory context belong at the latest model-message
boundary, never in the frozen instructions. Changed stable sections produce only
a data-only pending-change notice until the next epoch. Ephemeral context is
request-local rather than repeatedly copied into durable conversation history.

Every parent/Task model request logs `prompt_sections`: section SHA-256 hashes,
Unicode character lengths, an opaque epoch ID, and change causes. First-message
hashes detect history-prefix replacement; provider/model identities are hashed.
Credential acquisition logs `prompt_request_identity`, including hashed account
identity and a distinct account-boundary cause for rotation. These stderr records
contain no prompt text, raw account IDs, or credentials. Hashes are diagnostics,
not encryption or a guarantee that low-entropy content cannot be guessed.

Provider-created Anthropic and OpenAI models default to Pydantic AI `cache=True`.
The installed adapter maps this to Anthropic stable system/tool cache controls
and conversation caching; supported OpenAI models receive explicit instruction
breakpoints, while other OpenAI models retain provider-side implicit caching.
Explicit per-request/provider settings take precedence. Mock-transport tests
inspect the actual serialized provider requests and usage parsing. They do not
establish live hit rates: minimum prompt sizes, account/model boundaries, provider
support, and retention still determine real cache reuse. No paid prefetch occurs.
