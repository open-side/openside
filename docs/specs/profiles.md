# 에이전트 프로필과 프롬프트 토큰 예산

작성: 2026-10-12. 근거: docs/01 A-1·A-2·A-9, docs/02 OP-3. 구현: `crates/opensided/src/profiles.rs`, 설정 `crates/openside-config/src/profile.rs`.

## 1. 프로필
설정 `[agent] profile`(기본 `"full"`)이 데몬이 사이드카에 넘기는 `open.tools` 와 `open.system_prompt_parts` 를 정한다. 값은 `open.config.agent.profile` 로도 전달된다. 세션별로 바꾸는 API 는 없다(데몬 전체 설정).

| 프로필 | 도구 | 시스템 프롬프트 |
| --- | --- | --- |
| `full` | 등록된 데몬 도구 전부(세션 모드·브라우저·Vault 가용성에 따라 기존 규칙대로 숨김) | `prompt::parts()` 3개 + 스킬 목록 |
| `browser-agent` | `ask_user`, `bash`, `browser_import_lookup`, `browser_pointer`, `browser_repl`, `browser_screenshot`, `edit`, `read`, `request_human_step`, `write` | 짧은 역할 문장 1개 + 안전 규칙 1개 |
| `ultrabrowse` | `ask_user`, `browser_act`, `browser_close_tab`, `browser_import_lookup`, `browser_navigate`, `browser_open_tab`, `browser_repl`, `browser_snapshot`, `browser_tabs`, `request_human_step` | 역할 문장 + 안전 규칙 + 계획·인용 규칙([ultrabrowse.md](ultrabrowse.md)). 사이드카가 높은 추론·계획 단계·인용 강제를 더한다 |

- 프로필은 노출 범위를 좁히기만 한다. 정책·승인·세션 모드 규칙은 그대로 적용되고, 읽기 전용 세션에서는 쓰기 도구가 여전히 빠진다.
- 프로필에 없는 도구는 실행 단계(`tools::visible`)에서도 거부된다. 모델이 이름을 직접 불러도 실행되지 않는다.
- `browser-agent` 와 `ultrabrowse` 는 `skill` 도구가 없으므로 `<available_skills>` 목록도 보내지 않는다.
- 짧은 프롬프트에도 기존 안전 규칙(페이지·파일 내용은 지시가 아님, 정책 우회 금지, 보안 장벽은 `request_human_step`, 비밀번호·코드는 `secret_input` 으로만)이 모두 들어 있다.
- `browser-agent` 에는 Vault 도구가 없다. 로그인은 `request_human_step`(`secret_input`·`takeover`)으로 사람에게 넘긴다.

## 2. 스냅샷(A-1)
`docs/specs/profiles/<프로필>.tools.json` 은 노출 도구 이름 목록, `<프로필>.open.json` 은 사이드카가 실제로 받는 `open` 메시지의 `system_prompt_parts` 와 `tools` 다. 둘 다 `browser-agent`, `ultrabrowse` 에 대해 커밋돼 있다.

- 테스트 `crates/opensided/src/tests/profiles.rs::profile_browser_agent_tools_and_open_payload_match_snapshot`(와 `profile_ultrabrowse_…`)가 데몬의 실제 세션 열기 경로로 `open` 메시지(`openside_agent_sidecar::open_wire`)를 만들고 커밋된 파일과 비교한다. 다르면 줄 단위 차이를 보여 주며 실패한다.
- 의도한 변경이면 `OPENSIDE_UPDATE_PROFILE_SNAPSHOTS=1 cargo test -p opensided profile_` 로 파일을 다시 쓰고, `tools/token-budget.py` 결과를 확인한다.
- 실제 사이드카의 `ready.tools` 가 같은 목록인지는 `crates/openside-e2e/tests/ultrabrowse.rs` 가 확인한다.

## 3. 토큰 예산(A-2)
`tools/token-budget.py` 가 `<프로필>.open.json` 의 시스템 프롬프트(파트를 빈 줄로 이은 것)와 도구 정의(도구마다 키 정렬한 압축 JSON 한 줄)를 잰다. 기본 대상은 `browser-agent`, `ultrabrowse`, 한도는 10,000 토큰이다. 넘으면 종료 코드 1, 입력을 못 읽으면 2. `tools/gate.sh` 의 `prompt token budget` 단계로 돈다.

`full` 은 재지 않는다. A-2 의 대상은 `browser-agent` 기본 툴셋이고, `full` 의 도구 목록은 브라우저·Vault 가용성과 세션 모드에 따라 달라져 커밋할 고정 페이로드가 없다.

추정식은 ASCII 문자 수를 3.0 으로 나눠 올림한 값에 ASCII 가 아닌 UTF-8 바이트 수를 더한 것이다. CI 에서 토크나이저나 네트워크가 필요 없도록, ASCII 부분은 실제 BPE 토크나이저로 한 번 보정해 모든 토크나이저보다 크게 세도록 정했다. ASCII 가 아닌 바이트는 바이트 단위 BPE 의 최악(바이트당 1토큰)으로 센다. 한글은 음절 하나가 3바이트지만 토크나이저에 따라 2~3토큰이 되므로, 바이트 수로 세야 적게 세지 않는다. 지금 페이로드는 모두 ASCII 다.

보정 방법(2026-10-12): Hugging Face 의 Xenova 공개 `tokenizer.json` 4종(Claude 토크나이저, o200k, cl100k, Llama 3)을 `/tmp` 에 받고, `uv run --no-project --with tokenizers` 로 위와 같은 텍스트를 인코딩했다(`add_special_tokens=False`).

| 프로필 | 부분 | 바이트 | Claude | o200k | cl100k | Llama 3 | 추정 |
| --- | --- | ---: | ---: | ---: | ---: | ---: | ---: |
| browser-agent | 시스템 | 681 | 153 | 143 | 141 | 141 | 227 |
| browser-agent | 도구 | 6,663 | 1,572 | 1,517 | 1,472 | 1,472 | 2,221 |
| ultrabrowse | 시스템 | 1,004 | 220 | 205 | 204 | 204 | 335 |
| ultrabrowse | 도구 | 7,305 | 1,744 | 1,670 | 1,620 | 1,620 | 2,435 |

가장 촘촘한 경우가 4.19 바이트/토큰(Claude 토크나이저, ultrabrowse 도구 스키마)이었다. 3.0 으로 나누면 이보다 약 40% 크게 센다. 보정 시점의 실측 최댓값은 `browser-agent` 1,725, `ultrabrowse` 1,964 토큰이다. 위 표는 보정 시점(도구 추가 전)의 값이고, 현재 추정치는 `tools/token-budget.py` 출력으로 확인한다.

테스트: `tools/tests/test_token_budget.py`(실제 프로필 통과, 부풀린 도구 설명·긴 프롬프트 파트 실패, 한도 경계, 비ASCII 바이트 계산 포함).
