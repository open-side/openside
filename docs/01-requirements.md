# 01. openside 요구사항

openside의 기능 요구사항을 정리한 문서다. 완료 기준과 측정 방식은 `docs/05-roadmap.md` §1에 정의되어 있다.

범례
- **단계**: M0~M8 (로드맵 마일스톤).
- **상태 표기**: 단계 열의 `(구현됨)`은 `docs/specs/acceptance.csv`의 해당 AC 행이 모두 `pass`(테스트 존재)임을 뜻하고, `(부분 구현)`은 미충족 항목이 있어 AC 칸에 그 내역을 적었음을, `(미구현)`은 해당 요구사항의 AC 행이 하나도 통과하지 못해 모두 `unverified` 임을, `(blocked-external)`은 외부 환경(실기기·계정)이 필요함을 나타낸다.
- **기반**: 에이전트 런타임인 Pydantic AI 사이드카(`docs/08`)와 데몬이 이 요구사항을 어떻게 채우는가.
  - 🟢 Pydantic AI / `pydantic-ai-harness` 기능을 그대로 사용 (설정·연결만)
  - 🟡 사이드카 또는 데몬이 그 위에 추가 구현 (툴 프록시, 훅, 데몬 로직)
  - 🔴 기존 기능이 없어 openside 가 처음부터 구현
- 모든 요구사항은 **수용 기준(AC)** 을 가진다. AC 를 통과하는 자동 테스트가 있어야 "완료".
- 비목표: 다른 제품의 코드·프롬프트·에셋 복제, 타사 상표 사용, 서드파티 클라우드 서비스와의 호환성.

## 0. 공통 전제
| ID | 전제 | 
| --- | --- |
| G-1 | 구현 언어는 Rust (데몬, CLI, 브라우저 컨트롤러, Vault, 정책, 스케줄러). 웹 UI/브라우저 확장만 TypeScript. |
| G-2 | Pydantic AI 사이드카는 모델 요청 시도마다 새로운 비공개 자격 증명 스냅샷을 가져온다. 명시적 프로바이더 풀, 어피니티, 사용량 및 차단, 갱신은 데몬이 소유한다. API 키와 관리형 구독 OAuth 상태는 OS 키체인 기반 Vault를 정본으로 삼는다. 사이드카에는 액세스 키와 토큰만 전달되며, 리프레시 토큰이나 공개·설정·로그 출력에는 절대 노출되지 않는다. 관리형 CLI 홈은 비공개 자격 증명 파일을 직접 생성하며, 더 최신의 외부 로그인 상태는 리비전과 mtime을 기준으로 반영하고 삭제된 계정은 완전히 지운다. 기본 사용자 홈은 읽기 전용으로 유지된다. 브로커 장애가 발생해도 목록에 없는 소스로 넘어가지 않으며, 여러 소스가 혼합된 페일오버는 사람이 직접 구성한 풀에서만 동작한다. `docs/specs/models-accounts.md`를 참고한다. |
| G-3 | Linux 우선(Wayland/X11). macOS/Windows 는 같은 코드베이스의 후속 타깃(플랫폼 추상화 계층을 처음부터 둔다). |
| G-4 | 로컬 우선. 기본값에서 외부로 나가는 데이터는 LLM 제공자 요청뿐. 텔레메트리 기본 OFF. |
| G-5 | 모든 클라이언트(CLI, MCP, 웹, 모바일, 채널봇)는 같은 공개 데몬 API 의 클라이언트다. 숨은 경로 없음. |

## A. 에이전트 코어
| ID | 기능 | openside 요구사항 / 수용 기준 (AC) | 기반 | 단계 |
| --- | --- | --- | --- | --- |
| A-1 | 코딩 에이전트. 핵심 도구는 JS REPL + bash. Playwright 문법의 코드를 모델이 작성 | 에이전트 기본 툴셋을 REPL(브라우저 prelude 포함)+bash+파일 도구로 제한한 `browser-agent` 프로필 제공. AC: 프로필 로드 시 노출 도구 이름 목록이 고정 스냅샷 테스트와 일치. `browser-agent` 프로필이 구현돼 있다. 노출 도구는 `ask_user`·`bash`·`browser_pointer`·`browser_repl`·`browser_screenshot`·`edit`·`read`·`request_human_step`·`write` 9개이며, 커밋된 스냅샷 테스트가 도구 목록과 `open` 페이로드를 검증한다(`docs/specs/profiles.md` §2). 기본 프로필 `full` 은 등록된 데몬 도구를 모두 노출한다 | 🟡 데몬 툴 + Pydantic AI function tools | M1 (구현됨) |
| A-2 | 시스템 프롬프트 + 도구 정의 합계 약 10K 토큰, AI 가 프롬프트를 쓰지 않음 | 프롬프트 토큰 예산을 CI 에서 측정. AC: 기본 툴셋의 `open.system_prompt_parts` + `open.tools` 합 ≤ 10K 토큰, 초과 시 CI 실패. `tools/gate.sh` 의 `prompt token budget` 단계가 `tools/token-budget.py` 로 `browser-agent`·`ultrabrowse` 를 재고 10K 토큰을 넘으면 실패시킨다. omp `toks` 대신 보수적 추정식을 쓰며(`docs/specs/profiles.md` §3), 현재 두 프로필 모두 한도의 절반 이하다 | 🔴 측정 하네스 | M1 (구현됨) |
| A-3 | Long-history compaction safety | Tool-free session-model summarization; default 64,000 estimated tokens (UTF-8 history bytes/3), normal tail 20, minimum tail 2 with complete tool pairs. Message-count trigger is opt-in. AC: goal survives a scripted 200-turn conversation; large old results trigger compaction; stale idle summaries cannot replace new owner input. See `agent/README.md`. | compaction | M3 |
| A-4 | Queue / Steer 후속 메시지, 큐 메시지 편집·재정렬, Cmd+Enter steer | 데몬이 세션별 영속 큐를 소유(`MessageQueue`). AC: ① steer 가 실행 중 턴에 주입(`user_message` mode `steer`, 다음 모델 요청 직전; `docs/08` §C) ② queue 는 현재 런의 작업이 끝난 뒤(`queue`) ③ 큐 항목 편집/삭제/steer 로 승격(`PATCH`·`DELETE /v1/sessions/{id}/queue/{mid}`, `POST …/promote`), `POST /v1/sessions/{id}/queue/{mid}/move`로 레인 내 `position` 재정렬(클램핑, `message.moved` 기록, 완료·미존재 시 409)을 지원한다 ④ 프로세스 재시작 후 큐 보존 ⑤ 중복 전송 0 (message id) | 🟡 사이드카 steer 버퍼 + 데몬 큐 영속화 | M1 (구현됨) |
| A-5 | 서브에이전트: task 안에서 그룹, 라이브 팔로우, 모델 선택, 승인이 부모로 올라옴, fork 는 완료된 컨텍스트만 상속 | AC: ① 서브에이전트 목록/진행/이벤트 스트림을 UI·API 로 제공 ② 서브에이전트별 모델 지정 ③ 서브에이전트의 승인·질문이 항상 부모 세션 승인함에 표시 ④ 개별 취소/steer ⑤ fork 시 미완료 tool-call batch 제외 테스트. BLOCKED-1(harness `SubAgents` 가 자식 보류를 부모로 올리지 못함)은 ADR-020 으로 해소했다. harness 대신 `delegate` 데몬 툴과 사이드카 자체 어댑터로 서브에이전트를 실행하며, ①~⑤ 가 모두 통과한다(`crates/openside-e2e/tests/subagents.rs`) | 🔴 자체 어댑터(ADR-020) | M3 (구현됨) |
| A-6 | 스킬: 커스텀 스킬 관리, 대화 중 스킬 생성, `/` 멘션, CLI를 통한 스킬 설치 | AC: ① 스킬 목록/보기/생성/편집/토글 API ② 채팅 입력에서 스킬 멘션(칩) ③ `openside skills install <agent>` 로 openside 브라우저 스킬을 Claude Code/Codex 등에 설치 ④ 잘못된 frontmatter 가 설정 화면을 깨지 않음(퍼징 테스트). harness `skills` 는 연결하지 않고(S-9 `later`, `docs/08` §E) 데몬이 직접 구현한다: `openside-skills` 크레이트, `/v1/skills` API, `skill` 도구, 프리셋 `claude`·`codex`·`omp` 의 `openside skills install <agent>`(`docs/specs/skills.md`) | 🔴 관리 API/설치기(harness skills 는 미연결) | M3 (구현됨) |
| A-7 | 훅, 샌드박스 | AC: 승인 게이트·시크릿 redaction 을 모델 호출 경로가 아니라 데몬 호출 파이프라인(`docs/09` §2)에서 강제하고, 사이드카는 Pydantic AI hook(`SessionHooks`)으로 이벤트·steer 를 처리. 샌드박스는 D-3 | 🟡 데몬 정책 + Pydantic AI hooks | M2 (구현됨) |
| A-8 | 모델 및 다중 계정 | 프로바이더 및 자격 증명 준비 상태별로 모델을 그룹화하고, 재시작 없이 API로 기본 모델과 fallback을 설정한다. 일반 및 History 세션은 저장된 모델을 유지하고, Team 홈은 다음 유휴 턴에 Agent 재정의나 현재 기본값을 적용하며, 진행 중인 요청은 캡처된 모델로 완료한다. 라벨이 지정된 N개의 API 키와 격리된 N개의 Claude Code/Codex CLI 계정을 관리하며, 계정별 활성화·삭제와 정확한 외부 로그인 명령을 지원한다. 순서가 지정된 각 프로바이더 풀은 사람이 직접 구성하며, 데몬이 담당하는 선택 로직은 스티키(sticky) 어피니티, 인증·속도·할당량(auth/rate/quota) 분류별 로테이션, 사용량 윈도우 및 리셋 시각을 활용한다. 읽기(read) 스코프의 모바일 기기는 키를 조회하거나 자격 증명·풀을 변경할 수 없으며, 변경 작업에는 Admin 권한과 사람 기기(person device) 또는 로컬 소켓이 필요하다. openside 자체 OAuth 로그인 흐름은 두지 않는다. | 소유자 승인을 거치는 다중 계정 전환 규약이며, 집중 검증 결과는 별도로 추적한다 | M1 |
| A-9 | Ultrabrowse (리서치 모드, 출처 인용) | 별도 `ultrabrowse` 프로필(더 높은 thinking, 계획 단계, 인용 강제). AC: 응답의 모든 사실 주장에 출처 URL 링크, 인용 없는 주장 비율 측정 harness. `ultrabrowse` 프로필이 구현돼 있다(`docs/specs/ultrabrowse.md`): 높은 추론, 계획 단계, 인용 강제(인용 없는 주장은 재시도 후 `[citation needed]` 로 표시). 인용 없는 주장 비율은 `tools/citation-harness.py` 로 측정한다 | 🟡 프로필+프롬프트 | M3 (구현됨) |
| A-10 | Routines: cron(새 태스크) / heartbeat(기존 채팅 재개), 겹침 스킵, 대상 소멸 시 일시정지, 제안 | AC: ① cron 표현식 스케줄 ② heartbeat = 기존 세션 큐에 메시지 주입 ③ 이전 실행 진행 중이면 스킵+기록 ④ 대상 세션 삭제 시 pause ⑤ "지금 실행" ⑥ 반복 작업 후보를 찾아 초안 생성. `openside-sched` 와 `/v1/routines` 로 구현돼 있다(`docs/specs/routines.md`). ⑥ 은 `GET /v1/routines/suggestions` 이며, 비슷한 사용자 메시지가 규칙적인 간격으로 3번 이상 반복되면 초안을 돌려준다 | 🔴 스케줄러 | M3 (구현됨) |
| A-11 | Memory: 브라우징 기록·채팅·태스크에서 로컬 임베딩 기억, 보존기간, 열람·편집·이력 | AC: ① 로컬 SQLite+벡터 검색 ② 보존 정책(영구/90일/30일) 적용 ③ 기억 CRUD + 변경 이력 ④ 네트워크로 나가는 임베딩 호출 0 (로컬 모델) ⑤ 시크릿 패턴이 기억에 저장되지 않음. harness `memory` 는 연결하지 않고(S-9 `later`) 데몬의 `openside-memory`(`memory.db`)로 직접 구현했다(`docs/specs/memory.md`). 벡터 검색은 로컬 정적 임베딩 모델(model2vec)이 설치돼 있을 때만 동작하고, 없으면 FTS5 어휘 검색(`lexical_only`)으로 대신한다. 모델은 `openside memory model fetch` 로 명시적으로 받으며, 이것이 유일한 네트워크 경로다(임베딩 계산은 네트워크를 쓰지 않는다) | 🔴 히스토리 인덱서 | M4 (구현됨) |
| A-12 | 태스크 산출물: 태스크 폴더에 transcript·파일 보관, 이미지/PDF/HTML/텍스트 미리보기 | AC: 태스크별 디렉토리, 생성/변경 파일 목록, 안전한 sandbox iframe 미리보기, 태스크 삭제 시 정리. 내부 관찰용 스크린샷은 출력 개수 및 파일 목록에서 제외하며, 명시적으로 전달된 스크린샷만 표시한다. 기존 스크린샷은 디스크에 유지하되 출력물로 노출하지 않는다. docs/specs/artifacts.md 참조. | 🟡 | M3 (구현됨) |
| A-13 | Channels: Slack/Telegram/Discord 원격 제어, 음성 메모 전사, `/stop` `/model` `/sessions` `/usage` `/new`, 승인 카드 | AC: 각 채널 어댑터가 데몬 API 의 클라이언트로 동작(별도 로직 없음). 스레드=세션 매핑, 승인 카드, 음성 전사(로컬 STT 옵션). `openside-channels` 로 구현돼 있고 가짜 Telegram·Slack·Discord 서버로 검증했다. 실제 플랫폼 계정으로는 확인하지 않았다(`docs/specs/channels.md`) | 🔴 어댑터 | M5 (구현됨) |
| A-14 | Gmail이나 Slack 메시지를 사이드패널 세션으로 전달한다 | 확장의 컨텍스트 메뉴 + 사이트별 버튼 주입 프레임워크. AC: Gmail/Slack 웹에서 선택한 메시지가 새 세션 컨텍스트로 열림 | 🔴 확장 | M5 (구현됨) |
| A-15 | 에이전트가 채팅에 차트·시각화를 인라인 렌더링 | AC: sandbox iframe 에서 HTML/SVG/차트 렌더링, 소스 보기 토글 | 🟡 UI | M4 (구현됨) |
| A-16 | 빠른 프론트 에이전트와 Task 분리. 모든 사용자 메시지를 빠른 모델이 먼저 받아 즉시 답하고 Task 를 관리하며, 오래 걸리는 작업은 강한 모델의 Task 워커가 병렬로 실행 | `pydantic-ai` 계층에 프론트 에이전트와 Task 워커를 도입하며, 사용자는 프론트 에이전트와만 대화한다. AC: ① 모든 사용자 메시지는 빠른 모델의 프론트 에이전트가 가장 먼저 처리하며, 메시지 수신 후 300 ms 이내에 상태 이벤트를 발행한다(스크립트 모델로 측정) ② 시간이 걸리는 작업은 강한 모델의 Task 워커로 실행하고 `create_task`·`steer_task`·`cancel_task`·`get_tasks` 도구로 제어하며, 프론트 에이전트의 첫 응답은 실행 중인 Task 가 끝나기 전에 도착한다 ③ 한 세션의 여러 Task 는 독립된 ID·상태·이벤트 스트림을 갖고 병렬로 실행되며, 특정 Task 를 취소하거나 실패해도 다른 Task 에 영향을 주지 않는다 ④ Task 완료는 데몬 재시작 이후를 포함해 사용자에게 정확히 1회만 전달된다 ⑤ Task 가 실행 중일 때 전달된 메시지는 프론트 에이전트가 즉시 답하거나 알맞은 Task 로 steer 한다. 해당 요구사항에 대한 구현이 반영되어 있으며, acceptance.csv에 명시된 테스트 스위트 검증 결과에 따라 통과 여부를 판정한다 | 프론트 에이전트 + Task 어댑터 | M8 (구현됨) |

Task 완료 인도의 정확히 한 번(exactly-once) 보장 대상은 영속화된 'Task-finished' 메시지 행(`task-report-<task_id>`)과 이벤트 로그에 보존되는 작업 출력(`subagent.ended.output`)이다. 이 두 데이터는 재시작 시점에도 한 번만 복원되며, Front 에이전트가 생성하는 요약 텍스트는 최선 노력(best-effort) 동작이므로 exactly-once 인도 대상에 해당하지 않는다. 리포트 주입(`MessageInjected`) 후 `assistant.final` 전에 사이드카가 종료되더라도 해당 리포트는 실패 또는 중단으로 안전하게 마킹되어 재실행되지 않으며, 영속화된 행과 작업 출력은 그대로 유지된다.

## B. 브라우저
| ID | 기능 | openside 요구사항 / 수용 기준 | 기반 | 단계 |
| --- | --- | --- | --- | --- |
| B-1 | 다른 최초 실행 설정보다 먼저 기존 브라우저 프로필을 모두 가져온다 | Chrome, Chromium, Brave, Edge, Vivaldi, Opera, Arc, Aside, Firefox 및 파생 브라우저와 허용된 환경의 Safari를 감지한다. GUI에서 방문 기록, 북마크, 쿠키, 비밀번호, 자동완성, 복원 탭, 검색엔진, 권한, 확장 프로그램 메타데이터의 항목 수를 미리 보여주며, 기본값으로 모든 프로필과 종류를 선택한다. 로컬 사람 권한 원칙을 따르며, 비밀번호는 설정 및 잠금 해제 후에만 암호화된 Vault로 가져오고 인메모리 CSV 대안도 지원한다. 복사된 원본 DB만 읽고, 암호화 실패는 종류별로 격리하며, 원본별 멱등한 쓰기와 작업 진행 상태의 영속적 저장을 보장한다. 쿠키는 에이전트 프로필로 전달되고, 다른 레코드는 프로필 범위 메타데이터를 유지한다. 플랫폼별 제한 사항과 검증 근거는 docs/specs/import.md를 참고한다. | 기존 가져오기 도구 | M5 |
| B-2 | Playwright와 완벽히 호환되는 얇은 CDP 래퍼. 비동기 이벤트는 steer로 전달한다 | AC: ① 모델이 쓰는 코드가 Playwright 문법 그대로 동작(호환 스위트 통과율 공개) ② 팝업/다운로드/탭닫힘 이벤트가 steer 로 주입 ③ 페이지 스크립트 허용: `page.evaluate`, `$eval`/`$$eval`, `locator.evaluate`, `addInitScript` 및 원시 `page.cdp`(ADR-022). `browser_repl`(QuickJS, ADR-021)과 `openside repl` 이 구현돼 있다. Playwright 호환 통과율은 `docs/specs/browser-repl.md` 에 공개하고, 공개한 수치가 계산값과 같은지 테스트가 확인한다. 팝업·다운로드·탭 닫힘은 소유 세션에 `steer` 로 주입한다. ③ 은 새로 추가된 기준이며 아직 구현되지 않았다(현재 REPL 은 페이지 스크립트를 주입하지 않으며 `evaluate*` 호출 시 `NotSupported` 예외를 던진다) | 🔴 자체 CDP 래퍼 | M1 (부분 구현) |
| B-3 | 개량 접근성 트리: ref ID, 중간 노드 제거, focus·iframe·clickable, diff, agent-browser 대비 70% 작게 | AC: 스냅샷 크기 벤치(고정 페이지 세트)에서 agent-browser 대비 ≤ 0.35x, 연속 호출은 diff 만 반환, ref 는 REPL locator 와 동일, truncation 금지 경고. `browser_snapshot` 은 `eN` ref, `since` diff, 잘라내지 않고 예산 초과를 경고하는 페이징을 제공하고, REPL(`page.snapshot()`, `aria-ref=eN`)이 같은 ref 를 쓴다. 크기 벤치(`bench/snapshot/`)는 여러 번 호출하는 작업 흐름 기준으로 agent-browser 대비 ≤ 0.35x 를 만족하며, 스냅샷 한 장만 비교하면 이 기준에 미치지 못한다(`bench/snapshot/README.md`) | 🔴 접근성 스냅샷 + diff | M1 (구현됨) |
| B-4 | CUA 폴백 (스크린샷+좌표) | AC: DOM 으로 못 쓰는 페이지에서 스크린샷·좌표 클릭 도구, 뷰포트 1440x900 고정 | 🟡 | M2 (구현됨) |
| B-5 | 백그라운드 탭: 포커스 안 뺏음, 팝업 없음, 뷰포트 1440x900 고정 | Stage A: CDP `Target.createTarget{background:true}`+`Emulation.setDeviceMetricsOverride`+`setFocusEmulationEnabled`(전용 창). Stage C: Chromium 패치로 완전 보장. AC: 에이전트 작업 중 활성 창이 변하지 않음을 자동 검증(데스크톱 환경의 창 관리자 IPC 로 확인하는, 환경 의존 선택 실행 테스트). Stage A(전용 창의 백그라운드 탭)와 Stage C(패치 Chromium P5, Linux)가 구현돼 있다. 컴포지터 IPC 로 확인하는 포커스 테스트는 IPC 에 닿지 못하면 사유를 남기고 건너뛴다 | 🔴 | M1(근사) / M7(완전) (구현됨) |
| B-6 | 봇 탐지 회피 (진짜 브라우저, Chromium 내부 패치) | Stage A: 실제 Chrome 프로필 사용으로 최대한 정상 지문 유지. Stage C: 지문 패치 시리즈. AC: 공개 봇 탐지 테스트 페이지 세트에서 점수 기록(합격 기준은 로드맵에 명시). Stage A 와 패치 빌드가 구현돼 있다. 점수 기록과 로컬 검사 페이지 세트는 `docs/specs/bot-detection.md` 에 있고, 합격 기준(로드맵 BR-2)은 패치 빌드가 P7 자동화 아티팩트 점검을 모두 통과하는 것이다(stock 모드는 기록만 한다) | 🔴 | M7 (구현됨) |
| B-7 | Agent Tabs 그룹, 탭 소유권(openedBy), 5개 이상 시 경고, 마지막 탭 닫힘 처리, 잠든(unloaded) 탭 처리 | AC: ① 에이전트가 연 탭만 닫을 수 있음(사용자 탭은 읽기 전용) ② 탭 ≥5 시 시스템 경고 ③ unloaded 탭에 attach 하지 않고 `TabAsleep` 반환(타임아웃 12초 이내) ④ 외부 에이전트(MCP/CLI)의 탭도 Agent Tabs 그룹으로 | 🟡 | M1 (구현됨) |
| B-8 | 브라우저 UX: 세로 탭, 분할 보기, 북마크 바/사이드바, 핀 탭, 탭 이름 변경, AI 탭 정리(Tidy), 프로필 전환, PiP, 사이드패널(Ask), 주소창 Ask AI(`Tab` 전환, `@` 컨텍스트), Mini popup | Stage A: 확장(`chrome.sidePanel`, `chrome.tabGroups`, omnibox keyword, 새 탭 페이지 override)로 구현 가능한 것부터. 브라우저 크롬 자체가 필요한 항목은 Stage C 패치. 항목별 AC 는 구현 시 `docs/specs/browser-ux.md` 로 분해. 항목별 현황은 그 문서에 기록하고 `tools/check-browser-ux.py` 가 검사한다. implemented: 핀 탭, PiP 미리보기, 사이드패널(Ask), 주소창 Ask AI, 명시적 `@` 컨텍스트, Agent Tabs, Handle with openside. needs-test(헤디드 검증 필요): 세로 탭, 분할 보기, 북마크 사이드바, 새 탭 Ask 페이지, 스케줄러 UI. not-implemented: 북마크 바, 탭 이름 변경, AI Tidy, 프로필 전환, Mini popup | 🔴 | M5~M7 (부분 구현) |
| B-9 | 클라우드 동기화(핀 탭, 볼트, 브라우저 데이터 복구) | **서버 없이** 동기화: 사용자 소유 저장소(자체 서버/S3/WebDAV/Syncthing 디렉토리)에 E2E 암호화 스냅샷. AC: 두 기기 간 핀 탭·볼트 항목 동기화, 서버는 평문 접근 불가, 충돌 해결 규칙 문서화. `openside-sync` 로 구현돼 있다(`docs/specs/sync.md`). 저장소는 WebDAV 공유와 디렉터리(Syncthing 등으로 동기화)만 지원하고 S3 백엔드는 없다. 동기화 대상은 핀 탭·Vault 항목·동기화 태그가 붙은 메모리이며, `sync.settings = true` 일 때만 정책도 동기화한다. 브라우저 프로필·기록·대화·스킬·`config.toml` 은 동기화하지 않으므로 브라우저 데이터 복구는 범위 밖이다 | 🔴 | M6 (구현됨) |
| B-10 | Default / Incognito 태스크 모드 (시크릿에서는 Vault 불가) | AC: incognito 태스크는 임시 프로필, 종료 시 완전 삭제, Vault 접근 거부 테스트 | 🟡 | M2 (구현됨) |
| B-11 | 세션별 격리 브라우저 공간 선택(`browser_space`: `shared` 또는 `isolated`, 기본값 `shared`) | 격리 공간은 관리형 Chromium 안에서 세션마다 만드는 독립된 CDP 브라우저 컨텍스트다. AC: ① 기본 `browser_space` 는 `shared`(로그인이 유지되는 관리형 프로필)이며, `isolated` 는 세션 생성 시점에만 지정할 수 있고(API, CLI `--browser-space`, MCP `session_create`, 웹 New chat 토글) 이후에는 변경할 수 없다 ② 격리 공간의 쿠키와 스토리지는 공유 프로필이나 다른 격리 세션에 노출되지 않는다 ③ 격리 공간은 세션을 보관·삭제할 때 파기되며, Chromium 재시작으로 유실되면 세션에 한 번 알린다 ④ 격리 공간 안에서도 `vault_login`·OTP·로그인 도구가 동작한다(Vault 접근을 거부하는 incognito 와 다르다) ⑤ 격리 공간에서 방문한 페이지는 메모리 히스토리에 색인하지 않는다. 기준 ①·②·④·⑤는 구현돼 있으나, 보관된 세션이 격리 브라우저 공간을 다시 생성할 수 있는 점, 파기 실패·취소 시 보관/삭제 성공으로 응답하면서 활성 컨텍스트가 추적되지 않은 채 남을 수 있는 점, 격리 공간에서 다운로드한 파일이 파기 후에도 공유 프로필 디렉터리에 남는 점 등 세 가지 결함이 검토에서 발견되어 기준 ③은 통과로 처리하지 않는다(`docs/09` §3, `docs/03`) | 🟡 CDP 브라우저 컨텍스트 | M8 (부분 구현) |
| B-12 | Shared browser workspace with per-Task tab ownership | Tasks share the session browser profile so a login persists across Tasks. Each tab has exactly one owning Task; only that Task may act on it. The front cannot inspect or act on tabs: browser reads and actions both return `front_agent_cannot_act`. | Task tab ownership | M8 |
| B-13 | 브라우저 상태를 구조화한 도구 UI: Task 별 타입 있는 `browser_state` 와 라이브 브라우저 카드 | 브라우저 상태는 모델의 서술이 아니라 타입이 있는 데이터로 전달한다. AC: ① 데몬은 Task 마다 `url`·`host`·`title`, `phase`(`loading`·`reading`·`acting`·`waiting_human`·`done`·`error`), 동작 문구, 대상 라벨, 페이지 종류(`login`·`otp`·`choice`·`captcha`·`form`·`list`·`article`·`checkout`·`error`·`other`), 핵심 정보, 썸네일을 담은 `browser_state` 를 이벤트와 API 로 발행한다 ② 동작 문구와 대상 라벨은 모델 출력이 아니라 페이지의 텍스트(접근성 이름, 제목)에서만 가져온다 ③ 웹 UI 는 Task 마다 상태 표시, 사람이 필요할 때 뜨는 `Action needed` 칩, 최소화, 확인을 거치는 취소, 완료 칩을 갖춘 라이브 브라우저 카드를 보여준다 ④ 세션 화면은 에이전트의 현재 동작을 보여주는 읽기 전용 라이브 브라우저 카드를 띄우고, 티켓 대기 중에는 `Action needed` 상태를 표시하며 최소화와 취소를 지원한다. 해당 요구사항에 대한 구현이 반영되어 있으며, acceptance.csv에 명시된 테스트 스위트 검증 결과에 따라 통과 여부를 판정한다 | 🟡 | M8 (구현됨) |
| B-14 | 스냅샷 뷰포트 범위(`scope: "viewport"`) | `browser_snapshot` 에 `scope: "viewport"` 를 주면 화면에 보이는 영역만 돌려준다. 상세 사양과 측정치는 `docs/specs/browser-snapshot.md` 에 있다. AC: ① `scope: "viewport"` 는 시각적 뷰포트와 겹치는 노드와 그 조상만 남기고, 남은 노드는 전체 캡처와 같은 ref 를 받는다 ② 뷰포트 캡처는 부분 뷰이므로 diff 기준으로 쓰이지 않고 `since` 는 무시된다 ③ 긴 페이지 픽스처 세 개에서 뷰포트 캡처 크기의 합은 전체 캡처의 50% 미만이다(실측 약 41%). 구현돼 있다 | 🟡 | M8 (구현됨) |
| B-15 | 브라우저 드라이버 동등성: 데몬의 CDP 드라이버 계층(`openside-browser`, REPL)을 성숙한 자동화 드라이버 수준으로 확장 | 별도 API 를 새로 두지 않고 기존 도구와 REPL 을 확장한다. 페이지 내부 일괄 실행을 허용한다(ADR-022). AC: ① 포인터·키보드 동작 전에 조작 가능 여부(보임·안정·활성·이벤트 수신)와 히트 타깃을 검사하고, 막혔으면 대상을 가린 요소를 보고한다 ② 포인터·키보드 입력은 IME 조합과 드래그를 포함해 신뢰되는 CDP 입력으로 보낸다 ③ JavaScript 대화상자·다운로드·팝업이 떠도 탭이 멈추지 않는다 ④ 다른 프로세스의 프레임(OOPIF)과 shadow DOM 안의 요소도 스냅샷 ref 와 동작으로 다룰 수 있다 ⑤ 페이지 로드·네트워크 유휴·요소 상태를 기다리는 동작이 있고, 제한 시간과 명확한 오류를 돌려준다 ⑥ 같은 문서의 스냅샷 사이에서 ref 가 안정적으로 유지된다 ⑦ 단일 `browser_repl` 호출 안에서 여러 드라이버 동작(스냅샷·클릭·입력·대기·이동)과 페이지 스크립트를 한 번에 묶어 실행할 수 있다(ADR-022). ③ 과 ⑥ 은 지금도 통과한다(대화상자·팝업·다운로드는 B-2·`browser_flows`, 안정적인 ref 는 B-3). 나머지는 아직 구현돼 있지 않다 | 🟡 | M8 (부분 구현) |
| B-16 | 지연 시간: 브라우저와 에이전트 사전 준비(prewarm), 콜드·웜 수치 기록 | 사전 준비가 포커스를 바꾸거나 창을 띄우면 안 된다(B-5). AC: ① 세션을 만들 때와 입력창에 포커스가 갈 때 Chromium 과 에이전트 탭 하나를 미리 준비하고, 첫 메시지가 오기 전에 사이드카도 이미 준비돼 있다 ② 콜드·웜 시간(세션 생성부터 쓸 수 있는 브라우저 탭까지, 첫 응답 토큰까지)을 반복 가능한 명령으로 재서 커밋된 결과 파일에 기록한다 ③ 사전 준비는 활성 창을 바꾸지 않고, 쓰지 않은 사전 준비 탭은 정리한다. 해당 요구사항에 대한 구현이 반영되어 있으며, acceptance.csv에 명시된 테스트 스위트 검증 결과에 따라 통과 여부를 판정한다 | 🟡 | M8 (구현됨) |

## C. Vault (에이전트용 패스워드 매니저)
| ID | 기능 | openside 요구사항 / AC | 기반 | 단계 |
| --- | --- | --- | --- | --- |
| C-1 | 자동 완성을 통한 자격 증명 주입, 도구 결과에서 마스킹 | `vault_login`/`vault_otp` 데몬 툴(`docs/09` §3). 데몬이 CDP를 통해 비밀값을 직접 입력하며 도구 결과, 이벤트, 로그, 스냅샷, 스크린샷, transcript에서 redaction 처리한다. AC: 해당 영역 전체를 대상으로 canary 비밀값 문자열을 grep 검색했을 때 일치 항목이 0건이어야 한다(자동화 테스트). 단, 에이전트가 실행한 페이지 스크립트는 입력된 필드를 포함해 페이지 데이터를 직접 읽을 수 있으며, 값을 변형해 반환하면 redaction을 우회할 수 있다(ADR-022) | 🟡 데몬 툴 | M2 (구현됨) |
| C-2 | passkey / FIDO2 / TOTP | TOTP 로컬 생성. passkey/FIDO2 는 소프트웨어 authenticator(CDP WebAuthn virtual authenticator) 우선, 하드웨어 키는 사용자 개입. AC: 공개 WebAuthn 테스트 페이지 등록·로그인 성공. `vault_passkey` 도구로 패스키 등록·로그인을 구현했고, 공개 테스트 페이지 대신 로컬 mock relying party 로 검증한다 | 🔴 | M2(TOTP) / M6(passkey) (구현됨) |
| C-3 | 접근 정책 Always / While unlocked / Never, 항목별 override | AC: 정책 위반 시 도구가 거부하고 감사 로그 기록 | 🔴 | M2 (구현됨) |
| C-4 | 사용 감사 로그 (언제·무엇·왜) | AC: 모든 get/fill/otp 호출에 task id·origin·시간·결과 기록, UI 에서 조회, 시크릿 값은 기록하지 않음 | 🔴 | M2 (구현됨) |
| C-5 | origin 검증 | AC: 저장된 도메인과 현재 페이지 origin 불일치 시 채우기 거부(리다이렉트·피싱 시나리오 테스트) | 🔴 | M2 (구현됨) |
| C-6 | 하드웨어 기반 잠금 해제 (Secure Enclave / Touch ID / Windows Hello) | Linux: TPM2(systemd-creds/tpm2-tss)·libsecret·FIDO2 hmac-secret 중 선택, 플랫폼 추상화 trait. AC: 마스터 키가 디스크에 평문으로 없음. Linux 는 Secret Service 백엔드(키링에 래핑 키 보관)와 passphrase 백엔드가 구현돼 있고 TPM2·FIDO2 hmac-secret 은 없다. Windows `native` 키링 백엔드는 cfg 뒤에서 컴파일만 되며 검증하지 못했다. macOS `native` 백엔드는 x86_64 Tahoe VM 네이티브 검증(2026-10-13, `docs/PLATFORMS.md`)을 통과했다(`docs/specs/vault.md` §2) | 🔴 | M6 (구현됨) |
| C-7 | 서드파티 PM 확장 호환 | 표준 Chromium 확장 + native messaging 이 그대로 동작해야 함(→PP-19). AC: 1Password/Bitwarden 확장 설치·동작 확인. Bitwarden 은 순정·패치 Chromium 에서 설치·로그인·자동 완성을 확인했다(`tools/pwmgr-qa.sh all`). 1Password 는 로그인에 계정과 데스크톱 앱이 필요해 설치와 로드만 확인했고, 생체 인증이 필요한 Bitwarden 데스크톱 앱 연동도 시험하지 않았다 | 🟡 | M5 (부분 구현) |
| C-8 | 결제/게시/메시지 등 민감 행동 사람 승인 | → D-4 | | M2 (구현됨) |
| C-9 | 안전한 로그인 폼 도구 `request_login` 과 페이지 선택지 도구 `request_choice` | 실제 페이지 양식을 바탕으로 생성된 카드에서 사용자가 로그인하면, 데몬이 입력값을 페이지에 대신 채워 넣고 도구 결과에서는 마스킹한다. AC: ① `request_login` 은 페이지의 실제 로그인 폼(필드 라벨·종류, 페이지가 제공하는 다른 로그인 수단)으로 입력 카드를 만든다 ② Vault 를 먼저 조회해, 그 origin 에 맞는 저장된 로그인이 하나면 묻지 않고 로그인하고 여럿이면 고르게 한다 ③ 입력값이 이벤트, 로그, 메모리, transcript에 남지 않고 도구 결과에서 마스킹된다(canary 테스트). 단, 에이전트가 실행한 페이지 스크립트는 페이지 데이터를 직접 읽을 수 있다(ADR-022) ④ 로그인 저장은 `human_step` 과 `approve` 범위를 모두 가진 기기의 응답이 있어야 하며 `login_save` 로 감사에 남는다 ⑤ 제출 뒤 다음 단계를 `done`·`otp`·`choice`·`captcha`·`error`·`unknown` 으로 분류한다 ⑥ `request_choice` 는 페이지가 쓰는 라벨 그대로 선택지를 보여주고, 고르려면 `approve` 범위가 필요하며 데몬이 선택한 항목을 클릭한다 ⑦ 채팅 채널은 로그인을 웹 UI 로 가는 링크로만 안내하고 선택지는 버튼으로 답할 수 있다. ①~⑥ 은 구현·테스트돼 있다. ⑦ 은 `openside-channels` 에 선택지 버튼은 있지만 로그인 링크를 검증하는 테스트가 없다 | 🟡 데몬 툴 | M8 (부분 구현) |
| C-10 | 로그인 시나리오: 사용자가 채팅 카드로만 응답하면서 에이전트가 각 로그인을 완결한다. | 로컬 루프백에서 여러 페이지로 구성된 실제 로그인 환경을 제공하는 픽스처 서버(사이트별 독립 상태 관리, 상태 확인용 `/__fixture/state`, 사용자 휴대폰 역할을 흉내 내는 `/__fixture/push/<verdict>` 엔드포인트 포함)를 띄워 CI에서 검증한다. 사용자가 에이전트의 브라우저를 직접 조작할 필요는 없으며, 브라우저 제어권을 넘겨받지 않고 모든 입력을 채팅의 티켓 카드로만 처리한다. AC: ① S1 이메일·비밀번호 로그인, S7 잘못된 비밀번호 입력 시 재시도 카드에 사이트 오류 메시지 노출, S10 카드의 "볼트에 저장" 선택 시 로그인 정보를 보관하고 다음 로그인부터는 카드 없이 볼트 정보로 자동 입력 ② S2 사용자명 입력 후 비밀번호를 입력하는 2단계 페이지는 카드를 두 번 띄우며, 비밀번호 카드에서도 계정 정보를 함께 저장 가능 ③ S3 로그인 카드의 "<provider>(으)로 계속하기" 버튼 지원 및 제공자 측 계정 선택·동의 페이지의 선택지 카드 처리 ④ S4 사이트 대기 상태에 맞춰 `Approve on your phone` 카드가 실시간 대기하고, 승인 시 로그인 진행, 거부나 시간 초과 시 사이트 오류 메시지로 반환 ⑤ S5 `request_otp` 카드로 SMS 인증번호와 OTP 앱 인증번호 입력 ⑥ S6 인증 방식, 계정·프로필, "이 기기를 신뢰합니까?", 약관 동의를 각각 `request_choice` 카드로 처리 ⑦ S8 사이트가 인증기 등록을 제안하면 볼트 저장 여부를 카드로 묻고, 승인했을 때만 `register_authenticator`가 정보를 저장한 뒤 페이지에서 인증번호 확인 절차 완료 ⑧ S9 이후 동일 사이트 로그인 시 카드 표시 없이 볼트 정보로 인증번호 자동 입력 ⑨ 비밀번호, 인증번호, 등록 키가 대화 기록, 이벤트 로그, 어떤 파일에도 남지 않음 (crates/openside-e2e/tests/login_scenarios.rs) | 🟡 데몬 툴 | M8 (구현됨) |
| C-11 | 에이전트가 로그인한 사이트 관리 | 로그인 메타데이터(사이트, 마스킹된 계정, 유효 상태, 출처, 최근 사용 시각, 에이전트별 권한 부여)만 저장한다. 사이트별 어댑터 없이 완료된 login·Vault·OTP·takeover·import 흐름을 감지하며, 불확실한 후보는 별도로 분리한다. chief는 기본 허용되고 teammate는 확인을 요청하며, 항상 승인은 지정된 teammate에게만 허용된다. 인증된 동일 출처(same-origin) fetch는 쿠키와 헤더 값을 API, 도구, 이벤트, 감사 로그에 절대 노출하지 않는다. revoke는 해당 사이트의 모든 공유 프로필 계정을 로그아웃하고 모든 권한을 삭제하지만, forget-only는 공유 로그인 상태를 유지한다. AC: 로컬 Login 티켓으로 활성 행이 생성되고, teammate가 사용할 때 확인을 요청하며, 사람의 항상 승인으로 GET이 허용되고, 로그아웃 시 쿠키와 스토리지가 삭제되어 이후 사용 시 찾을 수 없음(not found)으로 처리된다. | `docs/specs/connections.md`; 격리된 Connections e2e 테스트 | M2 |
| C-12 | 별도 계정 공간 분리 | 사이트 자체 계정 전환 기능을 기본값으로 유지한다. 사람은 추가 계정당 하나의 영속 프로필을 선택(opt-in)하여 사용할 수 있으며, 필요할 때 실행되고 유휴 상태가 되면 닫히며 데몬 재시작 시에는 복원되지 않는다. 사이트당 한 번에 하나의 별도 공간만 실행된다. 전환 시 이전 공간을 닫고 공유 프로필은 전혀 건드리지 않는다. RAM, 콜드 스타트, 디스크, 별도 로그인·2FA 비용을 문서화한다. 두 revoke 모드 모두 해당 별도 프로필을 중지한 후 삭제한다. | `docs/specs/connections.md`; 계정 공간 데몬/브라우저 회귀 테스트 | M2 |

## D. 권한 / 보안 / 프라이버시
| ID | 기능 | openside 요구사항 / AC | 기반 | 단계 |
| --- | --- | --- | --- | --- |
| D-1 | 세션 모드 Read only / Guard(기본) / Full access | `openside-policy` 크레이트. 설정 `[policy] mode = "read-only" \| "guard" \| "full"`. AC: 모드별 허용 매트릭스 테스트, Guard 기본값 | 🟡 데몬 정책 | M2 (구현됨) |
| D-2 | 규칙 Allow / Ask / Deny, Deny 우선, 에이전트 단위+세션 단위 | AC: 우선순위 속성 테스트(Deny>Ask>Allow, 세션 규칙이 에이전트 기본값 위에 적용) — 이 저장소에 이미 구현·테스트 존재 | 🔴 | M0 (구현됨) |
| D-3 | OS 수준 sandbox | Linux: bubblewrap(`--unshare-all`) 또는 Landlock. 경로는 canonical 비교, symlink 탈출 차단. AC: 샌드박스 밖 경로 읽기/쓰기 시도 시 거부(탈출 테스트 스위트). 현재 데몬 `bash` 가 bwrap 을 직접 실행하고, bwrap 실행이 실패하면 샌드박스 없이 다시 실행하지 않는다. bwrap 이 없으면 `sandbox.required=false` 일 때만 경고(`sandbox_unavailable`)와 함께 정책 전용 격리로 실행하고 `true` 면 `bash` 를 숨긴다(`docs/09` §7) | 🟡 데몬 `bash` + 외부 sandbox | M2 (구현됨) |
| D-4 | 결제·게시·메시지는 항상 사람 승인 | AC: 승인은 API 객체(`id`, `summary`, 대상 오리진, 만료 시각)로 다룬다. 승인 응답은 `approve` 스코프를 가진 페어링된 `person_device`에서만 할 수 있으며 웹·모바일·확장 프로그램 및 사람에게 연결된 채널에서 응답할 수 있다. 에이전트 클라이언트와 로컬 소켓은 거절이나 취소만 할 수 있고 승인할 수 없다. 응답하지 않은 요청은 안전하게 대기하다 만료된다. 프롬프트 지침이 아니라 데몬 호출 파이프라인에서 강제한다(민감한 작업은 자동 허용되지 않음, `docs/09` 4절). | 🟡 | M2 (구현됨) |
| D-5 | 로컬 우선 데이터 | AC: 데이터 디렉토리 문서화, 외부 전송 목록(허용 도메인) 자동 검사, 텔레메트리 기본 OFF | | M0 (구현됨) |
| D-6 | 비밀번호 직접 조회 불가(Full access 모드에서도 Vault는 값을 반환하지 않음) | AC: Full access 모드에서도 Vault가 값을 직접 반환하는 도구나 API 경로는 없다(테스트). 에이전트가 실행한 페이지 스크립트로 이미 입력된 값을 읽는 동작은 ADR-022에 따라 허용된다 | | M2 (구현됨) |

## E. 개발자 표면
| ID | 기능 | openside 요구사항 / AC | 기반 | 단계 |
| --- | --- | --- | --- | --- |
| E-1 | CLI 태스크, 모델 및 다중 계정 | 기존 task/session 및 비-TTY(non-TTY) JSONL 명령어는 유지된다. `openside accounts`는 라벨이 지정된 계정, 사용량, 차단 상태를 표시하고, `openside models`는 카탈로그 및 풀 준비 상태를 표시한다. `key set <provider> --label <label>`은 숨김 처리된 표준 입력(stdin)으로만 값을 읽으며 `--key-id`로 키 하나를 교체한다. `accounts add claude-code/codex-cli`는 격리된 홈을 생성하고 해당 로그인 명령을 출력한다. 계정별 활성화·비활성화·삭제, 풀, 세션 선호도 설정은 CLI에서도 동일하게 지원한다. 키와 토큰은 argv, 출력, 로그에 절대 나타나지 않는다. | 다중 계정 규약이 적용되었으며, 검증 결과는 별도로 추적한다 | M1 |
| E-2 | MCP 서버 | `openside mcp`: 브라우저 도구 노출. AC: Claude Code/Codex 등에서 `mcp.json` 로 연결해 탭 열기·스냅샷. 구현돼 있다(`docs/specs/mcp.md`). e2e 는 `mcp.json` 항목과 같은 stdio 전송으로 rmcp 클라이언트를 연결해 검증했고, Claude Code·Codex 자체는 실행하지 않았다 | 🔴 | M2 (구현됨) |
| E-3 | REPL | `openside repl` 결정적 브라우저 제어. AC: 에이전트가 쓰는 스냅샷·ref 와 동일 | 🟡 | M1 (구현됨) |
| E-4 | 퍼블릭 HTTP/WS API 및 SDK | OpenAPI 스펙+JSON Schema 에서 Rust/TS 클라이언트 생성. AC: 모든 UI 기능이 API 호출로만 구현됨. OpenAPI 3.1 스냅샷 `docs/specs/openapi.json` 은 `openside-proto` 의 라우트 표 `ROUTES` 에서 생성하고, 웹 UI 는 이 스냅샷에서 TS 타입을 생성한다(`pnpm -C web gen:api`). Rust 클라이언트(CLI·채널·MCP)는 `openside-proto` 타입을 직접 공유하며 따로 생성한 Rust 클라이언트는 없다 | 🔴 | M1 (구현됨) |
| E-5 | 벤치마크 | `openside-bench`: Online-Mind2Web/BU-Bench/Odysseys 러너 (BU Bench V1 결과 형식 호환). AC: 결과 JSON 이 BU Bench V1 결과 스키마로 출력. `bench/` 에 로컬 루프백 합성 태스크 5개(폼 입력, 3페이지 탐색, 표 추출, 2FA 가짜 로그인, 파일 다운로드)가 있고 픽스처 상태로 채점해 BU Bench V1 결과 스키마로 결과를 쓴다. 공개 Online-Mind2Web·BU Bench·Odysseys 데이터셋 러너는 없고, 실제 모델(live) 모드는 문서만 있을 뿐 실행하지 않았다. `openside-bench` 라는 크레이트도 없다 | 🔴 | M6 (부분 구현) |
| E-6 | 외부 에이전트의 CLI 및 MCP 연동 | 코딩 어시스턴트 같은 외부 에이전트가 CLI나 MCP를 통해 전체 작업을 openside 에이전트에 위임하거나 브라우저를 직접 제어할 수 있다. AC: ① 위임 실행: CLI에서 `openside run --follow`, steer, 취소, 작업 목록 조회를 지원하고, MCP에서 `agent_run`, `agent_status`, `agent_steer`, `agent_cancel`, `agent_result` 도구를 제공한다 ② 앱 내부 브라우저 도구는 모두 `openside browser <verb>` CLI와 동등한 MCP 도구로 직접 제어할 수 있다 ③ `openside repl`은 표준 입력(stdin)으로 코드 조각 여러 개를 일괄 실행할 수 있다 ④ 위임 실행 중 발행된 티켓은 웹 UI와 모바일 화면에 즉시 표시되며, CLI와 MCP에서 승인 대기할 수 있다 ⑤ 설치된 스킬에서 작업을 위임할 상황과 브라우저를 직접 제어할 상황을 구분해 안내한다. 동일한 데몬 퍼블릭 API를 통해 CLI 및 MCP 위임과 브라우저 직접 제어를 구현했다. 실제 프로세스 기반 인수 테스트는 외부 호출자의 승인 티켓이 데스크톱 및 390x844 모바일 UI에 실시간으로 표시되고, 사용자가 승인하면 대기 중이던 CLI/MCP 호출이 재개되는 동작을 검증한다. 설치된 스킬은 작업을 위임할 시점과 직접 브라우저를 구동할 시점을 구분한다. 관련 검증 코드는 `crates/openside-e2e/tests/mcp.rs` 및 `crates/openside-e2e/tests/skills.rs`에 작성했다. | 🟡 | M8 (구현됨) |
| E-7 | 원격 CLI 및 MCP 접속 | 다른 머신에서 실행 중인 데몬에도 CLI와 MCP로 원격 접속해 동일한 에이전트 및 브라우저 기능을 쓸 수 있다. AC: ① `openside connect <pairing link>`로 원격 데몬 정보를 등록하고, `--remote`, `OPENSIDE_REMOTE`, `openside mcp --remote`로 대상을 지정한다 ② CLI와 MCP가 릴레이, Tailscale, Cloudflare Tunnel을 통해 원격 데몬에 접속해 에이전트 및 브라우저 기능을 그대로 사용한다 ③ 원격 연결 시 발급된 토큰 스코프에 맞춰 권한을 제한한다 ④ 관리자는 사전 검토된 동작 관련 설정만 원격으로 변경할 수 있다. 보안에 민감한 설정 변경, 보안 정책 수정, 세션 모드나 워크스페이스 권한 확대는 로컬 소켓을 통해서만 허용한다. `connect` 페어링 링크로 원격 프로필을 저장하며, `--remote`, `OPENSIDE_REMOTE`, `openside mcp --remote`로 해당 프로필을 지정한다. `crates/openside-e2e/tests/mcp_remote.rs`의 테스트는 실제 릴레이와 루프백 TCP(실제 외부 서비스 대신 Tailscale/Cloudflare HTTPS 프런트 역할을 대신하도록 구성)를 거쳐 CLI/MCP 에이전트 위임, 브라우저 스냅샷, 스코프, 로컬 전용 보안 설정 변경을 검증한다. Cloudflare 관리 감독 및 페어링 동작은 `crates/opensided/src/tests/remote_transport.rs`에서 별도로 검증하며, 실제 외부 Tailscale/Cloudflare 환경에 직접 연결하여 검증하지는 않는다. | 🟡 | M8: 구현 완료. 실제 외부 Tailscale/Cloudflare 연결은 아직 검증하지 않음. |
| E-8 | 표준 CLI 및 API 기능 동등성 | `openside connections`는 정확한 사이트 필터를 지원하는 list, show, check, relogin, 선택적 keep-signed-in 플래그를 갖춘 revoke, 위치 인자 기반 에이전트별 권한 부여, connection·agent·action 결합 감사 필터, 별도 공간 add·remove를 지원한다. JSONL은 기존 요약과 함께 표준 API 객체를 출력하며, 204 응답 시 임의의 객체를 꾸며내지 않는다. needs_you 상태의 relogin은 티켓과 엔드포인트 보존 티켓 해결 힌트를 stderr로 출력하고, signed-in이 아닌 waiting_human을 보고한다. | `crates/openside-cli/src/tests/connections.rs`; `crates/openside-e2e/tests/connections.rs` | M1 |

## F. 원격 / 모바일
| ID | 기능 | AC | 단계 |
| --- | --- | --- | --- |
| F-1 | 모바일 웹/PWA: 대화 목록·검색·고정·보관, 라이브 스트리밍, 도구 활동 접기, 큐 편집·steer, 모델 선택, 브라우저 PiP 미리보기, 이미지 줌, 첨부 업로드 | 기능 목록을 체크리스트 형태로 관리한다(`docs/specs/remote-ui.md`에 세부 항목 정리). | M4 (구현됨) |
| F-2 | Web Push (VAPID): 완료/실패/승인 필요, 포커스된 기기 제외, 알림 클릭 시 해당 대화 | AC: iOS 홈 화면 앱/Android/Desktop 에서 수신. Desktop 은 순정 Chromium 에서 실제 Google 푸시 서비스로 수신을 확인했다(서비스 워커 알림 테스트 통과). iOS·Android 는 실기기가 필요하다(사용자가 PWA 를 설치하고 페어링·구독해야 한다). 패치 Chromium 은 GCM 키가 없어 푸시 구독이 실패한다 | M4 (blocked-external) |
| F-3 | 인증: 로컬 loopback 기본, QR 페어링(3분 만료), 기기별 토큰, 폐기 | AC: 토큰 스코프(read/steer/approve/admin) | M4 (구현됨) |
| F-4 | 원격 접속: Tailscale Serve 가이드, 선택적 reverse proxy | 문서+doctor 점검. Cloudflare Tunnel, 자체 릴레이, 통합 페어링은 F-8에서 확장한다. | M4 (구현됨) |
| F-5 | 웹 UI 첫 실행 온보딩 | AC: ① 첫 실행 때 웹 UI 가 설치된 에이전트(`claude`·`codex`·`omp` 프리셋)를 감지한다 ② 감지된 에이전트에 openside 브라우저 스킬을 설치할 수 있게 한다(`openside skills install <agent>`) ③ Chrome·Chromium·Firefox 프로필 가져오기를 제공하고, 쓰기 전에 dry-run 미리보기를 보여준다 ④ Vault 설정으로 안내한다 ⑤ 모든 단계는 데몬의 공개 API 엔드포인트만으로 동작한다. 첫 실행 안내는 `/welcome` 화면(에이전트 감지, dry-run 미리보기를 포함한 브라우저 데이터 가져오기, Vault 설정, 휴대폰 페어링; 건너뛰기 가능, Settings에서 다시 열기 지원, 데몬에 완료 플래그 보관)으로 구현되어 있으며 동작 방식은 `docs/specs/onboarding.md`에 정리되어 있다. 이제 안내 가이드는 이 컴퓨터에 이미 설치된 브라우저에서 모든 항목을 가져오는 과정(인라인 Vault 설정, 진행 상황, 요약)부터 시작하여 모델, 에이전트, 휴대폰 순으로 진행한다. 웹 UI에서는 사람에게 CLI 명령어를 실행하라는 안내를 일절 표시하지 않으며(명령어는 보조 힌트로만 제공), admin 스코프가 없는 기기는 화면에서 접근 권한을 요청하고 컴퓨터에서 승인받도록 한다(onboarding.md 1, 5, 8절). | M8 (구현됨) |
| F-6 | 웹 UI·PWA·확장이 함께 쓰는 글래스 시각 체계 | AC: ① 웹 UI·PWA·확장은 그라디언트 배경, 반투명 흐림 처리한 헤더와 입력창, 불투명도가 높은 글래스 어시스턴트 말풍선과 채팅 안 카드(흰색 기본 알약 버튼), 잠깐 나타나는 상태 알약으로 이루어진 하나의 글래스 시각 체계를 쓴다 ② 320~1440 px 의 모든 너비에서 가로 스크롤이 생기지 않는다(`web_overflow` e2e 테스트). 그라데이션 배경, 블러 처리된 헤더와 입력창, 고불투명도 유리 말풍선과 기본 흰색 알약 버튼을 포함한 카드, 일시 상태 알약으로 이루어진 글래스 시각 시스템은 웹 앱, PWA, 확장 프로그램에 적용되어 있으며 디자인 토큰과 규칙은 `web/DESIGN.md`에 정리되어 있다. `web_overflow` e2e 테스트는 1440x900, 390x844, 최소 지원 너비인 320x640을 검증하지만 대화 1건과 모바일 서랍만 다루며, 설정, 워크스페이스, 새 대화, `/welcome`, 코드 요청 독, 카드 등 다른 화면은 모든 너비에서 확인하지 않는다 | M8 (부분 구현) |
| F-7 | 네이티브 수준의 PWA 사용성 | 설치형 PWA가 모바일 앱처럼 자연스럽게 동작한다(외부 공유 수신, 바로가기, 배지, 제스처, 오프라인 전송). AC: ① Web Share Target: PWA로 텍스트, URL, 파일을 공유하면 해당 내용을 담아 새 대화 초안을 시작한다 ② 매니페스트 바로가기에 'New chat'과 'Needs you' 항목을 제공한다 ③ 앱 아이콘 배지에 대기 중인 티켓 수를 표시하고, 처리할 티켓이 없으면 배지를 지운다 ④ 안전 영역(safe area) 여백을 지켜 노치나 홈 바에 화면 요소가 가려지지 않는다 ⑤ 바텀시트는 아래로 끌어내려 닫을 수 있다 ⑥ 화면 가장자리를 쓸어 넘기면 사이드바가 열린다 ⑦ Android 환경에서 티켓을 승인할 때 햅틱 피드백을 제공한다 ⑧ 오프라인에서 작성한 메시지는 IndexedDB 보관함에 저장해 두고, 네트워크가 복구되면 중복 없이 전송한다 ⑨ 설치 안내: Android에는 설치 버튼을 띄우고 iOS에는 최초 1회 힌트를 표시한다. | M8 (구현됨) |
| F-8 | 원격 전송 방식 지원 (Tailscale, Cloudflare Tunnel, 자체 릴레이) | Tailscale Serve(F-4) 외에도 Cloudflare Tunnel이나 인바운드 포트 개방이 필요 없는 자체 릴레이를 통해 스마트폰에서 데몬에 접속할 수 있다. AC: ① Tailscale Serve 전송 방식을 그대로 유지한다 (F-4) ② Cloudflare Tunnel: 데몬이 `cloudflared` 프로세스를 관리하며, 퀵 터널과 네임드 터널을 모두 지원한다 ③ 자체 호스팅 릴레이 서버: 데몬이 외부로 직접 연결하므로 인바운드 포트를 열지 않아도 된다. CLI와 MCP의 릴레이 연결은 Noise 프로토콜로 종단간 암호화(E2EE)되며 페어링 QR 코드나 링크에 포함된 데몬 공개키를 고정(pinning)하여 검증한다. 릴레이에서 내려받은 PWA는 수동적 도청자(passive observer)로부터 트래픽을 보호하지만 HTML과 JS를 제공하는 릴레이 운영자를 신뢰해야 하므로, 자체 호스팅 릴레이를 구축하거나 릴레이 대신 데몬 또는 Tailscale 오리진에 직접 연결하여 PWA를 불러오는 구성을 권장한다. ④ `[remote] mode` 설정값, `openside remote setup <mode>` 명령어, 모드별 진단(doctor) 검사를 제공한다 ⑤ 페어링 QR 코드나 링크 하나에 전송 방식, 접속 주소(또는 릴레이), 데몬 공개키를 모두 담는다. AC ⑥ 실시간 이벤트 전달은 스트리밍이 지원될 때 SSE를 사용하고, Quick Tunnel을 비롯해 전송 구간에서 SSE를 버퍼링할 때는 단독 시퀀스 JSON 롱 폴링을 사용한다. 제어권 인수와 화면 중계는 계속 지원한다. 증적에서는 가짜 cloudflared 감시와 실제 릴레이 및 등록된 공개 Quick 라우팅을 구분한다. MobileE2E는 매회 헤드리스 Chromium에서 iPhone과 Android를 에뮬레이션하여 전체 릴레이 흐름 2회와 실제 Quick 흐름 2회를 통과했다: 데스크톱 QR 픽셀, 페어링, 새 대화, 동일 태스크 내 사람 승인, PiP 픽셀, 사진/모델 SHA, 배지 API 호출, 실제 서비스 워커 알림 처리를 확인했다. Quick은 사람의 확인을 기다리며 멈춘 시점에 events/poll 전달이 정상 완료되어야 하고 새로고침 없이 터치로 재개해야 한다. 원본 SSE나 토큰 단위 Quick DOM 스트리밍을 지원한다고 보장하지 않는다. 물리 기기 설치, 카메라 스캔, OS 배지, WebKit, GCM/APNs, 실제 Tailscale 라우팅, 네임드 Cloudflare 계정 연동은 미검증 상태로 남는다. E2EE는 네트워크 전송만 보호하며 릴레이가 제공한 PWA 코드 무결성은 보호하지 않는다. 신뢰 경계는 docs/specs/threat-model.md를 따른다. | M8 (구현됨) |

## B-16: 콜드·웜 수치 기록

B16 성능 측정은 동일한 스크립트 데몬과 헤드리스 Chromium 기반의 격리된 픽스처 환경에서 기준 단일 모드(baseline single), 프론트 모드(front), 프론트 사전 예열 모드(front prewarm)를 비교하며, 실제 LLM API 호출 없이 진행한다. 콜드 및 웜 측정값에는 사이드카 기동 시간, 브라우저 탭 준비 시간, 메시지 전송 시점부터의 첫 어시스턴트 토큰 도달 시간, 그리고 예열에 든 준비 시간이 가려지지 않도록 세션 생성 시점부터의 첫 토큰 도달 시간을 모두 포함한다.

`docs/specs/responsive-results.json`: 단일 실행 베이스라인 6/6, 프런트 프리웜 미적용 6/6, 프런트 프리웜 적용 6/6. 아래 표의 수치는 모두 밀리초 단위다.

| 케이스 | 단일 실행 변경 전: 첫 토큰 / 첫 내비게이션 완료 | 프런트 프리웜 미적용 | 프런트 프리웜 적용 | 전송 전 프리웜 선행 시간 |
| --- | ---: | ---: | ---: | ---: |
| 콜드 데몬/브라우저 | 11815 / 11805 | 1006 / 1345 | 30 / 106 | 898 |
| 웜 브라우저, 새 세션 | 2325 / 2314 | 1016 / 1211 | 30 / 95 | 888 |
| 웜 사이드카, 동일 세션 | 1 / 161 (기존 레코더) | 25 / 175 | 95 / 183 | — |

| 새 세션 | 프런트 프리웜 미적용: 시작 / 생성부터 첫 토큰까지 / 생성부터 내비게이션 완료까지 | 프런트 프리웜 적용: 시작 / 생성부터 첫 토큰까지 / 생성부터 내비게이션 완료까지 |
| --- | ---: | ---: |
| 콜드 | 971 / 1008 / 1347 | 877 / 928 / 1004 |
| 웜 브라우저 | 896 / 1020 / 1215 | 864 / 918 / 983 |

| 실험 | 단일 실행 변경 전 | 프런트 프리웜 미적용 | 프런트 프리웜 적용 |
| --- | ---: | ---: | ---: |
| E1 보정: 프런트 확인 응답 / Task steer 수락 | 프런트 / Task 분리 없음 | 39 / 30 | 26 / 22 |
| E1 실제 다음 모델 steer 주입 | 13017 (부모) | 13033 (Task) | 13044 (Task) |
| E2 빠른 질문 응답 | 13638 | 23 | 23 |
| E3 취소 확인 / 다음 메시지 응답 | 668 / 1217 (세션 전체 중단) | 7 / 4 (단일 Task) | 6 / 4 (단일 Task) |
| E4 재생: 첫 이벤트 / 도구 실행 중 | 4 / 4 | 2 / 2 | 1 / 2 |
| E4 중복 없는 재개 완료 | 13022 | 13009 | 13020 |
| E5 메시지 이후 두 번째 Task의 도구 시작 | 14022 | 20 | 23 |
| E5 첫 전송 시점부터 모든 작업 완료까지 | 31572 | 17111 | 16101 |

모든 수치의 단위는 밀리초(ms)이며, 실제 LLM 공급자나 모델의 지연 시간이 아니라 스크립트 기반 모의 모델과 헤드리스 Chromium을 사용한 격리된 픽스처 환경에서 측정한 값이다. 단일 모드의 첫 콜드 측정에는 Python과 uv 환경, 머신 캐시가 아직 웜업되지 않은 상태가 반영되어 있으므로, 초기 콜드 지연 시간 차이 전부를 프론트 모드 구조 자체의 차이로 볼 수는 없다. 또한 Front 에이전트가 steer 입력을 빠르게 접수하더라도, 백그라운드 Task는 실행 중인 블로킹 도구가 끝난 뒤 다음 모델 요청 시점에 이르러서야 steer 메시지를 받는다. 사전 예열(prewarm)은 메시지를 보내기 전에 기동 작업을 먼저 처리하는 방식이므로, 준비 소요 시간과 전송 기준 지연 시간, 그리고 세션 생성 기준 총 소요 시간을 함께 기록한다. 즉 예열은 초기화 비용을 없애는 것이 아니라 발생 시점을 앞으로 당기는 방식이다.

실행 명령:

```bash
flock --close /tmp/openside-heavy.lock nice -n 10 env OPENSIDE_TEST_DISABLE_PREWARM=1 cargo test -j 4 -p openside-e2e --test responsive -- --ignored --nocapture --test-threads=2
```

예열 활성화 비교 실행:

```bash
flock --close /tmp/openside-heavy.lock nice -n 10 env OPENSIDE_RESPONSIVE_PREWARM=1 cargo test -j 4 -p openside-e2e --test responsive -- --ignored --nocapture --test-threads=2
```

### 사전 예열(Prewarm) 및 정리

기본 Task e2e 테스트 환경에서는 IPC를 사용할 수 없어 Hyprland 단언을 건너뛰었다. 이후 네트워크가 차단된 독립 rootless podman 컨테이너 내 sway headed 하네스를 통해 검증을 마쳤으며, 4개 검사 항목을 모두 통과했다. 사전 예열은 사용자 메시지가 들어오기 전에 정확히 하나의 빈 에이전트 탭을 준비했고, 컴포지터 창 목록은 전후 모두 `[5]`로 일정했으며, 포그라운드 포커스 ID 5는 13회 샘플링 동안 다른 창에 포커스를 빼앗기지 않고 유지되었다. 이 작업은 실제 사용자 데스크톱이나 실행 중인 데몬 환경에 일절 영향을 주지 않는다. `--no-sandbox` 플래그는 격리된 컨테이너 픽스처 내부에서만 쓰였으며 상용 배포용 설정이 아니다.

증적 파일은 `/tmp/openside-qa/polish/responsive/prewarm-focus/sway/stock/` 디렉터리의 `result.json`, `focus-changes.json`, `prewarm-focus.png`에서 확인할 수 있다.

```bash
flock --close /tmp/openside-heavy.lock nice -n 10 env OPENSIDE_QA_OUT=/tmp/openside-qa/polish/responsive/prewarm-focus tools/qa/headed-ux.sh sway stock --prewarm-only --daemon-bin "$PWD/target/debug/e2e-opensided"
```

## F-9: 대화 메타데이터

브라우저 카드에는 민감 정보가 마스킹된 페이지 제목과 최대 32×32 픽셀, 8 KiB로 정규화된 PNG 파비콘이 제공된다. 공유 아이콘 서비스는 소유한 탭 내부에서 쿠키나 리다이렉트 없이 동일 출처(same-origin) 아이콘을 먼저 추출한다. 데몬의 유일한 대체 경로는 Fetch 및 egress 정책을 준수하는 정확한 HTTPS 오리진의 `/favicon.ico`다. 읽기 스코프의 아이콘 제공은 캐시 전용으로 이루어지며, 캐시에 없거나 누락된 아이콘은 원격 브라우저 fetch 없이 이니셜 타일을 표시한다.

티켓은 기존 `tool_call_id`를 통해 도구 호출과 연결된다. 정책 승인 요청 시 호출 세부 정보는 기술 상세 영역에 유지하면서, 확인된 액션·오리진·접근성 대상 라벨을 바탕으로 구성한 한 문장의 쉬운 영어 질문을 사람에게 제시한다. 세션 목록에는 브라우징, 사람 입력 대기, 완료 등 이벤트 기반의 실시간 활동 상태가 표시된다. 첫 어시스턴트 응답이 끝나면 모델이 URL 형식이 아닌 최대 6개 단어의 제목을 제안하며, 사용자가 직접 변경한 이름이 항상 우선한다. 런타임 오류는 차분한 기본 문구와 명시적인 재시도 가능 여부 플래그를 제공하고, 기술 진단 정보는 펼침 메뉴 뒤에 숨겨둔다. 재시도 시에는 새 메시지 ID를 부여해 마지막 사용자 메시지와 첨부파일을 다시 제출하며, 이미 처리된 승인은 절대 다시 실행하지 않는다.

승인 질문에는 진입 시점에 검사한 최신 대상 요소에서 캡처한 접근성 라벨을 사용하며, 이전에 전달된 브라우저 스냅샷은 보조 수단으로만 사용한다. 이 설명 라벨은 정책 매칭이나 승인 바인딩에 영향을 주지 않는다. 실행 중이 아닌 세션을 텍스트 없이 CLI로 재개할 때도 마지막 태스크 마커 이후부터 확인된 로그 끝까지 메타데이터를 재생하고, 해당 커서를 저장한 뒤 다른 태스크 완료를 기다리지 않고 바로 반환한다.

## 공유 대화 Card

단일 표준 Card 스키마와 렌더러가 상품, 장소, 표, 주문 요약, 로그인, OTP, 결제 양식, 승인, 팀 제안, 핸드오프를 처리한다. 읽기 전용 결과는 고정된 캡처 합계와 "Show all"을 포함해 3개 행을 미리 보여준다. 대기 중인 요청은 48픽셀 버블에서 펼쳐지며, 최종 결과는 동일한 Card를 대체한다. CLI 출력은 이미지를 다운로드하지 않고도 이에 상응하는 타입화된 정보를 보존한다.

프로바이더 선택은 실제로 캡처된 페이지 액션을 사용하며, 이후의 모든 로그인은 정확히 일치하는 해당 오리진에 바인딩된다. 계정 힌트는 마스킹된다. 결제 양식 응답은 검증된 최상위 문서나 프레임(top-document/frame) 필드에만 채워지며, 이후 결제를 최종 제출하려면 별도의 Confirm이 필요하다. 페이지 변경, 만료, 거부, 기기 간 경합이 발생해도 입력이나 승인을 재실행(replay)할 수 없다.

권한을 확장하는 채용에는 Hire Card를 요구하지만, 범위가 한정된 자율 채용에는 카드를 요구하지 않는다. 모든 채용 영수증에는 Retire 액션을 제공하며, 이전 방식의 명시적 영수증은 기존의 Undo 액션을 그대로 유지한다. Handoff Card는 대화에 참여하는 양쪽 스레드에 모두 표시한다. chief 사이드바의 Needs you와 축소된 attention dock은 안전하게 요약된 정보를 보여주며 단일 정식 양식으로 바로 연결되도록 라우팅한다. 보안 비밀 정보는 chief 스레드에 중복으로 노출하지 않는다. 새로고침, 재연결, 과거 로그 투영에서도 안정적인 식별자가 정확히 한 번만 유지된다. 수용 기준 검증에는 가상 자격 증명 및 결제 값, 스크립트 기반 프로바이더, 루프백 페이지만 사용한다.

openside는 어시스턴트의 기본 이름이며 별도 페르소나가 아니다. 반복되는 책임이나 장기 도메인, 독립적인 병렬 작업 흐름이 필요할 때는 사용자에게 묻지 않고 team_hire로 팀원을 직접 채용할 수 있다. 채용이 완료되면 팀원 이름과 담당 책임을 안내하는 짧은 send_message를 한 차례 전송한다. 일회성 작업은 create_task를 사용하고, 기존에 해당 책임을 맡은 팀원이 있다면 새로 채용하지 않고 그 팀원에게 작업을 맡긴다. team.max_active_teammates의 기본값은 chief를 제외한 5명이다. 단, 외부 연동이나 계정을 새로 추가하거나 권한 범위를 넓혀야 할 때는 여전히 사용자 승인을 거쳐야 하며, 사용자는 언제든지 팀원을 퇴출(retire)할 수 있다.
