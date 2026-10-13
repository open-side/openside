# 08. 하네스: Pydantic AI 사이드카 + 모델 자격증명 소스(선택적 omp broker)

Pydantic AI 사이드카 아키텍처를 채택했다. Models & accounts 제품 결정에 따라 프로바이더 키 저장을 전면 금지하던 이전 방침을 대체한다. 현재 지원하는 소스에는 Vault API 키와 외부에서 관리하는 Claude Code 및 Codex 구독 자격 증명이 포함된다. 아래에 기술된 과거 브로커 및 실시간 호출 결과는 기록으로 유지되며, 현재 자격 증명과 API 동작 방식은 `docs/specs/models-accounts.md`에 정의되어 있다.
표기: 검증 = 이 머신에서 실제 실행, 문서 = 소스·문서로만 확인, `unverified` = 확인하지 못함.

## 1. 결정 요지
- 에이전트 루프, 모델 호출, 보류 도구 재개, compaction 은 **Pydantic AI**(`pydantic-ai-slim 2.55.0`)와 **`pydantic-ai-harness`**(`0.55.0`) 위의 Python 사이드카(`agent/`, 패키지 `openside_agent`)가 맡는다. 데몬이 세션당 프로세스 1개를 띄운다.
- Rust 데몬 `opensided` 는 세션(이벤트 소싱)·큐·정책·Vault·브라우저·티켓·푸시를 소유하고 **모든 툴을 실행**한다. 사이드카의 툴은 reverse RPC 로 데몬을 부르는 얇은 프록시다(docs/09, `docs/specs/sidecar-protocol.md`).
- 데몬이 프로바이더별로 순서가 지정된 명시적 계정 풀을 관리하며, 외부 요청을 시도할 때마다 유효한 비공개 자격 증명을 공급한다. 마이그레이션 시에는 현재 선택된 소스만 유지하며, 사람이 직접 계정을 추가한 경우에만 여러 소스 간 장애 전환(failover)을 허용한다. API 키와 관리 대상 구독 OAuth 상태는 Vault에 암호화하여 저장하고 갱신은 사이드카에서 결코 실행하지 않는다. 기본 사용자 CLI 홈은 읽기 전용으로 유지된다. `docs/specs/models-accounts.md`를 참고한다.
- omp 는 `omp-broker` 소스를 고른 사용자에게만 필요한 **선택적** 의존이다. 기본 엔드포인트·토큰 파일 후보와 설정 키는 `agent/README.md` 에 있다.

## A. 최종안: 사이드카 직결 Provider (게이트웨이 비경유)
```
Rust opensided ──(unix socket JSONL)── Python agent sidecar (Pydantic AI, 세션당 1 프로세스)
                                         ├ tools: 데몬 호스트 툴 프록시 (docs/09)
                                         └ models: OpenAICodexModel / AnthropicModel / …
                                              └ Provider ← fresh private daemon request lease
Rust credential owner ← explicit provider pool (omp-broker · vault · env · command · claude-code · codex-cli · builtin)
                                              └ sidecar streams directly to the upstream provider
```
- `omp-broker` 소스는 broker 에서 **access token 만** 받는다(refresh token 은 broker 가 sentinel 로 가린다). 갱신은 broker 의 백그라운드 refresher(60초 주기, 만료 5분 전, 문서)가 한다.
- 브로커 로그인과 갱신은 브로커가 전담한다. 로컬 관리 대상 구독 계정은 기존의 메모리 전용 보관 규칙을 의도적으로 대체한다. 즉 정식 토큰은 Vault에 암호화하여 저장하고, 데몬만 갱신을 수행하며, 비공개 CLI 자격 증명 파일은 리비전과 수정 시간(mtime)에 따라 반영하거나 가져온다. 계정을 삭제하면 관리 파일은 완전히 지워지지만 기본 사용자 홈은 절대 건드리지 않는다. 독자적인 OAuth 로그인 흐름이나 외부로의 리프레시 토큰 전달은 추가하지 않는다.
- 사이드카가 제공자에 직접 스트리밍하므로 게이트웨이 `serve` 가 필요 없다(§F 의 stdio 스트리밍 제약 해소).

## B. 검증 결과 (이 머신, 실제 호출 — S-14)
| 경로 | 결과 |
| --- | --- |
| Codex 구독: broker 스냅샷 → `OpenAICodexCredentialSource.load` + Pydantic AI `OpenAICodexModel` | **성공** (`pong`, input 11/output 5). 계정은 스냅샷 3개 중 만료 여유가 가장 큰 것 |
| Claude 구독 (Pydantic AI `AnthropicModel` + `AsyncAnthropic(auth_token=…)`), 셰이핑 없음 | **실패** 429 `rate_limit_error` |
| 동일 + `anthropic-beta: oauth-2025-04-20` 만 | **실패** 429 |
| 동일 + 헤더 3종(`anthropic-beta: claude-code-20250219,oauth-2025-04-20`, `user-agent: claude-cli/<버전> (external, cli)`, `x-app: cli`) + 시스템 프롬프트 첫 문장 `You are Claude Code, Anthropic's official CLI for Claude.` | **성공** (`pong`) |
- 해석: Anthropic 은 구독 OAuth 토큰을 Claude Code 형태가 아닌 요청에 **429 로 거절**한다. 같은 계정·직후 비교에서 셰이핑만 바꾸자 성공했으므로 실제 한도 소진이 아니라 형태 검사로 판단한다. 이 최소 셰이핑이 현재 통과 조건이다.
- 이는 pi-ai `claude-code-fingerprint.ts` 가 하는 일의 일부다(서버가 `claude_code_version_too_old` 로 거절하면 요구 버전으로 올려 재시도하는 로직 포함). **이 셰이핑의 유지보수 책임은 우리에게 있다** → 구현은 `providers/anthropic.py` 한 곳에 격리했다(§C).
- broker 스냅샷 형식(검증): 최상위 `credentials/generation/generatedAt/serverNowMs/refresher`; 엔트리 `id, provider, identityKey, rotatesInMs, credential{type, access, refresh, expires(ms), accountId, email, orgId, orgName, authorizedAt}`.
- 보안 취급: 스파이크·구현 모두 토큰을 출력하지 않는다. 로그·에러에 토큰·스냅샷을 넣지 않는다(OP-4). 토큰 파일은 경로만 설정하고 값은 메모리에서만 쓴다. `repr` 제외는 Pydantic AI `OpenAICodexCredentials` 가 처리한다. 자격 증명 저장 및 출력 경계는 카나리 E2E 테스트(accounts API·CLI·doctor 출력, 데몬 및 사이드카 stderr, 이벤트 로그, 스냅샷, 모든 데이터·설정·상태 파일, 생성된 OpenAPI 및 웹 스키마 대상)로 검증을 마쳤으며, 파일 보호와 샌드박스 규칙은 `docs/CONFIGURATION.md` "자격 증명 파일 보호"에 정리되어 있다.

## C. 구현 현황 (계획 대비 변경)
초안의 구현 계획과 실제 결과의 차이다. 근거는 소스다.

| 항목 | 초안 계획 | 구현 결과 |
| --- | --- | --- |
| 자격 증명 계정 | 데몬 주관 풀 및 갱신 | 계정 메타데이터는 7가지 소스 종류를 지원하며, API 키와 CLI 구독은 라벨이 지정된 항목을 여러 개 둘 수 있다. 데몬은 요청마다 비공개 접근 자격 증명을 확인하고 어피니티/사용량/차단을 주관하며, 사람이 명시한 풀 내에서만 장애 조치를 허용한다. Python 진단 정보에는 자격 증명을 포함하지 않으며, 리프레시 토큰은 정본 소유자에게만 유지된다. |
| 계정 선택 | 영속 세션 어피니티 및 귀속 사용량 | 명시적 전역/에이전트 제한 내에서 데몬이 고정/선호 계정을 관리하며, 분류된 인증/속도/할당량/정책 결과에 따라 목록에 등록된 적격 형제 계정만 로테이션한다. 사용량 초기화/예약 정책이 재진입 여부를 제어하며, 확인되지 않은 초기화 시점은 미정 상태로 유지한다. 사이드카 스냅샷에는 자격 증명 고정값을 두지 않는다. 집중적인 데몬/SDK 모의 경계 테스트로 정책을 검증하며, 실서버 브로커 연동을 전제하지 않는다. |
| Fail-closed (P-1) | 풀 소진/불가 시 자격 증명 거부 | 마이그레이션 시 선택한 소스만 유지한다. 브로커 장애가 발생해도 목록에 없는 로컬 소스를 임의로 끌어오지 않으며, 이종 소스 간 장애 조치는 사전에 사람이 직접 소속을 지정해야만 허용된다. AuthUnavailable 상태가 모델 fallback 허용을 의미하지는 않는다. 이후의 모든 아웃바운드 시도는 최신 자격 증명을 새로 조회하므로, warm 상태의 SDK 클라이언트를 재시작하지 않아도 계정 비활성화/삭제/교체가 즉시 적용된다. |
| Codex refresh (S-16) | provider 가 스스로 refresh 하면 `save()` 예외로 발견 | **방식 변경.** 기본 `OpenAICodexProvider(credential_source=…)` 는 sentinel 을 일반 refresh token 으로 보고 OAuth 요청을 보냈다(`oauth-hits=1`, `sentinel-submitted=True`; `save()` 예외는 요청이 나간 뒤라 늦다). `OpenAICodexProvider(openai_client=AsyncOpenAI(http_client=<앱 소유 httpx Auth>))` 로 provider 의 refresh 경로를 끄고, 만료·401 시 소스(broker 스냅샷)를 재조회한다. 검증(MockTransport, 네트워크 없음): `broker 401 source rotated: outcomes=200 loads=2 saves=0 oauth-hits=0`, 동시 401 은 단일 비행(`oauth-hits=0`) |
| Anthropic 셰이핑 | 설정 가능한 버전, `*_too_old` 시 1회 재시도 | 구현됨(`providers/anthropic.py`). 헤더 3종 + 첫 시스템 프롬프트 문장. 기본 버전 `2.1.280`, 재정의는 `OPENSIDE_CLAUDE_CODE_VERSION` 또는 `credentials.anthropic.claude_code_version`. `claude_code_version_too_old` 응답이면 요구 버전을 채택하고 요청당 1회 재시도 |
| 모델 폴백 | `FallbackModel` 체인 | 구현됨(`models.fallback`). 모델 ID 는 설정값 `provider:model` 이고 코드에 하드코딩하지 않는다(미설정이면 세션 생성이 `model_not_configured`, D5) |
| steer (S-8) | 데몬 `MessageQueue` 가 도구 호출 경계에서 주입하도록 hook | **네이티브 `run.enqueue`/`ctx.enqueue` 금지.** `DeferredToolRequests` 로 끝나야 할 런을 덮어써 일반 문자열 출력으로 바꾸고(`DEFERRED BOUNDARY asap: output: str model-requests: 2`) 추가 모델 요청을 일으킨다. 대신 사이드카가 steer 버퍼를 소유하고 `SessionHooks.before_model_request` 에서 `UserPromptPart` 요청으로 주입한다. 런 종료 시 남은 메시지는 새 런, 보류 종료 시에는 재개 런에서 주입, abort 는 `run.cancel()`. 큐·영속화는 계속 데몬 소유 |
| 보류 도구 | `CallDeferred`, `ApprovalRequired`, `DeferredToolResults` | 구현됨. 프록시가 outcome `deferred/kind:"approval"` → `ApprovalRequired`, `external` → `CallDeferred`(metadata 에 `ticket_id`), `denied`/`error` → `ModelRetry`(기본 3회, `agent.tool_retries`)로 바꾼다. 사람 단계는 항상 보류(D2, 90초 블록 fast path 없음), 보류 호출이 모두 해결되면 `DeferredToolResults` 로 새 런을 시작하며 재개는 최대 1회(`stale_ticket`). 열린 티켓이 있으면 큐 메시지를 사이드카로 보내지 않는다(D4). live: 실제 데몬+사이드카에서 `ask_user` 보류 → CLI resolve → 재개(모델 요청 2회), 최종 응답 `b` |
| 툴 노출 게이트 | (계획 없음) | 추가됨. `open.tools` 로 받은 명세만 노출한다. 사이드카는 모델 요청 직전에 선언된 함수 툴 이름 집합이 기대 집합과 다르면 `ToolGateError`, 데몬은 `ready.tools` 가 `open.tools` 와 정확히 같지 않으면 세션 시작을 거부한다(`tool_gate`) |
| 서브에이전트 | harness `Subagents` 사용 또는 자체 구현 | **ADR-020 으로 해결.** harness `SubAgents` 는 서브에이전트의 `CallDeferred`/`ApprovalRequired` 결과를 문자열로 바꿔 부모 `DeferredToolRequests` 로 올리지 못하므로(S-9 `subagents.deferred parent_deferred=false`) 채택하지 않는다. 대신 사이드카의 자체 어댑터(`runtime/subagents.py`)가 데몬 툴 `delegate` 뒤에서 서브에이전트를 실행한다. 서브에이전트의 승인 요청과 질의는 `origin.subagent_id` 를 붙여 부모 세션에 티켓을 열고, 부모의 `delegate` 호출은 서브에이전트가 끝날 때까지 보류된다. 취소·steer·복구 규칙은 `docs/specs/sidecar-protocol.md` §10 에 있다. `delegate` 는 데몬 툴이므로 툴 노출 게이트는 그대로다. 프로토콜의 `tool_call.subagent`, `event subagent.*` 도 이 어댑터가 보낸다 |
| harness capability | 평가 후 채택(S-9) | §E. 5개를 툴 0개 노출 조건으로 연결 |
| 테스트 | fake broker + fake provider, `pytest -m live` | 가상 broker 와 `MockTransport`, `FunctionModel`, `OPENSIDE_AGENT_SCRIPT`(스크립트 모델, 프로토콜 §9)로 hermetic 하게 돌린다. live 는 `@pytest.mark.live` 로 분리하고 `OPENSIDE_LIVE_MODEL` 을 지정할 때만 실행한다. 이번 goal 의 live 호출은 4회(Claude 1회): Codex `pytest -m live` 통과, Claude `pytest -m live` 통과(429 없음), 데몬+사이드카 `ask_user` 왕복 |

compaction 임계값(`compaction_max_messages` 기본 100, `compaction_keep_messages` 기본 20)과 `tool_retries` 는 `open.config.agent` 로 바꿀 수 있지만, 데몬은 현재 `config.agent` 에 빈 객체를 보내므로(`crates/openside-agent-sidecar/src/connection.rs`) 기본값이 쓰인다.

## D. 알려진 위험
- Claude 구독 셰이핑은 Anthropic 이 바꾸면 깨진다(위 429). 완화: 셰이핑 모듈 격리, `*_too_old` 자동 적응, API 키 경로(`env`/`command`) 병행, `openside doctor` 점검. 실제 서비스에 대한 지속적 호환은 `unverified`.
- 소비자 구독을 서드파티 하네스에서 쓰는 약관 위험(R-6)은 omp/pi-ai 도 같다. 사용자 책임으로 문서화한다.
- `omp-broker` 소스를 고르고 토큰이 있는데 broker 가 꺼져 있으면 모든 호출이 실패한다(fail-closed, 의도). 로컬 소스로 폴백하지 않는다.
- Pydantic AI 의 Codex provider 는 sentinel refresh token 을 그대로 OAuth 에 제출할 수 있다는 위험이 S-16 에서 확인됐고 `openai_client` 주입으로 제거했다(§C).
- harness 는 0.x 라 API 가 바뀔 수 있다. S-9 는 설치된 소스 기준이며 8개 capability 만 실행했고 나머지 18개는 소스 조사다(§E).
- steer 는 Pydantic AI 네이티브 큐를 쓰지 않으므로 사이드카의 `SessionHooks` 구현이 업스트림 `before_model_request` 계약에 의존한다.

## E. harness 채택표 (S-9 권고 + 최종 연결 상태)
`docs/spikes/S-9-harness-maturity.md` 의 채택표(26행) 그대로이며, 마지막 열 "최종" 만 이 저장소가 실제로 연결한 것(`agent/src/openside_agent/capabilities.py`, ultrabrowse 전용은 `research.py`)이다. S-9 의 `adopt` 는 스파이크의 **권고**이고 제품 구현을 뜻하지 않으며, 추가 capability 의 실행·데몬 adapter 는 `unverified` 다. "툴은 데몬 것만" 계약은 두 겹으로 지킨다. `adopted_capabilities()` 는 툴셋을 노출하는 capability(`get_toolset()` 이 `None` 이 아님)를 기동 시 `ValueError` 로 거부한다. 또 `Session.open` 은 모든 툴셋이 실제로 노출하는 도구 이름을 데몬 목록과 비교해, 다르면 세션을 거부한다(`tool_gate`). `Planning(tools=())` 처럼 빈 툴셋만 가진 capability 는 두 번째 검사로 확인한다.

| capability | adopt / not adopt / later | 이유 | 설정 참고 | 최종 |
|---|---|---|---|---|
| subagents | later | 공용 프록시는 동작하지만 자식 deferred/approval 결과가 문자열로 변환됨 | explicit proxy binding; deprecated inherit_tools 금지; 자식 conversation/티켓 소유권과 부모 suspend adapter, 승인된 delegate 도구 필요 | 미연결 — harness capability 로는 연결하지 않고 사이드카 자체 어댑터(ADR-020)로 대체. 자식 deferred 가 부모로 올라오지 않기 때문 |
| skills | later | instruction 로딩은 동작하나 discovery와 load_capability의 도구 소유권 조정 필요 | directories와 include/exclude; 신뢰된 read-only/daemon workspace; PyYAML; 번들 자동 실행 금지 | 미연결 |
| compaction | adopt | 도구 없는 history 훅 동작, 세션 모델로 요약 가능 | model=None; threshold/window 설정; summarization_capabilities와 streaming 설정; 압축 후 Conversation 저장 | 연결: `SummarizingCompaction(model=None, max_messages=100, keep_messages=20)` (임계값 키 `compaction_max_messages`/`compaction_keep_messages`, 데몬은 기본값 사용) |
| memory | later | 기본은 프로세스 메모리이고 내장 도구가 store에 직접 씀 | daemon-backed MemoryStore/도구 schema; session/agent namespace; injection_errors='raise'; 비밀 저장 금지 | 미연결 |
| ask_user | later | stdin 없는 deferred 재개 동작, 기본 도구 이름·schema는 데몬과 다름 | daemon 경로에 AskUserRequest/ask_user_result 재사용; answerer=None; DeferredToolRequests output; secret_input에 사용 금지 | 미연결 — 데몬 `ask_user` 툴(`info` 티켓)로 대체 |
| step_persistence | later | 저장 이중화 및 Conversation metadata 누락 | daemon-backed StepStore, Rust 기준 저장소; session conversation_id와 fresh run_id; ConversationTypeAdapter 대체 금지 | 미연결 — 대화 스냅샷은 사이드카 `openside-agent-state/1` 을 데몬이 보관 |
| tool_output_limits | later | Truncate도 read_tool_result를 추가하고 기본 Spill은 파일에 저장 | 현재 daemon limits 사용; 향후 도구 비노출/daemon overflow store; redaction 후 spill; Truncate-only로는 부족 | 미연결 — 데몬 32 KiB 출력 제한(docs/09) |
| prompt_injection_defender | adopt | 추가 도구 없이 합성 위험 결과 차단 | block_high_risk=True, semantic_detection=False; 외부 결과 도구 선택; raw 로그 금지; stackone-defender | 연결: `PromptInjectionDefender(block_high_risk=True, semantic_detection=False)`; 의존성 `pydantic-ai-harness[prompt-injection-defender]` |
| guardrails | adopt | 실행 도구 없이 입력·출력·결과 검사 가능 | nonparallel InputGuardrail; 결정적 guard; Rust 정책 우선; runtime unverified | 연결: 모든 프로필에 `InputGuardrail(redact_secrets, parallel=False)`(사용자 프롬프트의 API 키·개인 키를 모델 전송 전에 가림). ultrabrowse 에 `OutputGuardrail`(인용 검사, [`specs/ultrabrowse.md`](specs/ultrabrowse.md)) |
| repair_tool_arguments | adopt | 일반 schema 검증 전 도구 없는 JSON 복구 | 최종 검증·정책은 daemon; json-repair; heuristic 의미/runtime unverified | 연결: `RepairToolArguments()` |
| overflowing_tool_output | not adopt | deprecated alias | 호환 조건 충족 시 canonical tool_output_limits 사용 | 미연결 |
| system_reminders | adopt | static/dynamic callback은 실행 도구를 추가하지 않음 | static/GoalReanchor; 기본 LLMReminder 금지; 비밀 없는 내용; runtime unverified | 연결: `SystemReminders(dynamic_reminders=(current_request_anchor,))`. 한 런에서 모델 요청 5번마다 지금 처리 중인 요청을 다시 알린다. `GoalReanchor` 는 세션의 첫 메시지를 되풀이하는데, 오래 쓰는 세션에서는 대개 이미 끝난 작업이라 쓰지 않았다. 모델 추가 호출 없음 |
| planning | adopt | injection-only 모드에서 도구 비노출 가능 | tools=[], daemon 제공/read-only PlanStore 상태, inject=True; custom store/runtime unverified | 연결(ultrabrowse): `Planning(tools=(), inject=True)`. 계획 출처는 모델이 쓴 `Plan:` 블록이며, `PlanFirst` 가 매 요청 기록에서 다시 읽어 저장소에 넣는다. 계획 전 `browser_*` 호출은 `ModelRetry` 로 거절 |
| bubblewrap_sandbox | not adopt | sidecar command wrapper는 daemon sandbox가 아님 | Rust가 sandbox와 fail-closed 동작 소유 | 미연결 — 데몬 `bash` 가 직접 bwrap 을 실행(docs/09) |
| shell | not adopt | 직접 명령 도구가 daemon 정책을 우회 | daemon bash proxy만 사용; 도구 전부 끄면 중복 기능 | 미연결 — 데몬 `bash` |
| filesystem | not adopt | read-only도 daemon 파일 정책을 우회 | daemon read/write/edit proxy; 사용자 파일에 local harness workspace 부여 금지 | 미연결 — 데몬 `read`/`write`/`edit` |
| playwright | not adopt | 독립 브라우저 도구가 daemon controller/Vault 우회 | daemon browser_*만 사용; CDP 연결로 정책 우회가 해소되지 않음 | 미연결 — 데몬 `browser_*` |
| browser_use | not adopt | 자율 브라우저 도구가 정책을 우회하고 모델 호출 추가 | sidecar sensitive_data 금지; daemon browser_*만 사용 | 미연결 — 데몬 `browser_*` |
| code_mode | later | 프록시 래핑은 가능하나 run_code와 수명주기 검증 필요 | os_access/mount 없음; resource bounds; speculative/eager 효과 금지; deferred/restart/policy routing unverified | 미연결 |
| spend | adopt | 기본 도구 비노출, 세션 사용량·예산 보조 기능 | expose_tools=False; hard USD에 명시적 price와 on_unpriced='raise'; daemon 집계 우선; runtime unverified | 연결: `SpendLimits(expose_tools=False)` (예산 미설정, 관측 전용) |
| cache_stability | not adopt | deprecated alias | warn_on_cache_busts 사용 | 미연결 |
| warn_on_cache_busts | adopt | 추가 도구 없는 진단 | canonical module; threshold 명시; 재시작 시 관측 상태 소실; runtime unverified | 연결: `WarnOnCacheBusts()` (임계값 기본값) |
| background_tools | later | deferred/approval이 실패로 바뀌고 run 종료 시 작업 취소 | 티켓 가능 도구 background 금지; Rust 작업 소유권·복구 먼저; runtime unverified | 미연결 |
| conversation_search | later | 검색 도구와 history 소유권·격리 경계 필요 | daemon-backed HistorySource; conversation scope 명시; daemon 명세; tenant 간 검색 금지 | 미연결 |
| context | not adopt | deprecated module, 기본 discovery를 신뢰된 데몬 context로 볼 수 없음 | daemon instruction 사용; 향후 trusted workspace/비노출 inventory의 RepoContext 검토 | 미연결 |
| tool_call_judge | later | 추가 모델·비용·정보 전송, 중복 정책 판정 | 명시적 model; on_uncertain='block'; include_conversation=False; Rust 판정을 덮어쓰지 않는 보조 기능만 | 미연결 |

연결한 capability 는 모두 툴을 노출하지 않는다. `PromptInjectionDefender` 는 로컬 패턴 tier 만 쓰고(ML extra·모델 가중치 없음), `SummarizingCompaction` 의 요약은 세션 모델로 한 번 더 호출하는 별도 모델 요청이다. `guardrails`·`system_reminders`·`planning` 은 위 표대로 연결했다. 테스트: `agent/tests/test_citations.py`(ultrabrowse 계획·인용 재시도·추론 수준), `agent/tests/test_capabilities.py`.

## F. 기각한 안: omp auth-gateway 경유 (기록)
초기에는 Pydantic AI 가 omp `auth-gateway`(stdio 또는 `serve`)를 OpenAI 호환 엔드포인트로 보고 호출하는 안을 검토했다. Pydantic AI 단독으로는 구독 인증이 부족하기 때문이다.

| 구독 | Pydantic AI 자체 지원 | 비고 |
| --- | --- | --- |
| ChatGPT/Codex | 있음(문서): `openai-codex:` provider, `~/.codex/auth.json` 또는 자체 OAuth 흐름 | 계정 1개. broker 의 멀티 계정·사용량 기반 로테이션 없음 |
| GitHub Copilot | 있음(문서): device login, OAuth 토큰 | 마찬가지로 1계정 |
| Claude Pro/Max | 없음(anthropic 문서에 OAuth 구독 항목 없음) | pi-ai 는 구독 토큰 사용 시 Claude Code 식별 헤더·베타 플래그·지문을 맞춘다 |
| 멀티 계정, 사용량 reserve/priority, 429 block, 세션별 계정 핀/풀 | 없음 | omp `AuthStorage` 가 소유 |

검증한 것(저장소에 없는 임시 스파이크): `OpenAIChatModel` + 커스텀 httpx2 transport → `omp auth-gateway stdio` 로 구독 인증 실제 호출 성공(`@smol` 역할, `anthropic/claude-haiku-5-5` 모두 `pong`), 그리고 `CallDeferred` → `DeferredToolRequests` 종료 → conversation 직렬화·복원 → `DeferredToolResults` 재개(`docs/specs/human-step.md` 의 티켓 모델이 프레임워크 기본 기능으로 성립).

기각 이유: stdio 모드는 `stream: true` 의 SSE 를 스트림이 끝난 뒤 문자열 하나로 돌려줘 실시간 토큰 스트리밍(UI)이 안 되고, `serve` 는 broker URL·게이트웨이 토큰 파일을 추가로 요구하며 세션 단위 계정 핀 유지 여부도 확인하지 못했다(`unverified`). 게이트웨이는 로컬 `models.yml` 오버라이드를 무시하고 broker 카탈로그만 쓴다(문서). 사이드카가 broker 스냅샷을 직접 읽는 §A 로 이 제약이 사라졌다.

omp 런타임 대비 잃은 것: 검증된 steer/follow-up 큐와 RPC 계약 테스트 자산, 서브에이전트 구독 프로토콜(사이드카 자체 어댑터(ADR-020)로 대체), compaction 튜닝, 공식 Rust SDK. 얻은 것: 보류 도구의 프로토콜 수준 지원, Python 생태계(평가, durable execution, 관측, MCP, guardrails), 에이전트 로직을 코드로 직접 제어.
