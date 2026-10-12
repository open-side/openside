# S-9 — pydantic-ai-harness 성숙도

> 이 문서에 나오는 `spikes/` 아래 임시 스크립트와 내부 개발 기록(`docs/GOAL.md`, `docs/PROGRESS.md`, `docs/archive/`)은 공개 저장소에 포함되어 있지 않으며, 실행 명령과 출력 결과는 작업 과정을 확인하는 기록용으로 남겨 둔다.

## 범위와 근거

설치된 `pydantic-ai-slim 2.55.0`, `pydantic-ai-harness 0.55.0`을 기준으로 조사했다. 최신 온라인 문서보다 설치 소스를 우선한다. 아래 소스 표기의 기준 경로는 다음과 같다.

- `H/` = `agent/.venv/lib/python3.13/site-packages/pydantic_ai_harness/`
- `P/` = `agent/.venv/lib/python3.13/site-packages/pydantic_ai/`

`spikes/sidecar/s9_harness.py`는 핵심 capability 8개를 각각 `Agent(FunctionModel(...), capabilities=[...])`에 연결한다. 실제 LLM·broker·브라우저·사용자 자격증명은 사용하지 않는다. 파일 저장은 임시 디렉터리에서만 수행하며 종료 시 삭제한다. 데몬 프록시는 공용 `FunctionToolset` callback으로 모사했다. 실제 Rust reverse RPC 연결은 `unverified`다. `agent/src`, 제품 의존성 선언, Rust 코드는 변경하지 않았다.

## 필요 의존성

초기 실행에서는 `Skills` import가 실패했다.

```text
ImportError: PyYAML is required to load Agent Skills.
```

의존성 오류를 분리한 실행에서는 `PromptInjectionDefender`도 `stackone-defender`가 없어 import에 실패했다. Main의 허가로 스파이크용 venv에만 다음 extras를 설치했다. `agent/pyproject.toml`, `uv.lock`은 변경하지 않았다.

```sh
uv pip install --python agent/.venv/bin/python --index-url https://pypi.org/simple 'pydantic-ai-harness[skills,prompt-injection-defender]==0.55.0'
```

실제 출력:

```text
Using Python 3.13.15 environment at: agent/.venv
Resolved 24 packages in 699ms
Installed 2 packages in 9ms
 + pyyaml==6.0.3
 + stackone-defender==0.7.4
```

추가한 extras는 `skills`, `prompt-injection-defender`뿐이다. ML extra·ONNX Runtime·모델 가중치 다운로드는 요청하지 않았다. 제품에서 채택하는 extras는 별도로 의존성 선언과 lock에 반영해야 한다. 현재 스파이크의 추가 설치는 lock으로 재현되지 않는다.

## 핵심 capability 8개

### 1. subagents

**공개 API·설정.** `SubAgents`, `SubAgent`, `ModelOption`, `SubAgentToolset`, 별도 작업 관리자인 `DelegationTasks`가 있다. 주요 `SubAgents` 생성자 인자는 다음과 같다.

```text
agents=(), models={}, agent_folders=None, agent_overrides={}, tool_resolver=None,
forward_usage=True, inherit_tools=False, shared_capabilities=(),
event_stream_handler=None, tool_name='delegate_task', tool_retries=2,
contain_errors=False, workspace=None, include_self=False, max_depth=3,
id='sub_agents'
```

`SubAgent(agent, name=None, description=None, models=None, usage_limits=None, timeout_seconds=None, max_calls=None, on_failure=None, contain_errors=None, *, read_only=False)`로 자식별 제한을 정한다. `inherit_tools=True`는 deprecated다. 근거: `H/subagents/_capability.py:72-275`, `H/subagents/_toolset.py:109-179`.

**훅·모델·저장.** `for_run`, `before_run`, `wrap_run`, `get_instructions`, `get_toolset`을 사용한다. 자식은 별도 `Agent.run`에서 모델을 호출한다. `deps`와 기본 공용 `usage`를 전달하지만 부모 capability의 훅은 자동 상속하지 않는다. `shared_capabilities`로 지정하거나 자식 Agent에 직접 연결해야 한다. 기본 영속 저장소는 없고, `agent_folders`도 기본 `None`이다. 디스크 정의를 쓰면 지정된 workspace에서 읽는다. 자식 모델은 직접 설정하거나 모델 메뉴로 선택하고, 모델이 없는 자식은 부모 모델을 사용한다. 별도 외부 의존성은 core·AnyIO 수준이다. 근거: `H/subagents/_capability.py:161-275,420-489`, `H/subagents/_toolset.py:493-552`.

**실행 결과.** 같은 프록시 toolset을 부모와 자식에 명시적으로 연결하면 자식 호출도 같은 handler에 도착한다. `daemon_probe` 호출은 각 사례에서 1회였다. 부모 도구를 그대로 상속하는 deprecated 경로 대신 이 패턴을 사용할 수 있다. deprecated 상속 경로는 capability 소유 toolset을 제외하므로 승인·감사 훅까지 상속한다고 보면 안 된다 (`H/subagents/_toolset.py:316-341`).

**중요한 제약: 자식 deferred는 부모 deferred로 전달되지 않는다.** 자식의 `output_type=[str, DeferredToolRequests]`에서 `CallDeferred`와 `ApprovalRequired`는 각각 자식 run의 `DeferredToolRequests` 결과가 된다. 그러나 `delegate_task`는 성공한 자식 결과를 `str(result.output)`으로 바꾼다 (`H/subagents/_toolset.py:619-671`). 실제 부모는 두 사례 모두 `parent_deferred=false`, 최종 `str` 결과였다. 자식의 요청·승인 정보는 일반 도구 결과 문자열에 들어갔다.

`_ALWAYS_PROPAGATE`에 두 예외가 있어도 이미 자식 core가 처리한 **결과 객체**에는 적용되지 않는다 (`H/subagents/_toolset.py:98-107,667-671`). 따라서 예외 전파와 deferred 결과 전달은 구별해야 한다. 자식 conversation·티켓 소유권, 자식 run을 포함한 call ID 상관관계, 부모 중단·재개 adapter 없이는 채택할 수 없다. 이 adapter와 자식 재개·재시작 복구는 `unverified`다.

### 2. skills

**공개 API·설정.** `Skills(directories, *, include=None, exclude=None, workspace=None)`를 제공한다. `include`와 `exclude`는 동시에 지정할 수 없다. `for_run`에서 라이브러리를 읽어 instruction 전용 deferred capability로 바꾼다. core가 `load_capability(id=...)`를 노출하며, 해당 skill의 `get_instructions`로 본문을 제공한다. 추가 모델 호출은 없다. `PyYAML`이 필수 import 의존성이다 (`H/skills/_capability.py:49-131,179-201`, `H/skills/_loader.py:18-25`).

**저장 위치.** 라이브러리 경로와 workspace를 호출자가 정한다. run workspace 또는 별도 `workspace=`가 필요하며, 고정 홈 경로는 없다. 바로 아래의 선택된 skill 디렉터리에서 `SKILL.md`를 읽는다. 번들 파일은 자동으로 읽거나 실행하지 않는다. 자체 쓰기·영속 저장소도 없다.

**실행 결과.** 임시 workspace의 `sample`을 `include=['sample']`로 선택하고 `load_capability`로 불러오니 instruction이 전달됐다. 별도 workspace 없이 실행하면 `UserError`가 발생했다. skill 자체는 shell·filesystem 도구를 추가하지 않지만 **core의 `load_capability`도 추가 도구 이름**이다. 데몬 제공 도구만 노출한다는 계약과의 조정이 필요하다. 신뢰된 읽기 전용 설정 또는 데몬 기반 workspace 연결은 `unverified`다.

### 3. compaction

**공개 API.** `SummarizingCompaction`, `SlidingWindowCompaction`, `TieredCompaction`, `ClearToolResults`, `ClampOversizedMessages`, `DeduplicateFileReads`, `FallbackCompaction`, `ReportContextUsage`, `WarnNearLimits`, `compact_now`, pin 관련 helper가 있다 (`H/compaction/__init__.py:8-30`).

`SummarizingCompaction`의 설정은 다음과 같다.

```text
model=None, model_settings=None, event_stream_handler=None,
summarization_capabilities=(), max_messages=None, max_tokens=None,
max_fraction=None, context_window=None, fallback_context_window=DEFAULT_CONTEXT_WINDOW,
keep_messages=20, keep_tokens=None, summary_prompt=..., instructions=...,
tokenizer=None, preserve_first_user_message=True, incremental=True,
bridge_prefix=False, keep_user_messages=False, keep_user_messages_max_chars=20000,
tool_return_max_chars=500, receipts=False, id='summarizing_compaction'
```

최소 하나의 임계값을 지정해야 한다. `max_tokens`와 `max_fraction`은 배타적이다 (`H/compaction/_summarizing_compaction.py:300-482`).

**훅·모델·저장.** `before_model_request`에서 history와 요청 메시지를 압축한다. 요약은 내부 Agent의 **별도 모델 호출**이다. `model=None`이면 해당 요청의 `ModelRequestContext.model`, 보통 세션 모델을 사용한다. 명시적 `model`로 고정할 수도 있다. 요약은 conversation history에 남고 자체 파일 저장소나 실행 도구는 없다. 내부 요약 Agent에는 외부 Agent의 capability가 자동 적용되지 않으므로 필요한 guardrail은 `summarization_capabilities`에 추가해야 한다 (`H/compaction/_summarizing_compaction.py:665-760`).

`SlidingWindowCompaction(max_messages/max_tokens/max_fraction, keep_messages=40, keep_tokens=None, tokenizer=None, preserve_first_user_message=True, receipts=False)`도 `before_model_request`를 쓰지만 모델 호출 없이 오래된 메시지를 제거한다 (`H/compaction/_sliding_window_compaction.py:39-114`). context window 해석은 core 모델 메타데이터를 이용한다.

**실행 결과.** `model=None`에서 같은 FunctionModel이 `summary`, `session` 순으로 호출됐다. `requests=2`이며 요약이 history에 남았다. 압축 직전 9개 메시지에서 최종 응답 포함 5개가 됐다. streaming 전용 요약 endpoint에는 `event_stream_handler=drain_summary_events` 경로가 있으나 실제 provider·자격증명 갱신·stream-only endpoint 조합은 `unverified`다 (`H/compaction/_summarizing_compaction.py:286-297,346-364`).

### 4. memory

**공개 API·설정.** `Memory`, `MemoryStore`, `InMemoryStore`, `FileStore`, `SqliteMemoryStore`, `PostgresMemoryStore`를 제공한다.

```text
store=InMemoryStore(), store_resolver=None, agent_name='main', namespace='',
inject_memory=True, heading='', max_tokens=2000, max_lines=200,
max_memory_size=65536, max_search_results=10, max_search_result_chars=4000,
max_search_files=1000, guidance=None, injection_errors='ignore', id='memory'
```

**훅·모델·저장.** `for_run`, `before_run`, `before_model_request`, `get_instructions`, `get_toolset`을 사용한다. `MEMORY.md`는 신뢰된 system instruction이 아니라 **user-role context**로 넣는다. `write_memory`, `read_memory`, `delete_memory`, `search_memory`가 store를 직접 호출한다. 별도 모델 호출은 없다. 기본 store는 프로세스 수명 동안만 유지된다 (`H/memory/_capability.py:51-175,221-305`).

`FileStore(directory, *, workspace=None)`는 경로·workspace를 지정할 수 있다. namespace/agent별 파일과 `.memory-operations.json` receipt를 저장하며 고정 홈 경로는 없다. 한 디렉터리에 여러 store·프로세스가 쓰면 갱신을 잃을 수 있다. 동시 writer에는 SQLite/Postgres를 사용해야 한다 (`H/memory/_store.py:477-515`). `SqliteMemoryStore(database=... 또는 connection=...)`는 stdlib `sqlite3`를 쓰며 둘 중 하나만 받는다 (`H/memory/_store.py:815-865`). `PostgresMemoryStore(pool, table='agent_memory')`는 호출자가 제공하는 asyncpg 호환 pool을 사용한다 (`H/memory/_postgres.py:1-64`).

**실행 결과.** 임시 `FileStore('memory', workspace=...)`, `namespace='session'`에서 `memory/session/main/MEMORY.md`와 receipt를 생성했다. 같은 실행과 다음 실행의 memory 주입이 모두 확인됐다. 그러나 이 직접 파일 쓰기를 제품에 그대로 연결하면 데몬 정책을 우회한다. 데몬 기반 `MemoryStore`와 데몬 제공 도구 명세 연결이 필요하며 공개 `disable-tools` 인자는 없다. 저장 실패는 `injection_errors='raise'`로 명시적으로 전파할 수 있다. 비밀 값은 memory·receipt 인자에 남겨서는 안 된다. 원격 store와 secret canary는 `unverified`다.

### 5. ask_user

**공개 API·설정.** `AskUser(*, answerer, timeout=None)`, `AskUserToolset`, `AskUserRequest`, `AskUserResponse`, `AskUserAnswer`, `Question`, `QuestionOption`, `ask_user_result`를 제공한다. `get_instructions`, `get_toolset`만 사용한다. 추가 모델 호출·저장소·stdin 의존성은 없다 (`H/ask_user/_capability.py:25-75`).

**입력 주입과 deferred.** `answerer`는 async `Answerer(request) -> AskUserResponse`를 호출자가 넣는다. `answerer=None`이면 요청 이벤트 후 `CallDeferred`를 발생시킨다. Agent의 output type에 `DeferredToolRequests`가 있어야 한다. timeout은 양수 또는 `None`이고, deferred에는 timeout을 지정할 수 없다 (`H/ask_user/_toolset.py:24-75`).

기본 도구 이름은 데몬의 `ask_user`가 아니라 `ask_user_question`이다. 1~10개 질문, 질문당 2~6개 선택지, 중복 없는 header·label을 요구한다. `AskUserRequest.from_tool_call`과 `ask_user_result`로 응답을 검증한다 (`H/ask_user/_types.py:14-23,61-109,120-163`).

**실행 결과.** 주입한 answerer에서 `Read`를 선택했다. 별도 deferred 사례는 `ConversationTypeAdapter` 왕복 후 같은 call ID에 `DeferredToolResults.calls`를 넣어 `done`으로 재개했다. stdin은 사용하지 않았다. 티켓은 앱이 저장해야 하며 일반 답변은 history에 들어간다. `secret_input`에 이 경로를 쓰면 안 된다. 비밀은 Rust가 직접 입력하고 사이드카에는 상태만 전달해야 한다. 기본 도구 이름·schema 소유권 조정 또는 helper만 데몬 경로에 재사용하는 연결은 `unverified`다.

### 6. step_persistence

**공개 API·설정.** `StepPersistence`, `StepStore`, `InMemoryStepStore`, `FileStepStore`, `SqliteStepStore`, `PostgresStepStore`, `MongoStepStore`, `ContinuableSnapshot`, `StepEvent`, `ToolEffectRecord`, `continue_run`, `fork_run`이 있다.

```text
store=InMemoryStepStore(), agent_name=None, run_id=None, parent_run_id=None,
metadata={}, capture_frontier=False, id='step_persistence'
```

`from_spec`은 `backend=memory|file|sqlite`, `directory`/`database`, `max_snapshots_per_run`을 받는다. `conversation_id`는 세션 묶음, `run_id`는 실행별 식별자다. 같은 명시적 `run_id`를 다시 쓰면 오류가 난다 (`H/step_persistence/_capability.py:50-182`).

**훅·모델·저장.** `for_run`, `wrap_run`, run 시작·완료·오류, model request 시작·완료·오류, tool 실행 시작·완료·오류, `after_node_run`을 기록한다 (`H/step_persistence/_capability.py:423-726`). 추가 도구나 자체 모델 호출은 없다. `capture_frontier`는 입력·모델 도구 호출 경계 snapshot도 추가한다.

기본 store는 메모리다. `FileStepStore(directory, *, media_store='auto', media_threshold_bytes=65536, max_snapshots_per_run=None)`는 다음 위치에 저장한다.

```text
<directory>/<run_id>/run.json
<directory>/<run_id>/events.jsonl
<directory>/<run_id>/tool_effects.jsonl
<directory>/<run_id>/snapshot-keys.jsonl
<directory>/<run_id>/snapshots/<seq>.json
<directory>/media/<sha256>.bin  # automatic large-content externalization
```

경로는 설정 가능하다. File/SQLite는 stdlib 기반이며, PostgreSQL은 자체 driver import 없이 호출자 소유 asyncpg 호환 pool을 사용한다. MongoDB store는 `pymongo.AsyncMongoClient`가 필요하다 (`H/step_persistence/_store.py:467-520`, `H/step_persistence/_postgres.py:1-11`, `H/step_persistence/_mongo.py:32-34,85-98`).

**실행 결과와 conversation 충돌.** `capture_frontier=True`, `media_store=None`에서 snapshot 6개를 저장했다. 마지막 complete snapshot의 메시지 4개는 `ConversationTypeAdapter`로 복원한 메시지와 같았다. 명시적 run ID 재사용은 `ValueError`였다. 완료된 실행의 메시지 직렬화에는 충돌이 관찰되지 않았다.

하지만 `ContinuableSnapshot`은 **완전한 Conversation이 아니다.** 메시지와 식별자는 저장하지만 누적 `usage`, `deferred_tool_requests`는 없다. `continue_run`도 메시지만 반환한다 (`H/step_persistence/_types.py:93-105`, `H/step_persistence/_helpers.py:58-80`). 반면 Conversation은 둘 다 포함한다 (`P/conversation.py:75-100`). 따라서 티켓·Conversation 직렬화를 대체할 수 없다. 로컬 step 저장소와 Rust 세션 저장소를 함께 쓰면 저장 권한·복구 기준이 이중화된다. 채택하려면 데몬 기반 `StepStore`를 사용하고 Rust를 기준 저장소로 유지해야 한다. effect ledger만으로 외부 작업의 exactly-once 실행이 보장되지는 않는다. crash·재시작·deferred snapshot 복구는 `unverified`다.

### 7. tool_output_limits

**공개 API·설정.** `ToolOutputLimits`, `Band`, `Passthrough`, `Truncate`, `Spill`, `Summarize`, `OverflowStore`, `WorkspaceStore`, `LocalFileStore`, `TruncationStrategy`를 제공한다.

```text
bands=default Spill(then=Truncate()), per_tool={}, tool_filter='all',
over_tokens=False, tokenizer=None, store=None, strip_ansi=False,
summary_prompt=..., serializer=None, id='tool_output_limits'
Band(over, action)
Truncate(strategy=head_tail, max_chars=4000, then=None, *, keep_tail_lines=0)
Spill(preview_chars=1000, then=None)
Summarize(model=None, summarize=None, then=None)
```

**훅·모델·저장.** `before_run`, `after_tool_execute`, `get_toolset`을 사용한다. Truncate/Spill은 추가 모델 호출이 없고, Summarize는 기본 세션 모델로 별도 Agent를 호출한다. retry·실행 예외는 이 출력 훅을 거치지 않는다 (`H/tool_output_limits/_capability.py:96-218,501-538`, `H/tool_output_limits/_bands.py:31-101`).

기본 spill은 workspace의 `.pydantic-ai-harness/tool-output`에 저장하며 자동 정리하지 않는다. `LocalFileStore(base_dir=None)`는 기본 사용자별 임시 경로 또는 명시한 `base_dir`을 사용한다. 별도 외부 의존성은 custom backend를 선택할 때 생긴다 (`H/tool_output_limits/_store.py:74-84,165-191`).

**실행 결과.** `Band(over=50, action=Truncate(max_chars=40))`로 200자를 40자로 줄였다. 줄인 결과가 history에 남았다. **Truncate만 설정해도 `read_tool_result`가 노출된다.** 공개 `expose_tools=False` 설정은 없다 (`H/tool_output_limits/_capability.py:227-258`). 기본 spill 설정으로 workspace 없이 실행하면 `UserError`다.

현재 그대로는 데몬 제공 도구만 노출한다는 계약에 맞지 않는다. 검색 도구 비노출/데몬 도구 연결과 데몬 기반 overflow store가 필요하다. 우선 데몬의 출력 제한을 사용해야 한다. 비밀 redaction은 spill·preview·요약 **이전**에 데몬에서 끝내야 한다. 표시만 잘라도 전체 spill에는 비밀이 남을 수 있다. binary·spill·custom summarizer 경로는 `unverified`다.

### 8. prompt_injection_defender

**공개 API·설정.** `PromptInjectionDefender(defense=None, *, block_high_risk=None, semantic_detection=False, tool_filter='all', on_detection=None, blocked_message=...)`를 제공한다. `stackone-defender`가 필요하다. 기본 `block_high_risk=None`은 보고만 하고 막지 않으므로 `True`를 명시해야 한다. semantic tier는 ML extra·ONNX 기반 로컬 classifier를 사용하는 선택 기능이다. 기본 패턴 tier에는 별도 LLM 호출이 없다. custom defense의 Tier 3는 `unverified`다 (`H/prompt_injection_defender/_capability.py:18-24,79-191`).

**훅·저장.** ordering은 `innermost`다. `before_run`에서 선택적 ML을 준비하고, `after_tool_execute`에서 모델에 보이는 반환값·content를 검사한다. 위험한 결과는 전체를 안내문으로 대체하고 진단 metadata 및 `on_detection`을 제공한다. 도구·자체 저장소는 없다. provider-native tool은 이 훅을 거치지 않는다 (`H/prompt_injection_defender/_capability.py:184-240`).

**실행 결과.** 합성된 외부 문서 지시문 1개를 local pattern tier로 차단했다. `risk=high`, `allowed=false`였으며 일반 요청→도구→최종 응답에 해당하는 `requests=2`였다. 이 결과는 완전한 보안 경계나 secret redaction을 증명하지 않는다. 사용자 입력·기존 history·native tools·이미 저장된 원문은 별도 보호가 필요하다. raw result·args·진단 원문을 로그에 남겨서는 안 된다. 다른 패턴·오탐·멀티모달·ML 정확도·live 동작은 `unverified`다.

## 나머지 capability 소스 조사

이 표는 소스 확인만 했으며 **모든 추가 capability의 실행은 `unverified`**다. 별도 모델 호출이 없다는 표기는 사용자 callback 자체가 모델을 호출하는 경우를 제외한다.

| capability / 공개 API·주요 설정 | 훅 | 모델 호출·의존성·저장 | 근거 |
|---|---|---|---|
| `guardrails`: `InputGuardrail(guard, parallel=False)`, `OutputGuardrail(guard)`, `ToolGuardrail(guard=None, result_guard=None, tools=None, hidden=())` | input `wrap_model_request`; output `after_output_process`; tool `prepare_tools`, `before_tool_execute`, `wrap_tool_execute`, `after_tool_execute` | 자체 모델 호출·도구·저장소 없음, core/AnyIO. `parallel=True`면 입력 판정 전 요청이 전송될 수 있음. guard가 대체한 인자는 재검증하지 않음 | `H/guardrails/_capability.py:136-207,261-264,327-389`; `H/guardrails/_tool_guardrail.py:67-75,200-239,274-293,324-467` |
| `repair_tool_arguments`: `RepairToolArguments()` | `before_tool_validate` | `json-repair`; heuristic JSON 복구 후 일반 schema 검증. 추가 모델·도구·저장 없음 | `H/repair_tool_arguments/_capability.py:15-40` |
| `overflowing_tool_output`: `OverflowingToolOutput=ToolOutputLimits` | canonical API와 동일 | deprecated alias. 별도 기능·의존성·저장 없음 | `H/overflowing_tool_output/__init__.py:1-25` |
| `system_reminders`: `Reminder(content, interval=1, first_after=None, trigger=None, max_fires=None, tag='system-reminder')`; `SystemReminders(reminders=(), dynamic_reminders=(), cache_ttl='5m')`; `GoalReanchor`; `LLMReminder(model, max_context_messages=10, instructions=...)` | `for_run`, `wrap_model_request` | static/GoalReanchor는 추가 모델 호출 없음; LLMReminder만 지정 모델 호출. core, 일시적인 tail reminder, 자체 파일 저장 없음 | `H/system_reminders/_capability.py:42-66,89-134,158-170,295-343,388-391` |
| `planning`: `Planning(guidance=None, cache_ttl='5m', store=None, store_resolver=None, enable_subtasks=False, inject=True, tools=None, descriptions=None)` | `for_run`, `get_toolset`, `get_instructions`, `wrap_model_request` | 자체 모델 호출 없음. 기본 plan 도구 추가; `tools=[]`로 비노출 설정 가능. 기본은 run별 메모리, 지정 `PlanStore`로 보존; 선택적 DB/Redis backend | `H/planning/_capability.py:52-126,133-201` |
| `bubblewrap_sandbox`: `BubblewrapSandbox(wrapped, network=False, bwrap_args=())`; `BubblewrapWorkspace(wrapped, ...)` | `get_workspace` | 시스템 `bwrap`와 POSIX 명령 실행. 자체 모델·도구·DB 없음. 호스트 프로세스 공간은 공유하며 완전한 process isolation 아님 | `H/bubblewrap_sandbox/_capability.py:13-40`; `H/bubblewrap_sandbox/_workspace.py:205-234` |
| `shell`: `Shell(allowed_commands, denied_commands, denied_operators, default_timeout=30, max_output_chars=50000, max_file_bytes=None, persist_cwd=False, allow_interactive=False, env=None, denied_env_patterns, tools=RUN_SCOPED_TOOL_NAMES)`; `cwd`는 deprecated/ignored | `before_run`, `get_toolset`, `wrap_run` | WorkspaceBackend에서 shell 실행; system shell/AnyIO, 추가 모델 없음. cwd/job은 run 상태, 기본 영속 DB 없음 | `H/shell/_capability.py:55-128,147-160` |
| `filesystem`: `FileSystem(root_dir=None, allowed_patterns, denied_patterns, read_only_patterns, read_only=False, tools=DEFAULT_TOOL_NAMES, content_hashes=True, read/list/search/find limits, max_retries)`; `cwd`는 ignored | `before_run`, `get_toolset` | workspace 파일 읽기·쓰기 도구, 선택적 command-backed 검색. 추가 모델 없음. 파일시스템 자체가 영속 상태. read-only도 데몬 읽기 정책을 우회 | `H/filesystem/_capability.py:36-140,171-178` |
| `playwright`: `PlaywrightBrowser(headless=True, allowed_domains=None, policy=None, block_private_addresses=True, timeouts, chromium_sandbox=True, auto_install_chromium=False, storage_state=None, cdp_url=None)` | `for_agent`, `for_run`, `get_toolset`, `get_instructions`, `wrap_run` | Playwright+Chromium, 직접 브라우저 도구 18개. 추가 모델 없음. run별 browser state와 명시적 storage_state; cookie 등의 비밀 포함 가능 | `H/playwright/_capability.py:77-118,138-234,276-323` |
| `browser_use`: `BrowserUse(llm=None, browser_profile=None, allowed_domains=None, block_ip_addresses=True, headless=None, max_steps=50, use_vision=True, output_schema=None, sensitive_data=None, agent_settings=None, session_scope='call', cdp_url=None, guidance=None, browser_agent=None)` | `get_toolset`, `get_instructions` | browser-use/browser binary. `browse_web` 자율 브라우저 자식이 step별 모델 호출. call/agent별 세션, profile 경로·비밀 설정 | `H/browser_use/_capability.py:22-26,65-232,274-287` |
| `code_mode`: `CodeMode(tools='all', max_retries=3, max_tool_calls=100, os_access=None, mount=None, resource_limits=None, eager=False, speculate=None, monty_sandbox_url=None, dynamic_catalog=False)` | `for_run`, `get_wrapper_toolset`, `before_model_request`, `wrap_run_event_stream`, `after_tool_execute` | `pydantic-monty`; `run_code`로 sandbox code 실행. 자체 추가 모델 없음. run 내 Monty 재개 상태. OS/file은 별도 설정하지 않으면 제공 안 함 | `H/code_mode/_capability.py:58-208,224-313`; `H/code_mode/_toolset.py:39-54` |
| `spend`: `SpendLimits(budgets=(), store=InMemorySpendStore(), price=None, on_unpriced='zero', expose_tools=False, clock=utc_now)`; `Budget` | `before_model_request`, `wrap_model_request`, `get_toolset` | 추가 모델 없음. core 가격 registry/genai-prices; 기본 프로세스 메모리, 선택적 Redis/custom store. 선택적 `get_spend`; 기본 미가격 요청은 0으로 계산 | `H/spend/_capability.py:67-162,267-294` |
| `cache_stability`: `CacheStabilityMonitor=WarnOnCacheBusts` | canonical API와 동일 | deprecated alias; 별도 동작 없음 | `H/cache_stability/__init__.py:1-15` |
| `warn_on_cache_busts`: `WarnOnCacheBusts(min_missed_ratio, min_missed_tokens)`; deprecated `collapse_ratio`, `min_prefix_tokens`, `cache_ttl_seconds`; warning classes | `for_run`, `after_model_request` | core, 추가 모델·도구 없음. instance/conversation별 메모리 관측 상태. cache를 안정화하는 기능이 아니라 경고 | `H/warn_on_cache_busts/_capability.py:160-259,322-339` |
| `background_tools`: `BackgroundTools(tools={'background': True})`; optional metadata `background='optional'` | `for_run`, `before_tool_validate`, `wrap_tool_execute`, `after_node_run`, `wrap_run` | AnyIO, 추가 모델 없음. run 내 task group. CallDeferred/ApprovalRequired를 background 실패 문자열로 변환. deferred pause 포함 run 종료 시 취소; 동기 작업은 즉시 중단 불가 | `H/background_tools/_capability.py:116-153,245-294,325-344` |
| `conversation_search`: `ConversationSearch(source, scope=None, tool_id='conversation-search', max_matches=10, context_lines=5, bm25_k1=1.5, bm25_b, add_instructions=True)`; `HistorySource`, `SnapshotHistorySource` | `get_toolset`, `get_instructions` | 외부 검색 의존성 없는 BM25, 추가 모델·자체 저장 없음. source가 history/StepStore를 읽음. `search_conversation_history` 추가, scope 명시 필요 | `H/conversation_search/_capability.py:34-37,89-128,150-165` |
| `context`: deprecated module의 `RepoContext`; canonical `RepoContext(home_dir=None, filenames=('CLAUDE.md','AGENTS.md'), autoload_instructions=True, expose_inventory_tool=False, nested_traversal=False, nested_inject='pointer', asset_roots)`; `workspace_dir` ignored | `for_run`, `before_run`, `get_instructions`, `get_toolset`, `after_tool_execute` | core workspace에서 instruction 파일을 읽음. 추가 모델·자체 영속 store 없음. inventory 도구는 선택적 | `H/context/__init__.py:1-16`; `H/repo_context/_capability.py:44-126,153-205` |
| `tool_call_judge`: `ToolCallJudge(model, *, question, tools='all', include_conversation=False, conversation_window=4000, on_uncertain='block', denial_message=..., on_verdict=None)`; `ToolCallVerdict` | `before_tool_execute`, ordering innermost | core, 지정된 별도 모델 호출. 자체 도구·store 없음. `on_uncertain='ask'`는 ApprovalRequired; conversation 포함 시 판정 모델에도 transcript 전송 | `H/tool_call_judge/_capability.py:145-221,256-294` |

## openside 적용 제약

Rust 데몬이 도구 실행·정책·Vault·브라우저·티켓·세션 저장을 소유한다. capability가 설치돼 있다는 사실과 제품 채택 가능 여부는 별개다.

- `delegate_task`, `load_capability`, memory 도구, `ask_user_question`, `read_tool_result`, planning/search 도구, `run_code`도 도구 목록을 바꾼다. harness 도구라는 이유로 자동 예외를 만들지 않는다. 데몬 제공 목록과의 일치 또는 별도의 **명시적 orchestration 계약**이 필요하다.
- 현재는 실행 도구 없는 훅만 연결하거나, 공개 설정으로 도구 노출을 끈 범위만 사용한다. `FileSystem(read_only=True)`, 기존 브라우저에 CDP 연결, Bubblewrap 적용만으로 Rust의 정책 소유권이 복구되지는 않는다.
- 비밀 redaction은 Rust가 reverse RPC 결과를 보내기 전에 수행한다. 자격증명·Vault 값은 prompt, event, history, memory, overflow, snapshot, judge, summary에 들어가면 안 된다. injection defense는 secret redaction이 아니다.
- 이 스파이크의 `NONSECRET-MEMORY` 등은 공개 합성 값이다. daemon 로그·세션 DB·history 전체에 대한 secret canary 검증은 `unverified`다.

## 채택표 — docs/08 §E 복사용

아래는 스파이크의 **권고**이며 제품 구현 완료나 계약 변경을 뜻하지 않는다. `adopt`는 설정란의 제한된 모드만 대상으로 한다. 추가 capability의 실행·데몬 adapter는 `unverified`다.

| capability | adopt / not adopt / later | 이유 | 설정 참고 |
|---|---|---|---|
| subagents | later | 공용 프록시는 동작하지만 자식 deferred/approval 결과가 문자열로 변환됨 | explicit proxy binding; deprecated inherit_tools 금지; 자식 conversation/티켓 소유권과 부모 suspend adapter, 승인된 delegate 도구 필요 |
| skills | later | instruction 로딩은 동작하나 discovery와 load_capability의 도구 소유권 조정 필요 | directories와 include/exclude; 신뢰된 read-only/daemon workspace; PyYAML; 번들 자동 실행 금지 |
| compaction | adopt | 도구 없는 history 훅 동작, 세션 모델로 요약 가능 | model=None; threshold/window 설정; summarization_capabilities와 streaming 설정; 압축 후 Conversation 저장 |
| memory | later | 기본은 프로세스 메모리이고 내장 도구가 store에 직접 씀 | daemon-backed MemoryStore/도구 schema; session/agent namespace; injection_errors='raise'; 비밀 저장 금지 |
| ask_user | later | stdin 없는 deferred 재개 동작, 기본 도구 이름·schema는 데몬과 다름 | daemon 경로에 AskUserRequest/ask_user_result 재사용; answerer=None; DeferredToolRequests output; secret_input에 사용 금지 |
| step_persistence | later | 저장 이중화 및 Conversation metadata 누락 | daemon-backed StepStore, Rust 기준 저장소; session conversation_id와 fresh run_id; ConversationTypeAdapter 대체 금지 |
| tool_output_limits | later | Truncate도 read_tool_result를 추가하고 기본 Spill은 파일에 저장 | 현재 daemon limits 사용; 향후 도구 비노출/daemon overflow store; redaction 후 spill; Truncate-only로는 부족 |
| prompt_injection_defender | adopt | 추가 도구 없이 합성 위험 결과 차단 | block_high_risk=True, semantic_detection=False; 외부 결과 도구 선택; raw 로그 금지; stackone-defender |
| guardrails | adopt | 실행 도구 없이 입력·출력·결과 검사 가능 | nonparallel InputGuardrail; 결정적 guard; Rust 정책 우선; runtime unverified |
| repair_tool_arguments | adopt | 일반 schema 검증 전 도구 없는 JSON 복구 | 최종 검증·정책은 daemon; json-repair; heuristic 의미/runtime unverified |
| overflowing_tool_output | not adopt | deprecated alias | 호환 조건 충족 시 canonical tool_output_limits 사용 |
| system_reminders | adopt | static/dynamic callback은 실행 도구를 추가하지 않음 | static/GoalReanchor; 기본 LLMReminder 금지; 비밀 없는 내용; runtime unverified |
| planning | adopt | injection-only 모드에서 도구 비노출 가능 | tools=[], daemon 제공/read-only PlanStore 상태, inject=True; custom store/runtime unverified |
| bubblewrap_sandbox | not adopt | sidecar command wrapper는 daemon sandbox가 아님 | Rust가 sandbox와 fail-closed 동작 소유 |
| shell | not adopt | 직접 명령 도구가 daemon 정책을 우회 | daemon bash proxy만 사용; 도구 전부 끄면 중복 기능 |
| filesystem | not adopt | read-only도 daemon 파일 정책을 우회 | daemon read/write/edit proxy; 사용자 파일에 local harness workspace 부여 금지 |
| playwright | not adopt | 독립 브라우저 도구가 daemon controller/Vault 우회 | daemon browser_*만 사용; CDP 연결로 정책 우회가 해소되지 않음 |
| browser_use | not adopt | 자율 브라우저 도구가 정책을 우회하고 모델 호출 추가 | sidecar sensitive_data 금지; daemon browser_*만 사용 |
| code_mode | later | 프록시 래핑은 가능하나 run_code와 수명주기 검증 필요 | os_access/mount 없음; resource bounds; speculative/eager 효과 금지; deferred/restart/policy routing unverified |
| spend | adopt | 기본 도구 비노출, 세션 사용량·예산 보조 기능 | expose_tools=False; hard USD에 명시적 price와 on_unpriced='raise'; daemon 집계 우선; runtime unverified |
| cache_stability | not adopt | deprecated alias | warn_on_cache_busts 사용 |
| warn_on_cache_busts | adopt | 추가 도구 없는 진단 | canonical module; threshold 명시; 재시작 시 관측 상태 소실; runtime unverified |
| background_tools | later | deferred/approval이 실패로 바뀌고 run 종료 시 작업 취소 | 티켓 가능 도구 background 금지; Rust 작업 소유권·복구 먼저; runtime unverified |
| conversation_search | later | 검색 도구와 history 소유권·격리 경계 필요 | daemon-backed HistorySource; conversation scope 명시; daemon 명세; tenant 간 검색 금지 |
| context | not adopt | deprecated module, 기본 discovery를 신뢰된 데몬 context로 볼 수 없음 | daemon instruction 사용; 향후 trusted workspace/비노출 inventory의 RepoContext 검토 |
| tool_call_judge | later | 추가 모델·비용·정보 전송, 중복 정책 판정 | 명시적 model; on_uncertain='block'; include_conversation=False; Rust 판정을 덮어쓰지 않는 보조 기능만 |

## 실행 기록

명령은 아래 하나를 5회 실행했다. 처음 3회는 import 및 스파이크 작성 오류로 실패했고, 4회는 선택 의존성 오류를 결과로 기록한 상태에서 나머지 probe를 완료했다. 허가된 extras 설치 후 5회째는 8개 모두 실제 Agent에 연결해 실행했으며 exit 0, 0.53초였다.

```sh
agent/.venv/bin/python spikes/sidecar/s9_harness.py
```

작성 중 발생한 오류도 숨기지 않는다.

- `Skills` import: PyYAML 누락. 선택 의존성 설치로 해결했다.
- `TypeError: daemon_probe() takes 0 positional arguments but 1 was given`: `FunctionToolset.tool`은 context를 받으므로 context 없는 함수의 등록을 `tool_plain`으로 수정했다.
- `TypeError: 'RunUsage' object is not callable`: 2.55의 usage는 property이므로 `result.usage.requests`로 수정했다.
- 4회째는 skills/defender의 `dependency_error`를 출력했다. 실제 8개 연결 성공은 설치 후 마지막 실행에서 확인했다.

마지막 실행의 실제 stdout:

```text
versions {"harness": "0.55.0", "live_calls": 0, "pydantic_ai": "2.55.0"}
subagents.done {"child_tools": ["daemon_probe"], "delegate_return": "done", "parent_deferred": false, "parent_output_type": "str", "shared_proxy_calls": 1}
subagents.deferred {"child_tools": ["daemon_probe"], "delegate_return": "DeferredToolRequests(calls=[ToolCallPart(tool_name='daemon_probe', args={}, tool_call_id='call-1')], approvals=[], metadata={'call-1': {'ticket': 'fake-info'}})", "parent_deferred": false, "parent_output_type": "str", "shared_proxy_calls": 1}
subagents.approval {"child_tools": ["daemon_probe"], "delegate_return": "DeferredToolRequests(calls=[], approvals=[ToolCallPart(tool_name='daemon_probe', args={}, tool_call_id='call-1')], metadata={'call-1': {'ticket': 'fake-confirm'}})", "parent_deferred": false, "parent_output_type": "str", "shared_proxy_calls": 1}
skills {"bundled_execution": false, "loaded": true, "storage": "temporary configured workspace", "tools": ["load_capability"]}
skills.no_workspace {"error": "UserError"}
compaction {"messages_after": 5, "messages_before": 9, "model_calls": ["summary", "session"], "requests": 2, "summary_persisted": true}
memory {"files": ["memory/.memory-operations.json", "memory/session/main/MEMORY.md"], "injected": true, "second_run_injected": true, "tools": ["delete_memory", "read_memory", "search_memory", "write_memory"]}
ask_user.answerer {"answerer_calls": ["Mode"], "output": "done", "result": [{"Mode": ["Read"]}]}
ask_user.deferred {"approvals": 0, "calls": 1, "deferred_conversation_roundtrip": true, "request_id_matches": true, "resumed": "done", "stdin": false}
step_persistence {"conversation_id": "s9-conversation", "conversation_messages": 4, "files": ["s9-run/events.jsonl", "s9-run/run.json", "s9-run/snapshot-keys.jsonl", "s9-run/snapshots/0.json", "s9-run/snapshots/1.json", "s9-run/snapshots/2.json", "s9-run/snapshots/3.json", "s9-run/snapshots/4.json", "s9-run/snapshots/5.json", "s9-run/tool_effects.jsonl"], "messages_equal": true, "snapshot_has_deferred": false, "snapshot_has_usage": false, "snapshot_messages": 4, "snapshot_state": "complete", "usage_requests": 2}
step_persistence.reused_run_id {"error": "ValueError"}
tool_output_limits {"original_chars": 200, "read_tool_exposed_even_without_spill": true, "requests": 2, "result_chars": 40, "tools": ["daemon_output", "read_tool_result"]}
tool_output_limits.default_no_workspace {"error": "UserError"}
prompt_injection_defender {"blocked": true, "detections": [{"allowed": false, "risk": "high"}], "requests": 2, "result": "The result of `daemon_external_text` was withheld: it matched prompt injection patterns (risk: high). Do not retry this call; continue without this content and tell the user the tool result was blocked.", "semantic_model": false}
S9 COMPLETE: eight primary capabilities probed; temporary state removed; no live LLM calls
```

## unverified

- 실제 Rust reverse RPC·정책·데몬 제공 schema 연결.
- 자식 티켓의 부모 전달·재개·재시작 복구, 자식 run별 call ID 상관관계.
- StepPersistence의 crash/replay·동시 writer·pending ticket 복구 및 exactly-once 효과 실행.
- memory/overflow/plan/history의 데몬 기반 store adapter.
- 추가 capability 18개의 runtime 동작. 표의 권고는 소스 근거이며 실행 검증이 아니다.
- provider 자격증명 갱신·fallback·stream-only compaction 조합과 모든 live LLM·실제 브라우저 실행.
- semantic/ML detector의 다운로드·정확도·오탐·멀티모달 동작.
- daemon 로그·세션 DB·sidecar history 전체의 secret canary 부재.
- 출력 제한의 spill/binary/custom summarizer 경로.

cargo/build/lint/전체 테스트 suite는 실행하지 않았다.
