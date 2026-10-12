# Ultrabrowse(리서치 모드)와 인용 측정

작성: 2026-10-12. 근거: docs/01 A-9. 구현: 데몬 `crates/opensided/src/profiles.rs`(`ultrabrowse` 프로필), 사이드카 `agent/src/openside_agent/research.py`·`citations.py`, 하네스 `tools/citation-harness.py`.

## 1. 켜는 법과 구성
`[agent] profile = "ultrabrowse"`. 모델에는 브라우저 도구, `request_human_step`, `ask_user` 만 노출되고 셸·파일 도구는 없다([profiles.md](profiles.md) §1). 사이드카는 프로필을 `open.config.agent.profile` 로 받아 다음을 더한다.

| 기능 | 구현 |
| --- | --- |
| 높은 추론 | 모델 설정 `ModelSettings(thinking="high")`. Pydantic AI 의 공통 설정이라 제공자마다 맞는 요청 필드로 바뀐다(Anthropic thinking, OpenAI reasoning effort 등). 추론 수준을 지원하지 않는 모델에는 보내지 않는다. 다른 프로필은 제공자 기본값을 쓴다 |
| 계획 단계 | §2 |
| 출처 인용 강제 | §4 |

## 2. 계획 단계
- 이번 요청에서 모델이 `Plan:` 줄과 번호 목록을 쓰기 전에는 `browser_*` 호출이 데몬에 가지 않는다. 사이드카가 `ModelRetry` 로 돌려보내고, 모델은 계획을 쓰라는 안내를 받는다.
- "이번 요청"은 마지막 사용자 메시지 이후다. steer 로 새 메시지가 들어오면 계획을 다시 세운다.
- 세운 계획은 harness `Planning(tools=(), inject=True)` 가 이후 모든 모델 요청 끝에 `<plan-reminder>` 로 붙인다. 이 알림은 대화 기록에 저장되지 않는다. 계획 도구는 모델에 노출하지 않으므로 도구 목록은 데몬 것 그대로다.
- 계획은 매 요청마다 대화 기록에서 다시 읽는다. 사이드카가 재시작되거나 승인 대기 후 재개돼도 계획이 유지된다.

## 3. 주장 판정 휴리스틱
`citations.check(answer, visited)` 가 답변을 문장 단위로 나눠 판정한다.
- 줄마다 문장을 나눈다. 제목(`#`), 코드 블록, 목록 기호는 뺀다.
- 끝 문장부호 바로 뒤의 링크는 그 문장에 속한다(`사실. [출처](url) 다음 사실`). 링크만 있는 줄은 앞 문장에 붙는다.
- `e.g.`, `i.e.`, `U.S.`, `Dr.` 같은 약어와 `J.` 같은 머리글자 뒤의 마침표에서는 나누지 않는다. 숫자 뒤의 마침표, `etc.`, `no.` 에서는 나눈다.
- 링크와 URL 을 지운 뒤 단어가 4개(`MIN_CLAIM_WORDS`) 이상이고 `?` 로 끝나지 않으면 사실 주장이다.
- 주장 안의 URL 하나라도, 정규화한 값이 방문한 페이지와 같으면 인용된 것으로 본다. 정규화는 스킴·호스트 소문자화, 쿼리·프래그먼트·끝 `/`·끝 문장부호 제거다. 괄호가 한 겹 들어간 URL(`…/wiki/Nile_(river)`)도 하나로 읽는다. 열어 보지 않은 페이지 링크는 인용이 아니다.
- 방문 페이지는 `browser_open_tab`, `browser_navigate`, `browser_act`, `browser_repl` 결과의 `url` 필드뿐이다. 데몬은 탐색 뒤 최상위 프레임의 실제 URL(리다이렉트 반영, 쿼리·프래그먼트 제거)을 이 필드에 넣는다. 스냅샷 본문, 콘솔 출력, 반환값, 탭 목록에 나오는 URL 은 페이지 내용일 뿐 방문 증거가 아니라서 세지 않는다.

휴리스틱이라 짧은 사실 문장(3단어 이하)이나 한 문장에 여러 사실이 든 경우는 구분하지 못한다.

## 4. 인용 강제
- `OutputGuardrail` 이 최종 답변을 검사한다. 인용 없는 주장이 있으면 `ModelRetry` 로 돌려보낸다. 재시도 안내에는 해당 문장과 이번 세션에서 연 페이지 목록이 들어간다. 이전 브라우징 결과는 대화 기록에 그대로 남으므로, 모델은 답만 고쳐 쓰거나 더 찾아본다.
- 재시도는 `CITATION_RETRIES`(2)와 런의 출력 재시도 한도(`[agent] tool_retries`) 중 작은 값까지다. 그 뒤에도 남은 주장에는 문장 끝에 ` [citation needed]` 를 붙여 전달한다. 모두 인용된 답변은 바꾸지 않고, 표시는 다시 적용해도 중복되지 않는다.
- 스트리밍 일관성: ultrabrowse 에서는 응답의 텍스트를 바로 `assistant.delta` 로 보내지 않고 응답이 끝날 때까지 모은다. 같은 응답에 도구 호출이 있으면 그 텍스트(계획·진행 설명)를 `assistant.delta` 하나로 보낸다. 도구 호출이 없는 응답은 최종 답변 후보이므로 delta 로 보내지 않는다. 검사를 통과하거나 표시가 붙은 답변만 `assistant.final` 로 한 번 나간다. 그래서 거절된 답변은 사용자에게 보이지 않고, delta 와 final 이 서로 어긋나지 않는다. 추론 과정(`thinking.delta`)은 그대로 스트리밍한다.
- 다른 프로필의 답변은 검사하지 않고 delta 도 바로 보낸다.

## 5. 측정 하네스
```
tools/citation-harness.py --answer answer.md --visited URL [--visited URL ...]
tools/citation-harness.py --answer - --visited-file results.json --max-uncited-ratio 0
```
- `--visited-file` 은 브라우저 도구 결과나 URL 의 JSON 목록을 받는다. 결과 객체는 최상위 `url` 필드만 센다.
- 출력은 JSON 한 줄이다: `claims`, `uncited`, `uncited_ratio`, `uncited_claims`.
- 종료 코드: 0 측정 완료(임계값을 줬다면 그 이하), 1 `uncited_ratio` 가 `--max-uncited-ratio` 초과, 2 입력 오류.
- 사이드카와 같은 `citations.py` 를 경로로 불러 쓰므로 표준 라이브러리 Python 만 있으면 된다.

## 6. 테스트
- `agent/tests/test_citations.py`: 판정 규칙(약어, 문장부호 뒤 링크, 괄호 URL), 방문 출처는 `url` 필드만 인정, 계획 전 브라우징 거절, 계획 재주입, 추론 수준 `high`, 인용 재시도와 재시도 소진 뒤 표시, delta 일관성, 다른 프로필 무변경.
- `tools/tests/test_citation_harness.py`: CLI 비율·종료 코드.
- `crates/opensided/src/tests/browser.rs`, `tests/repl.rs`: `browser_navigate`·`browser_repl` 결과의 `url`.
- `crates/openside-e2e/tests/ultrabrowse.rs`: 실제 데몬·사이드카·Chromium 으로 루프백 페이지 3개를 연다. 계획 전 호출 거절, 인용 없는 첫 답변의 재시도, 인용이 모두 달린 최종 답변(하네스 비율 0), 재시도 소진 뒤 `[citation needed]` 표시를 확인한다. Chromium 이 없으면 이유를 출력하고 건너뛴다.
