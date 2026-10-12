# 01. openside 요구사항

openside의 기능 요구사항을 정리한 문서다. 완료 기준과 측정 방식은 `docs/05-roadmap.md` §1에 정의되어 있다.

범례
- **단계**: M0~M7 (로드맵 마일스톤). 
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
| G-2 | 에이전트 런타임은 Pydantic AI Python 사이드카(`agent/`, 세션당 프로세스 1개, 유닉스 소켓 JSONL 프로토콜 v1; `docs/08`, `docs/specs/sidecar-protocol.md`). 모델 자격증명은 플러그인 소스(`omp-broker`(선택), `env`, `command`, `codex-cli`, `builtin`)에서 사이드카가 읽고, openside 는 제공자 자격증명을 저장하지 않는다. broker 소스가 선택됐는데 쓸 수 없으면 다른 소스로 낮추지 않고 세션을 중지한다(fail-closed). |
| G-3 | Linux 우선(Wayland/X11). macOS/Windows 는 같은 코드베이스의 후속 타깃(플랫폼 추상화 계층을 처음부터 둔다). |
| G-4 | 로컬 우선. 기본값에서 외부로 나가는 데이터는 LLM 제공자 요청뿐. 텔레메트리 기본 OFF. |
| G-5 | 모든 클라이언트(CLI, MCP, 웹, 모바일, 채널봇)는 같은 공개 데몬 API 의 클라이언트다. 숨은 경로 없음. |

## A. 에이전트 코어
| ID | 기능 | openside 요구사항 / 수용 기준 (AC) | 기반 | 단계 |
| --- | --- | --- | --- | --- |
| A-1 | 코딩 에이전트. 핵심 도구는 JS REPL + bash. Playwright 문법의 코드를 모델이 작성 | 에이전트 기본 툴셋을 REPL(브라우저 prelude 포함)+bash+파일 도구로 제한한 `browser-agent` 프로필 제공. AC: 프로필 로드 시 노출 도구 이름 목록이 고정 스냅샷 테스트와 일치. 현재 구현은 `browser_*`·`bash`·파일·`vault_*`·티켓 툴 15개(`docs/09` §3)이고 `browser_repl` 과 `browser-agent` 프로필은 아직 없다 | 🟡 데몬 툴 + Pydantic AI function tools | M1 |
| A-2 | 시스템 프롬프트 + 도구 정의 합계 약 10K 토큰, AI 가 프롬프트를 쓰지 않음 | 프롬프트 토큰 예산을 CI 에서 측정. AC: 기본 툴셋의 `open.system_prompt_parts` + `open.tools` 합 ≤ 10K 토큰, 초과 시 CI 실패. 측정 도구는 아직 없다 | 🔴 측정 하네스 | M1 |
| A-3 | 긴 대화 compaction 안정성 | harness `SummarizingCompaction`(연결됨, 기본 `max_messages=100`·`keep_messages=20`; `docs/08` §E) + 세션 장기 실행 테스트. AC: 합성 200턴 세션에서 compaction 후 작업 목표 유지(스크립트화된 회상 테스트 통과) | 🟢 compaction | M3 |
| A-4 | Queue / Steer 후속 메시지, 큐 메시지 편집·재정렬, Cmd+Enter steer | 데몬이 세션별 영속 큐를 소유(`MessageQueue`). AC: ① steer 가 실행 중 턴에 주입(`user_message` mode `steer`, 다음 모델 요청 직전; `docs/08` §C) ② queue 는 현재 런의 작업이 끝난 뒤(`queue`) ③ 큐 항목 편집/삭제/steer 로 승격(`PATCH`·`DELETE /v1/sessions/{id}/queue/{mid}`, `POST …/promote`), `POST /v1/sessions/{id}/queue/{mid}/move`로 레인 내 `position` 재정렬(클램핑, `message.moved` 기록, 완료·미존재 시 409)을 지원한다 ④ 프로세스 재시작 후 큐 보존 ⑤ 중복 전송 0 (message id) | 🟡 사이드카 steer 버퍼 + 데몬 큐 영속화 | M1 |
| A-5 | 서브에이전트: task 안에서 그룹, 라이브 팔로우, 모델 선택, 승인이 부모로 올라옴, fork 는 완료된 컨텍스트만 상속 | AC: ① 서브에이전트 목록/진행/이벤트 스트림을 UI·API 로 제공 ② 서브에이전트별 모델 지정 ③ 서브에이전트의 승인·질문이 항상 부모 세션 승인함에 표시 ④ 개별 취소/steer ⑤ fork 시 미완료 tool-call batch 제외 테스트. **현재 BLOCKED-1**: harness `SubAgents` 는 자식 보류를 부모로 올리지 못해 채택하지 않았고, 프로토콜(`tool_call.subagent`, `subagent.*` 이벤트, 티켓 `origin.subagent_id`)만 정의돼 있다 | 🔴 어댑터 (결정 대기) | M3 |
| A-6 | 스킬: 커스텀 스킬 관리, 대화 중 스킬 생성, `/` 멘션, CLI를 통한 스킬 설치 | AC: ① 스킬 목록/보기/생성/편집/토글 API ② 채팅 입력에서 스킬 멘션(칩) ③ `openside skills install <agent>` 로 openside 브라우저 스킬을 Claude Code/Codex 등에 설치 ④ 잘못된 frontmatter 가 설정 화면을 깨지 않음(퍼징 테스트). harness `skills` 는 S-9 에서 `later`(`docs/08` §E)이고 설치기·API 는 아직 없다 | 🟡 harness skills(후속), 🔴 관리 API/설치기 | M3 |
| A-7 | 훅, 샌드박스 | AC: 승인 게이트·시크릿 redaction 을 모델 호출 경로가 아니라 데몬 호출 파이프라인(`docs/09` §2)에서 강제하고, 사이드카는 Pydantic AI hook(`SessionHooks`)으로 이벤트·steer 를 처리. 샌드박스는 D-3 | 🟡 데몬 정책 + Pydantic AI hooks | M2 |
| A-8 | 제공자 3종: 플랜 모델 / 구독 재사용(ChatGPT, Claude, Copilot) / BYO API 키 | **openside 는 제공자 로그인 UI 를 만들지 않는다.** 자격증명 소스(`credentials.order`)의 계정을 그대로 사용(→ `docs/08`). `omp-broker` 소스를 쓰면 broker 에 등록된 계정·사용량(`/v1/usage`)·rate-limit block 을 활용한다. AC: 설정된 소스로 모델을 호출하고, 사용 가능한 소스가 없으면 `openside doctor` 가 원인을 보이며 세션은 fail-closed 로 중지. 계정·모델 목록 UI 표시는 후속 | 🟢 Pydantic AI provider, 🟡 소스 플러그인 | M1 |
| A-9 | Ultrabrowse (리서치 모드, 출처 인용) | 별도 `ultrabrowse` 프로필(더 높은 thinking, 계획 단계, 인용 강제). AC: 응답의 모든 사실 주장에 출처 URL 링크, 인용 없는 주장 비율 측정 harness. 프로필은 아직 없다 | 🟡 프로필+프롬프트 | M3 |
| A-10 | Routines: cron(새 태스크) / heartbeat(기존 채팅 재개), 겹침 스킵, 대상 소멸 시 일시정지, 제안 | AC: ① cron 표현식 스케줄 ② heartbeat = 기존 세션 큐에 메시지 주입 ③ 이전 실행 진행 중이면 스킵+기록 ④ 대상 세션 삭제 시 pause ⑤ "지금 실행" ⑥ 반복 작업 후보를 찾아 초안 생성 | 🔴 스케줄러 | M3 |
| A-11 | Memory: 브라우징 기록·채팅·태스크에서 로컬 임베딩 기억, 보존기간, 열람·편집·이력 | AC: ① 로컬 SQLite+벡터 검색 ② 보존 정책(영구/90일/30일) 적용 ③ 기억 CRUD + 변경 이력 ④ 네트워크로 나가는 임베딩 호출 0 (로컬 모델) ⑤ 시크릿 패턴이 기억에 저장되지 않음. harness `memory` 는 S-9 에서 `later`(docs/06 ADR-007) | 🔴 히스토리 인덱서 | M4 |
| A-12 | 태스크 산출물: 태스크 폴더에 transcript·파일 보관, 이미지/PDF/HTML/텍스트 미리보기 | AC: 태스크별 디렉토리, 생성/변경 파일 목록, 안전한 sandbox iframe 미리보기, 태스크 삭제 시 정리 | 🟡 | M3 |
| A-13 | Channels: Slack/Telegram/Discord 원격 제어, 음성 메모 전사, `/stop` `/model` `/sessions` `/usage` `/new`, 승인 카드 | AC: 각 채널 어댑터가 데몬 API 의 클라이언트로 동작(별도 로직 없음). 스레드=세션 매핑, 승인 카드, 음성 전사(로컬 STT 옵션) | 🔴 어댑터 | M5 |
| A-14 | Gmail이나 Slack 메시지를 사이드패널 세션으로 전달한다 | 확장의 컨텍스트 메뉴 + 사이트별 버튼 주입 프레임워크. AC: Gmail/Slack 웹에서 선택한 메시지가 새 세션 컨텍스트로 열림 | 🔴 확장 | M5 |
| A-15 | 에이전트가 채팅에 차트·시각화를 인라인 렌더링 | AC: sandbox iframe 에서 HTML/SVG/차트 렌더링, 소스 보기 토글 | 🟡 UI | M4 |

## B. 브라우저
| ID | 기능 | openside 요구사항 / 수용 기준 | 기반 | 단계 |
| --- | --- | --- | --- | --- |
| B-1 | 로그인된 실제 브라우저 프로필에서 동작, 브라우저 데이터 import(기록·쿠키·북마크, Safari ZIP) | AC: Chrome/Chromium/Firefox 프로필에서 history·bookmarks·cookies import, 프로필별 분리, 실패 시 원인 메시지 | 🔴 | M5 |
| B-2 | Playwright와 완벽히 호환되는 얇은 CDP 래퍼. 비동기 이벤트는 steer로 전달한다 | AC: ① 모델이 쓰는 코드가 Playwright 문법 그대로 동작(호환 스위트 통과율 공개) ② 팝업/다운로드/탭닫힘 이벤트가 steer 로 주입 ③ 주입 스크립트 없음(CDP 호출만). 현재 브라우저 제어는 데몬의 `openside-browser`(CDP) + `browser_*` 툴이고 Playwright 호환 REPL 은 아직 없다 | 🔴 자체 CDP 래퍼 | M1 |
| B-3 | 개량 접근성 트리: ref ID, 중간 노드 제거, focus·iframe·clickable, diff, agent-browser 대비 70% 작게 | AC: 스냅샷 크기 벤치(고정 페이지 세트)에서 agent-browser 대비 ≤ 0.35x, 연속 호출은 diff 만 반환, ref 는 REPL locator 와 동일, truncation 금지 경고. 현재 `browser_snapshot` 은 `eN` ref 와 `diff` 를 제공하고 크기 벤치·REPL locator 는 아직 없다 | 🔴 접근성 스냅샷 + diff | M1 |
| B-4 | CUA 폴백 (스크린샷+좌표) | AC: DOM 으로 못 쓰는 페이지에서 스크린샷·좌표 클릭 도구, 뷰포트 1440x900 고정 | 🟡 | M2 |
| B-5 | 백그라운드 탭: 포커스 안 뺏음, 팝업 없음, 뷰포트 1440x900 고정 | Stage A: CDP `Target.createTarget{background:true}`+`Emulation.setDeviceMetricsOverride`+`setFocusEmulationEnabled`(전용 창). Stage C: Chromium 패치로 완전 보장. AC: 에이전트 작업 중 활성 창이 변하지 않음을 자동 검증(데스크톱 환경의 창 관리자 IPC 로 확인하는, 환경 의존 선택 실행 테스트) | 🔴 | M1(근사) / M7(완전) |
| B-6 | 봇 탐지 회피 (진짜 브라우저, Chromium 내부 패치) | Stage A: 실제 Chrome 프로필 사용으로 최대한 정상 지문 유지. Stage C: 지문 패치 시리즈. AC: 공개 봇 탐지 테스트 페이지 세트에서 점수 기록(합격 기준은 로드맵에 명시) | 🔴 | M7 |
| B-7 | Agent Tabs 그룹, 탭 소유권(openedBy), 5개 이상 시 경고, 마지막 탭 닫힘 처리, 잠든(unloaded) 탭 처리 | AC: ① 에이전트가 연 탭만 닫을 수 있음(사용자 탭은 읽기 전용) ② 탭 ≥5 시 시스템 경고 ③ unloaded 탭에 attach 하지 않고 `TabAsleep` 반환(타임아웃 12초 이내) ④ 외부 에이전트(MCP/CLI)의 탭도 Agent Tabs 그룹으로 | 🟡 | M1 |
| B-8 | 브라우저 UX: 세로 탭, 분할 보기, 북마크 바/사이드바, 핀 탭, 탭 이름 변경, AI 탭 정리(Tidy), 프로필 전환, PiP, 사이드패널(Ask), 주소창 Ask AI(`Tab` 전환, `@` 컨텍스트), Mini popup | Stage A: 확장(`chrome.sidePanel`, `chrome.tabGroups`, omnibox keyword, 새 탭 페이지 override)로 구현 가능한 것부터. 브라우저 크롬 자체가 필요한 항목은 Stage C 패치. 항목별 AC 는 구현 시 `docs/specs/browser-ux.md` 로 분해 | 🔴 | M5~M7 |
| B-9 | 클라우드 동기화(핀 탭, 볼트, 브라우저 데이터 복구) | **서버 없이** 동기화: 사용자 소유 저장소(자체 서버/S3/WebDAV/Syncthing 디렉토리)에 E2E 암호화 스냅샷. AC: 두 기기 간 핀 탭·볼트 항목 동기화, 서버는 평문 접근 불가, 충돌 해결 규칙 문서화 | 🔴 | M6 |
| B-10 | Default / Incognito 태스크 모드 (시크릿에서는 Vault 불가) | AC: incognito 태스크는 임시 프로필, 종료 시 완전 삭제, Vault 접근 거부 테스트 | 🟡 | M2 |

## C. Vault (에이전트용 패스워드 매니저)
| ID | 기능 | openside 요구사항 / AC | 기반 | 단계 |
| --- | --- | --- | --- | --- |
| C-1 | 자격증명을 autofill 로 주입, 모델엔 비노출 | `vault_login`/`vault_otp` 데몬 툴(`docs/09` §3). 시크릿은 데몬(Rust)이 CDP 로 직접 입력하고 사이드카·모델 경로를 지나지 않는다. AC: 모델 컨텍스트·로그·스냅샷·스크린샷·transcript 에서 시크릿 문자열 grep 0건(자동 테스트, canary 시크릿 사용) | 🟡 데몬 툴 | M2 |
| C-2 | passkey / FIDO2 / TOTP | TOTP 로컬 생성. passkey/FIDO2 는 소프트웨어 authenticator(CDP WebAuthn virtual authenticator) 우선, 하드웨어 키는 사용자 개입. AC: 공개 WebAuthn 테스트 페이지 등록·로그인 성공 | 🔴 | M2(TOTP) / M6(passkey) |
| C-3 | 접근 정책 Always / While unlocked / Never, 항목별 override | AC: 정책 위반 시 도구가 거부하고 감사 로그 기록 | 🔴 | M2 |
| C-4 | 사용 감사 로그 (언제·무엇·왜) | AC: 모든 get/fill/otp 호출에 task id·origin·시간·결과 기록, UI 에서 조회, 시크릿 값은 기록하지 않음 | 🔴 | M2 |
| C-5 | origin 검증 | AC: 저장된 도메인과 현재 페이지 origin 불일치 시 채우기 거부(리다이렉트·피싱 시나리오 테스트) | 🔴 | M2 |
| C-6 | 하드웨어 기반 잠금 해제 (Secure Enclave / Touch ID / Windows Hello) | Linux: TPM2(systemd-creds/tpm2-tss)·libsecret·FIDO2 hmac-secret 중 선택, 플랫폼 추상화 trait. AC: 마스터 키가 디스크에 평문으로 없음 | 🔴 | M6 |
| C-7 | 서드파티 PM 확장 호환 | 표준 Chromium 확장 + native messaging 이 그대로 동작해야 함(→PP-19). AC: 1Password/Bitwarden 확장 설치·동작 확인 | 🟡 | M5 |
| C-8 | 결제/게시/메시지 등 민감 행동 사람 승인 | → D-4 | | M2 |

## D. 권한 / 보안 / 프라이버시
| ID | 기능 | openside 요구사항 / AC | 기반 | 단계 |
| --- | --- | --- | --- | --- |
| D-1 | 세션 모드 Read only / Guard(기본) / Full access | `openside-policy` 크레이트. 설정 `[policy] mode = "read-only" \| "guard" \| "full"`. AC: 모드별 허용 매트릭스 테스트, Guard 기본값 | 🟡 데몬 정책 | M2 |
| D-2 | 규칙 Allow / Ask / Deny, Deny 우선, 에이전트 단위+세션 단위 | AC: 우선순위 속성 테스트(Deny>Ask>Allow, 세션 규칙이 에이전트 기본값 위에 적용) — 이 저장소에 이미 구현·테스트 존재 | 🔴 | M0 (구현됨) |
| D-3 | OS 수준 sandbox | Linux: bubblewrap(`--unshare-all`) 또는 Landlock. 경로는 canonical 비교, symlink 탈출 차단. AC: 샌드박스 밖 경로 읽기/쓰기 시도 시 거부(탈출 테스트 스위트). 현재 데몬 `bash` 가 bwrap 을 직접 실행하고, bwrap 실행이 실패하면 샌드박스 없이 다시 실행하지 않는다. bwrap 이 없으면 `sandbox.required=false` 일 때만 경고(`sandbox_unavailable`)와 함께 정책 전용 격리로 실행하고 `true` 면 `bash` 를 숨긴다(`docs/09` §7) | 🟡 데몬 `bash` + 외부 sandbox | M2 |
| D-4 | 결제·게시·메시지는 항상 사람 승인 | AC: 승인 요청은 API 객체(id, 요약, 대상 origin, 만료)이며 어떤 클라이언트에서든 응답 가능, 미응답 시 안전하게 대기·만료. 프롬프트 규약이 아니라 **데몬 호출 파이프라인에서 강제**(`Sensitive` action 은 자동 허용되지 않음, `docs/09` §4) | 🟡 | M2 |
| D-5 | 로컬 우선 데이터 | AC: 데이터 디렉토리 문서화, 외부 전송 목록(허용 도메인) 자동 검사, 텔레메트리 기본 OFF | | M0 |
| D-6 | 비밀번호 열람 불가 경계 (Full access 도 비밀번호 값 노출 안 함) | AC: Full access 모드에서도 Vault 값을 반환하는 경로 없음(테스트) | | M2 |

## E. 개발자 표면
| ID | 기능 | openside 요구사항 / AC | 기반 | 단계 |
| --- | --- | --- | --- | --- |
| E-1 | CLI: 작업 실행, 세션 지정, 지정한 모델로 비대화형 실행, 세션 중지, 계정 조회 및 선택 | `openside "task"`, `openside session resume|stop|list`, `openside exec -m provider/model`. 계정 조회·선택은 자격증명 소스 쪽이 소유하고 openside 는 `openside doctor` 로 상태를 보인다. AC: TTY 없이 파이프로 구조화 출력(JSONL) 가능 | 🟡 | M1 |
| E-2 | MCP 서버 | `openside mcp`: 브라우저 도구 노출. AC: Claude Code/Codex 등에서 `mcp.json` 로 연결해 탭 열기·스냅샷 | 🔴 | M2 |
| E-3 | REPL | `openside repl` 결정적 브라우저 제어. AC: 에이전트가 쓰는 스냅샷·ref 와 동일 | 🟡 | M1 |
| E-4 | 퍼블릭 HTTP/WS API 및 SDK | OpenAPI 스펙+JSON Schema 에서 Rust/TS 클라이언트 생성. AC: 모든 UI 기능이 API 호출로만 구현됨 | 🔴 | M1 |
| E-5 | 벤치마크 | `openside-bench`: Online-Mind2Web/BU-Bench/Odysseys 러너 (aside-benchmarks 포맷 호환). AC: 결과 JSON 이 aside-benchmarks 스키마로 출력 | 🔴 | M6 |

## F. 원격 / 모바일
| ID | 기능 | AC | 단계 |
| --- | --- | --- | --- |
| F-1 | 모바일 웹/PWA: 대화 목록·검색·고정·보관, 라이브 스트리밍, 도구 활동 접기, 큐 편집·steer, 모델 선택, 브라우저 PiP 미리보기, 이미지 줌, 첨부 업로드 | 기능 목록을 체크리스트 형태로 관리한다(`docs/specs/remote-ui.md`에 세부 항목 정리). | M4 |
| F-2 | Web Push (VAPID): 완료/실패/승인 필요, 포커스된 기기 제외, 알림 클릭 시 해당 대화 | AC: iOS 홈 화면 앱/Android/Desktop 에서 수신 | M4 |
| F-3 | 인증: 로컬 loopback 기본, QR 페어링(3분 만료), 기기별 토큰, 폐기 | AC: 토큰 스코프(read/steer/approve/admin) | M4 |
| F-4 | 원격 접속: Tailscale Serve 가이드, 선택적 reverse proxy | 문서+doctor 점검 | M4 |
