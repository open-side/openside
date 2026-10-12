# S-8 실행 중 steer 주입과 deferred 경계

> 이 문서에 나오는 `spikes/` 아래 임시 스크립트와 내부 개발 기록(`docs/GOAL.md`, `docs/PROGRESS.md`, `docs/archive/`)은 공개 저장소에 포함되어 있지 않으며, 실행 명령과 출력 결과는 작업 과정을 확인하는 기록용으로 남겨 둔다.

- 작성일: 2026-10-11
- 대상: Python sidecar 유지보수자

## 검증 범위 및 환경
- 실행 환경: `pydantic-ai-slim 2.55.0` 설치 환경.
- 검증 방식: `FunctionModel` 기반 오프라인 단위 검증이다. 실제 LLM API, 브라우저 프로세스, broker 통신은 일절 배제한다.
- 소스 코드 인용 기준: 별도 언급이 없는 한 인용 경로는 `agent/.venv/lib/python3.13/site-packages/pydantic_ai/` 기준 상대 경로이다. 단독 콜론 행 번호(`:line`) 표기는 직전에 언급된 파일을 가리킨다.
- 스파이크 스크립트: `spikes/sidecar/s8_steer.py`
- 재현 명령(저장소 루트 기준): `agent/.venv/bin/python spikes/sidecar/s8_steer.py` (종료 코드 0 확인 완료). 네이티브 프로브 확인 후 전체 확장 스크립트로 2회 실행하여 검증을 완료했다.

## Pydantic-AI 네이티브 API 동작 분석
Pydantic-AI는 실행 핸들을 통해 동기식 큐 주입 인터페이스를 제공한다.

1. **실행 핸들 획득**: `async with agent.iter(prompt) as run:` 구조를 사용할 때만 실시간 실행 핸들(`run`)을 얻을 수 있다(`run.py:736-774`). `agent.run()`은 내부적으로 `iter`를 호출하지만 외부로 라이브 핸들을 반환하지 않는다.
2. **큐 주입 인터페이스**: 외부 비동기 태스크나 소켓 리더는 동기 메서드인 `run.enqueue(text, priority='asap')` 또는 `priority='when_idle'`을 호출할 수 있다. 툴 내부나 이벤트 핸들러에서는 `RunContext.enqueue`를 통해 동일한 큐에 접근한다(`_run_context.py:837-887`). 에이전트 단위 메서드인 `Agent.enqueue`는 존재하지 않는다.
3. **이벤트 및 락 메커니즘**: `enqueue` 호출은 `enqueue_id`를 반환하며, 큐 메시지가 반영될 때 `EnqueuedMessagesEvent`가 발행된다. 이는 현재 진행 중인 모델 호출을 즉시 인터럽트하는 것이 아니다(`_enqueue.py:34-43, 143-167`). 큐 추가 및 인출(drain)은 스레드 락으로 보호된다. 인자 없이 호출하는 `run.enqueue()`는 no-op으로 처리되지만, 빈 문자열인 `run.enqueue('')`는 `UserPromptPart('')`를 생성한다. 닫힌 큐에 주입을 시도하면 `UserError`가 발생하며(`_enqueue.py:170-207`), 실행 종료 시 drain과 close는 원자적(atomic)으로 완료된다.
4. **우선순위 동작 및 히스토리 관측**: `asap` 우선순위는 다음 실제 모델 요청 직전이나 종료 직전의 터미널 리다이렉트 시점에 즉시 소진된다(`capabilities/_pending_messages.py:75-101`). 반면 `when_idle`은 별도의 사전 모델 응답 턴을 기다리는 것이 아니라, 정상 종료(otherwise-termination) 시점에 호출되는 `drain_at_end`에서 `asap` 메시지와 함께 한꺼번에 추출(drain)되어 `asap` 뒤에 덧붙여진 후 터미널 리다이렉트를 유발한다(`capabilities/_pending_messages.py:104-163`). 네이티브 히스토리에는 각 메시지가 개별 `ModelRequest`로 저장되지만, 모델 어댑터에 전달될 때는 인접 요청들이 단일 프롬프트로 병합되는 현상이 관측된다. 일반적인 툴 실행 시 네이티브 흐름에서는 총 3회의 모델 요청이 관측된다.

### 핵심 결함: DeferredToolRequests와의 충돌
네이티브 엔드포인트의 종료 처리 drain 로직은 `DeferredToolRequests` 반환을 일반 종료 조건과 구분하지 못한다.

- 툴이 실행을 지연(defer)하고 티켓을 기다리는 상태에서 외부에서 `asap` 또는 `when_idle` 메시지가 인큐되면, Pydantic-AI는 이를 즉시 소진하기 위해 추가 모델 요청을 전송한다.
- 이로 인해 최종 반환값이 `DeferredToolRequests`가 아닌 일반 문자열(`str`)로 덮어씌워지며, 지연 대기 중이던 툴 호출은 모델 컨텍스트 내에서 인위적인 중단(synthetic interrupted) 상태로 변질된다(`capabilities/_pending_messages.py:123-163`, `run.py:512`, `_tool_execution.py:1062-1077, 1130-1137`, `_agent_graph.py:875-878`, `agent/__init__.py:4443-4445`).
- 지연 상태로 이미 종료된 실행에 뒤늦게 `enqueue`를 호출하면, 실행이 대기 중이지 않다는 이유로 `UserError`가 발생한다.
- **결론**: 지연 실행을 처리해야 하는 openside 환경에서는 네이티브 `run.enqueue` 및 `ctx.enqueue`를 외부 steer 주입 수단으로 사용해서는 안 된다.

## 확정된 안전 전략 (Sidecar 세션 관리 버퍼 방식)
Main이 확정한 바와 같이 네이티브 큐를 배제하고 sidecar 세션이 직접 소유하는 steer 버퍼 전략을 채택한다.

```python
message = ModelRequest(
    parts=[UserPromptPart(text)],
    run_id=ctx.run_id,
    conversation_id=ctx.conversation_id,
)
ctx.messages.append(message)
request_context.messages = [*request_context.messages, message]
```

1. **주입 시점 제어**: 커스텀 `AbstractCapability.before_model_request` 훅을 구현하여, 실제 다음 모델 요청이 성립하는 시점에만 버퍼를 비워 메시지를 주입한다(`capabilities/abstract.py:937-953`, `models/__init__.py:327-360`, `_run_context.py:198-203`).
2. **경고 회피 및 명시적 할당**: `request_context.messages`에 대해 `append`, `extend`, `+=` 연산을 직접 쓰면 자동 히스토리 영속화 관련 지원 중단 경고(deprecated auto-history persistence warning)가 발생한다. 새 리스트를 생성해 명시적으로 재할당(`request_context.messages = [*..., message]`)하고 run의 인메모리 히스토리인 `ctx.messages`에도 함께 추가함으로써 경고를 피하고 향후 API 변경에 대비한다.
3. **메타데이터 보존**: 대화 및 실행 컨텍스트 추적을 위해 `run_id`와 `conversation_id`를 명시적으로 저장한다.

### 안전 전략 동작 증명
- **일반 툴 실행 중 주입**: 툴 대기 상태에서 메시지가 도착하면, 2번째 모델 요청 직전 `message_accepted`가 정확히 1회 기록되고 안전하게 주입된다.
- **최종 모델 응답 중 주입**: 1번째 요청 시작 후 메시지가 도착하면 추가 모델 요청이 발생하지 않으므로 주입되지 않고 pending 상태로 남는다. 1차 실행은 정상 문자열로 끝나며, sidecar가 새로운 후속 실행(NEW run)을 시작할 때 첫 요청에서 해당 메시지를 소진한다.
- **지연 툴(Deferred tool) 실행 중 주입**: 1차 실행 결과는 변형 없이 `DeferredToolRequests`로 유지되며, steer 메시지는 보류(pending=1, accepted=[]) 상태로 남는다. 이후 `DeferredToolResults(calls={'buffer-1': {'status': 'filled'}})`와 기존 대화 기록을 전달하여 재개(resume)하면, 재개된 첫 번째 모델 요청에 채워진 툴 결과와 버퍼링된 steer 메시지가 함께 전달되며 중단 반환(interrupted return)이 발생하지 않는다.
- **툴 실행 중 중단(Abort)**: 실행 중 취소 시 `RunCancelled`가 발생하고 비동기 툴이 취소된다. sidecar의 보류 메시지는 소진되지 않고 pending=1, accepted=[] 상태를 유지하며, 이후 새로운 실행으로 명시적 재개할 때 훅을 통해 정확히 1회 주입된다.

## 시맨틱 및 수명 주기 정의
- **Queue 모드 동작**: 단일 실행 내에서 비우는 네이티브 `when_idle` 방식이 아니라, 현재 실행이 정상 완료된 후 sidecar가 별도의 새로운 실행(NEW run)을 시작하여 처리하는 후속 큐 방식으로 동작한다.
- **지연 대기 중 Steer 보관**: 지연 티켓이 외부에서 해결될 때까지 steer 메시지는 버퍼에 보관되며, 결과를 조작하거나 모델을 임의로 진행시키지 않는다.
- **중단(Abort) 시 버퍼 보존**: 취소 시 미전달 버퍼는 삭제되지 않고 보존되지만, 데몬이나 sidecar가 새 실행을 명시적으로 시작하기 전까지는 자동으로 실행되지 않는다. 세션 재시작 및 장애 복구를 위한 독립 큐 영속화는 본 스파이크의 구현 범위를 벗어난다.
- **`message_accepted` 로깅 기준**: 소켓 수신 시점이 아니라 인메모리 `ctx.messages` 및 히스토리 삽입이 완료된 후 모델 어댑터가 호출되기 직전에 훅 내부에서 발행된다. 이는 디스크 영속화나 프로바이더 호출 성공을 보장하는 것이 아니며, 이후 훅 실패, API 호출 에러, 취소가 발생할 수 있다. 내구성 있는 수락 확인(durable ACK) 연동은 미검증(unverified) 상태이다.

## 실행 취소(Abort) API 규약
실시간 실행 취소는 공식적으로 `run.cancel()`을 호출하며, 툴 내부에서는 `RunContext.cancel()`을 사용할 수 있다(`run.py:776-805`, `_cancel.py:1-18, 149-181`). `agent.run(cancellation_token=...)`을 통한 취소 토큰도 지원된다(`_cancel.py:37-70`).

- 태스크 취소 시 `iter` 컨텍스트 종료 시점에 예외가 `RunCancelled`로 변환되므로 `async with` 블록 외부에서 포착해야 한다. 외부 `asyncio.Task.cancel()`을 직접 호출하면 기본 `CancelledError`로 남지만, `RunCancelled.from_cancellation(exception)`을 통해 취소 상태를 복원할 수 있다(`exceptions.py:263-327`).
- `RunCancelled` 객체는 `all_messages()`, `usage`, `run_id`를 보존한다(`exceptions.py:274-279, 356-365`). 취소 시점의 가공되지 않은 raw 스냅샷은 미완료 `request[]`를 포함하며, 해당 히스토리로 재개할 때 Pydantic-AI가 synthetic interrupted 툴 반환을 자동으로 주입한다.
- 네이티브 큐의 미전달 메시지는 취소 히스토리에 포함되지 않으므로(`_agent_graph.py:2809-2821`), sidecar 자체 버퍼로 보존해야 한다. 동기 워커 스레드 툴은 취소 시 스레드 실행을 강제로 중단시킬 수 없는 구조적 한계가 존재한다.

## 재현 실행 결과
아래 출력은 `agent/.venv/bin/python spikes/sidecar/s8_steer.py`를 실행하여 얻은 결과이다.

```text
pydantic-ai-slim: 2.55.0

ACTIVE ENQUEUE
socket-reader: queued asap + when_idle while pause was running
same-run follow-up: follow-up-complete model-requests: 3
history: request[user=START]; response[call=pause:pause-1]; request[return=pause:success:tool-complete]; request[user=STEER]; response[text=steer-complete]; request[user=FOLLOW-UP]; response[text=follow-up-complete]
model-request-1: request[user=START]
model-request-2: request[user=START]; response[call=pause:pause-1]; request[return=pause:success:tool-complete | user=STEER]
model-request-3: request[user=START]; response[call=pause:pause-1]; request[return=pause:success:tool-complete | user=STEER]; response[text=steer-complete]; request[user=FOLLOW-UP]
after-end enqueue: UserError `enqueue` is not available because the agent run has ended.

ABORT MID-TOOL
abort: RunCancelled tool-cancelled: True
snapshot: requests=1run-id-present=True
history: request[user=ABORT-START]; response[call=pause:abort-1]; request[]
undelivered queue appears in cancellation history: False
resume model-request: request[user=ABORT-START]; response[call=pause:abort-1]; request[return=pause:interrupted:The tool call was interrupted before a result was produced. | user=RESUME]

DEFERRED BOUNDARY no-queue
output: DeferredToolRequests model-requests: 1
history: request[user=DEFER]; response[call=ticket:ticket-1]
after-deferred/end enqueue: UserError
deferred resume: request[user=DEFER]; response[call=ticket:ticket-1]; request[return=ticket:success:{'status': 'filled'} | user=QUEUED-IN-NEW-RUN]; response[text=after-deferred]

DEFERRED BOUNDARY asap
output: str model-requests: 2
history: request[user=DEFER]; response[call=ticket:ticket-1]; request[user=QUEUED-BEFORE-DEFERRED]; response[text=after-deferred]
redirect model-request: request[user=DEFER]; response[call=ticket:ticket-1]; request[return=ticket:interrupted:The tool call was interrupted before a result was produced. | user=QUEUED-BEFORE-DEFERRED]
after-deferred/end enqueue: UserError

DEFERRED BOUNDARY when_idle
output: str model-requests: 2
history: request[user=DEFER]; response[call=ticket:ticket-1]; request[user=QUEUED-BEFORE-DEFERRED]; response[text=after-deferred]
redirect model-request: request[user=DEFER]; response[call=ticket:ticket-1]; request[return=ticket:interrupted:The tool call was interrupted before a result was produced. | user=QUEUED-BEFORE-DEFERRED]
after-deferred/end enqueue: UserError

SIDECAR BUFFER normal
model called: request-1; accepted=[]
socket-received: msg-1; message_accepted not emitted yet
message_accepted: msg-1; before_model_request; persisted=True
model called: request-2; accepted=['msg-1']
first-run output: str
pending after first run: 0 accepted: ['msg-1']
first-run history: request[user=BUFFER-START]; response[call=pause:buffer-1]; request[return=pause:success:tool-complete]; request[user=BUFFERED-STEER]; response[text=buffer-delivered]
delivery model-request: request[user=BUFFER-START]; response[call=pause:buffer-1]; request[return=pause:success:tool-complete | user=BUFFERED-STEER]
buffer outcome: delivered once; only at a real model-request boundary

SIDECAR BUFFER terminal
model called: request-1; accepted=[]
socket-received: msg-1; message_accepted not emitted yet
first-run output: str
pending after first run: 1 accepted: []
first-run history: request[user=BUFFER-START]; response[text=first-run-complete]
message_accepted: msg-1; before_model_request; persisted=True
model called: request-2; accepted=['msg-1']
new/resumed-run history: request[user=BUFFER-START]; response[text=first-run-complete]; request[user=FOLLOW-UP-RUN]; request[user=BUFFERED-STEER]; response[text=buffer-delivered]
delivery model-request: request[user=BUFFER-START]; response[text=first-run-complete]; request[user=FOLLOW-UP-RUN | user=BUFFERED-STEER]
buffer outcome: delivered once; only at a real model-request boundary

SIDECAR BUFFER deferred
model called: request-1; accepted=[]
socket-received: msg-1; message_accepted not emitted yet
first-run output: DeferredToolRequests
pending after first run: 1 accepted: []
first-run history: request[user=BUFFER-START]; response[call=pause:buffer-1]
message_accepted: msg-1; before_model_request; persisted=True
model called: request-2; accepted=['msg-1']
new/resumed-run history: request[user=BUFFER-START]; response[call=pause:buffer-1]; request[return=pause:success:{'status': 'filled'}]; request[user=BUFFERED-STEER]; response[text=buffer-delivered]
delivery model-request: request[user=BUFFER-START]; response[call=pause:buffer-1]; request[return=pause:success:{'status': 'filled'} | user=BUFFERED-STEER]
buffer outcome: delivered once; only at a real model-request boundary

SIDECAR BUFFER abort
model called: request-1; accepted=[]
socket-received: msg-1; message_accepted not emitted yet
abort outcome: RunCancelled; async tool cancelled=True
pending after first run: 1 accepted: []
first-run history: request[user=BUFFER-START]; response[call=pause:buffer-1]; request[]
message_accepted: msg-1; before_model_request; persisted=True
model called: request-2; accepted=['msg-1']
new/resumed-run history: request[user=BUFFER-START]; response[call=pause:buffer-1]; request[return=pause:interrupted:The tool call was interrupted before a result was produced.]; request[user=FOLLOW-UP-RUN]; request[user=BUFFERED-STEER]; response[text=buffer-delivered]
delivery model-request: request[user=BUFFER-START]; response[call=pause:buffer-1]; request[return=pause:interrupted:The tool call was interrupted before a result was produced. | user=FOLLOW-UP-RUN | user=BUFFERED-STEER]
buffer outcome: delivered once; only at a real model-request boundary
```

## 미검증 영역 (Unverified)
- 전송 계층 소켓 프로토콜 및 내구성 있는 수락 확인(durable ACK) 연동 (unverified)
- 데몬 원격 툴 취소 RPC 메커니즘 및 실제 취소 전파 (unverified)
- 스트리밍 응답 도중 발생하는 취소 레이스 컨디션 (unverified)
- 데몬 프로세스 비정상 종료 시 sidecar 큐의 독립 영속화 및 재시작 복구 흐름 (unverified)