# S-14 Pydantic AI + omp auth broker provider

> 이 문서에 나오는 `spikes/` 아래 임시 스크립트와 내부 개발 기록(`docs/GOAL.md`, `docs/PROGRESS.md`, `docs/archive/`)은 공개 저장소에 포함되어 있지 않으며, 실행 명령과 출력 결과는 작업 과정을 확인하는 기록용으로 남겨 둔다.

날짜 2026-10-11. 재현: `spikes/pydantic-broker/` (`uv venv; uv pip install "pydantic-ai-slim[openai,anthropic]"`; `python broker_codex.py`, `python broker_claude.py`). 스크립트는 토큰을 출력하지 않는다.

결론: **가능(Codex 확정, Claude 는 셰이핑 필요)**.
1. broker `GET /v1/snapshot`(bearer = `~/.omp/auth-broker.token`, 헤더 `OMP-Auth-Broker-Capabilities: codex-meter-block-scopes`) → 활성 OAuth 크리덴셜의 access token·만료·accountId 획득.
2. Codex: `OpenAICodexCredentialSource.load()` 가 broker 를 읽는 구현으로 `OpenAICodexModel('gpt-6-luna')` 호출 성공(`pong`).
3. Claude: 셰이핑 없이는 429, 헤더 3종 + Claude Code 식별 시스템 프롬프트로 성공. 상세는 docs/08 §B.
4. 보류 도구(`CallDeferred` → 직렬화 → `DeferredToolResults` 재개)는 `spikes/pydantic-broker/{spike,gw_deferred}.py` 로 검증(gw_deferred 는 게이트웨이 stdio 경유 구판).
영향: ADR-002 개정(docs/06), docs/04 의 omp RPC 계약은 "broker 클라이언트 + 모델 provider" 로 대체, docs/09 툴 계약은 Pydantic AI 사이드카 기준으로 재작성.
미검증: 계정 로테이션/핀 구현, provider 의 자체 refresh 경로, 스트리밍 호출, Copilot/Antigravity provider.

## 추가: 사이드카 구현 후 live 스모크 (2026-10-11, GOAL 2 Phase 5-2)
대상은 실제 구현(`agent/src/openside_agent/providers`, 데몬 `opensided` + 사이드카). 토큰 값은 출력하지 않았고 broker 는 GET 만 사용(`/v1/healthz` → `200`, 스냅샷 읽기는 provider 내부).
1. Codex: `cd agent && OPENSIDE_LIVE_MODEL=openai-codex:gpt-6-luna uv run pytest -m live -q` → `1 passed, 90 deselected in 3.29s`.
2. Claude: `OPENSIDE_LIVE_MODEL=anthropic:claude-haiku-5-5 uv run pytest -m live -q` → `1 passed, 90 deselected in 1.67s`(셰이핑 적용, 429 없음).
3. 사람 단계 왕복: 임시 `XDG_*` + `config.toml`(`agent.command` = 저장소 venv, `models.default = "openai-codex:gpt-6-luna"`)로 `opensided` 실행 → `openside --workspace <tmp> "Call the ask_user tool exactly once ..."` → CLI 출력 `Session ... is waiting on tickets.` / `Ticket ... (info): Pick a letter` → `openside ticket resolve <id> b` → `openside --json session resume <id>` 마지막 이벤트: `ticket.resolved{resolution:{answer:"b"}}`, `turn.started`, `assistant.final{text:"b"}`, `session.settled`, 요약 `status: settled`. 데몬 SIGTERM 종료 코드 0, 남은 `opensided`/사이드카 프로세스 없음.
같은 날 `openside doctor`(같은 임시 설정): `credentials.0: openai-codex / omp-broker: broker OAuth account available`, `models: Default model configured; 0 fallback(s); providers available`.
3 의 이벤트 로그(`events.db`, `json_extract(payload,'$.kind')`): `3 runtime.started startup_ms=1573`(사이드카 spawn→ready, 호스트 측정), `7 tool.started ask_user call_5f3d…`, `9 ticket.opened tool_call_id=call_5f3d…`, `10 tool.ended`, `11 message.completed`, `12 session.settled`, `13 ticket.resolved`, `14 turn.started`, `18 assistant.final`, `19 session.settled`. 재개 런은 새 `tool.started` 없이 같은 `tool_call_id` 의 결과로 이어졌다.
