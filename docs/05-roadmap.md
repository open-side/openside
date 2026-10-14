# 05. 로드맵과 완성도 정의

기간은 적지 않는다(팀 규모 미정). 대신 크기를 S/M/L/XL 로 표시하고, 각 마일스톤은 **종료 조건(자동 검증 가능)** 을 가진다.

## 1. 완성도 정의
완료 여부는 아래 세 가지 축으로 판단한다. 수치는 목표치이며, 측정 하네스는 M0–M1에서 먼저 구축해야 한다.

1. **수용 기준 커버리지**: `docs/01` 및 `docs/02`의 모든 요구사항 ID × 수용 기준(AC)을 `docs/specs/acceptance.csv`에 나열한다. 자동화 테스트를 통과한 AC 비율은 **99% 이상**이어야 한다(분모 = 범위 내 AC).
   - 범위 밖(분모에서 제외하며 각각 사유를 명시해야 함): 서드파티 클라우드 계정 및 구독 결제 연동, 공급업체 종속 모델 요금제, Widevine DRM 재생(PP-21), macOS 전용 권한에 대응하는 Linux 기능(컨텍스트 인식 권한 부여 등), 타사 상표 및 에셋
   - 면제(waiver) 는 범위 내 AC 의 1% 이하로 제한하고 이유를 기록.
2. **품질**: 
   - 메시지 손실 0 / 중복 0 (퍼징), 24시간 소크 테스트 크래시 0, 브라우저 강제 종료 후 자동 복구.
   - 시크릿 canary 테스트 100% 통과 (Vault 값이 어떤 출력에도 나오지 않음).
   - 프롬프트 예산 ≤ 10K 토큰(A-2), 스냅샷 크기 ≤ agent-browser 의 0.35x (B-3).
3. **에이전트 성능**(M6에서 측정하며, 보장이 아닌 목표치임): Online-Mind2Web 같은 공개 벤치마크를 실행하고 모델, 사고(thinking), 채점기 설정을 함께 보고한다. 라이브 웹 환경은 결과가 달라질 수 있으므로 재현 불확실성을 결과에 명시한다.

## 2. 마일스톤

### M0 — 기반 (S~M)
- ✅ 저장소·검증: `tools/gate.sh`에서 fmt, clippy, test, cargo-deny, 에이전트 검사 및 잔여 검증 통과. 2026-10-14 오너 결정으로 호스티드 CI를 걷어냈으며, 로컬에서 `tools/gate.sh`를 직접 돌리는 것을 표준 검증 기준으로 삼는다. ✅ 라이선스 고지(ADR-009, 워크스페이스 `MIT OR Apache-2.0`, NOTICE). ✅ 데이터 디렉터리 및 설정 스키마(`openside-config`: XDG 경로, `config.toml`, 환경변수 재정의). ✅ `openside doctor [--json]`(설정, 브라우저, 샌드박스, Vault, 자격 증명 소스 진단).
- ✅ `openside-policy`(D-2), ✅ `openside-core` 타입·trait(사이드카 계약으로 재작성).
- ✅ 스파이크: omp 시절 S-1~S-6, 사이드카 전환 S-8/S-9/S-14/S-15/S-16(`docs/spikes/`). 결과는 ADR 로 기록(docs/06).
- ✅ 수용 AC 목록(`docs/specs/acceptance.csv`), ✅ 테스트 harness(`crates/openside-e2e`: 실제 Python 사이드카 + 스크립트 모델).
- 종료 조건: ✅ 로컬 게이트 통과(호스팅 CI 실행은 PP-1 외부 의존성 차단 상태), ✅ 모든 스파이크 "가능 / 불가능 + 대안" 결론 도출 완료, ✅ 외부 아웃바운드 네트워크 허용 목록 테스트 통과(PP-25, `crates/openside-e2e/tests/egress.rs`, `[network].allow` 및 설정된 프로바이더·서비스 대상 격리 강제).

### M1 — 코어 루프 (XL)
- ✅ `opensided` + `openside-sessions`(이벤트 소싱) + `openside-agent-sidecar`(Pydantic AI 사이드카, warm 프로세스) + 영속 큐/steer/중단. omp RPC 런타임(`openside-agent-omp`)은 삭제.
- ✅ 퍼블릭 API v1(OpenAPI 3.1, 라우트 77개) + SSE + WebSocket + CLI(`--json`, `ticket` 하위 명령). ✅ `browser_repl`(QuickJS).
- ✅ `openside-browser`(Stage A) + 데몬 `browser_*` 도구 실연동(별도 윈도우 백그라운드 탭, 세션별 탭 소유권, `eN` 참조 체계, `Sensitive` 대상은 항상 Ask 처리, 데몬 재시작 후 재연결 보장). 브라우저 이벤트는 소유 세션의 이벤트로 기록. ✅ 사용자 브라우저 프로필 가져오기(B-1) 및 임시 시크릿 프로필(B-10). ✅ 팝업·다운로드·탭 종료 이벤트를 `steer`로 주입(B-2).
- ✅ `browser-agent` 프로필(10K 토큰 이하, A-2), ✅ 계정·모델·사용량 API(A-8, `GET /v1/accounts`). 자격 증명 소스(브로커 계정 선택, 고정/차단, fail-closed)는 사이드카에 구현 완료(docs/04 §9).
- 종료 조건: ✅ 엔드투엔드 시나리오 스위트 통과(`tools/e2e-scenarios.sh`, 공개 사이트 10곳 대신 로컬 루프백 사이트 대상 검증), ✅ 메시지 유실 및 중복 없음(`openside-sessions` proptest 및 사이드카 SIGKILL E2E 테스트 `sidecar_sigkill_restores_snapshot_preserves_queue_and_completes_each_id_once` 검증), ✅ 두 번째 메시지부터 별도 프로세스 기동 없음(세션당 웜 사이드카 유지), ✅ 타일링 창 관리자 활성 창 유지(B-5, 컴포지터 IPC 검증), ✅ 브로커 다중 계정 속도 제한 시 로테이션 E2E 검증(PP-18, 로컬 가상 브로커 대상; 2026-10-12에 로컬 브로커 대상 선택적 라이브 브로커 검사 통과).

### 현황표 (2026-10-12)
| 마일스톤/트랙 | 상태 | 근거 |
| --- | --- | --- |
| M0 | 완료 | 스파이크, acceptance.csv 및 검사기, `tools/gate.sh` (로컬 표준 게이트; 2026-10-14 호스티드 CI 제거), cargo-deny, licenses/NOTICE |
| M1 | 완료 | 데몬·세션·사이드카 런타임, CLI, 브라우저 도구, WebSocket, `browser_repl`, 계정 API; 타일링 창 관리자 활성 창 불변 조건(B-5) 컴포지터 IPC 검증 완료; 로컬 가상 브로커 기반 브로커 속도 제한 시 계정 로테이션 E2E(PP-18) 검증 완료 |
| M2 | 완료 | 정책 및 티켓, Vault(`vault_login`/`vault_otp`, 패스키, 플랫폼 잠금 해제), MCP 서버, 시크릿 모드, 레드팀 테스트 스위트, 샌드박스, 스코프 토큰 |
| M3 | 완료 | 서브에이전트(A-5, 전용 어댑터, ADR-020), 스킬, Ultrabrowse, 루틴, 태스크 아티팩트, 첨부 파일, 대화 압축 |
| M4 | 완료 (F-2 외부 차단 외) | 웹/PWA, Web Push 및 페어링, 서비스 유닛, Memory v1, 인라인 시각화, 설정 API; 데스크톱 Push는 실제 Google 푸시 서비스 연동 확인, iOS/Android 실기기 수신(F-2)만 외부 차단 |
| M5 | 완료 (1Password 계정 확인 외) | MV3 확장(Ask 사이드 패널, Agent Tabs), 가져오기, 채널 봇; Bitwarden은 C-7/PP-19 검증 완료; 1Password는 설치 및 로드만 확인(로그인 및 데스크톱 앱 연동 검증에는 벤더 계정 필요) |
| M6 | 완료 (24시간 연속 운전 진행 중 외) | 종단간 암호화 동기화, 패스키, 하드웨어 잠금 해제, 벤치마크 실행기; 15분 압축 안정성 테스트는 통과, 24시간 연속 운전 테스트(PP-23)는 2026-10-12 11:55 KST경 시작되어 진행 중 |
| M7 / BR-0…BR-4 | 완료 (Linux) | 패치된 Chromium P1–P11(패치 8종), `tools/patched-smoke.sh` 전 항목 통과; 격리된 sway 환경에서 P7/P9 헤디드 동작(세로 탭, 분할 보기, 북마크 패널) 검증; macOS/Windows 컴파일 확인(msvc는 cargo-xwin 이용, Windows 런타임 미구현, macOS 런타임 실기기 미검증); 네이티브 Ask 사이드 패널 미구현(확장 프로그램 사이드 패널 사용) |
| AC 자동 통과 | pass 219 / unverified 19 / blocked-external 3 / 전체 241행 | `docs/specs/acceptance.csv`. 아직 남아 있는 세부 요구사항 목록과 검증 방식은 acceptance.csv의 테스트 경로와 비고란을 통해 확인할 수 있다. |

### BLOCKED
초기 차단 요소였던 BLOCKED-1(서브에이전트)과 BLOCKED-2(takeover 프레임 스트림, HS-6)는 해결되었다. 외부 환경 때문에 아직 진행할 수 없는 항목은 [specs/acceptance.csv](specs/acceptance.csv)에서 `blocked-external`로 표시되어 있으며, 각 행의 `사용자 할 일:` 뒤에 필요한 작업이 적혀 있다.

### 다음 단계
1. 자체 환경에서 남은 외부 의존 항목을 검증한다: 호스팅 CI 실행, iOS/Android 실물 기기 푸시 수신, 완료된 24시간 soak 결과(`target/soak-24h-summary.json`), 계정이 준비된 1Password 환경.
2. Windows 및 macOS 런타임 지원 여부를 결정하고(견적은 docs/PLATFORMS.md 참고), Windows 런타임 구현에 앞서 정책 glob 정규화 문제를 수정한다.
3. 아직 확인하지 않은 헤디드 UX 세부 동작(세로·가로 전환 토글, 분할 보기 크기 조절/종료, 북마크 검색 후 열기)을 검증하고, 패치 0010 적용 후의 헤디드 매트릭스(PiP, 파일 선택 창, 인쇄, 네이티브 사용자 액티베이션)를 재실행한다. 아울러 확장 프로그램을 사용자 행위자로 취급하는 정책의 수용 여부를 확정한다.
4. cargo-xwin과 macOS/Windows 네이티브 CI 매트릭스를 CI 파이프라인에 연결한다.
5. 새 Chromium 핀 버전에 대응하는 자동 리베이스 CI(`browser/rebase.sh`)를 구축한다.

### M2 — 안전 (L)
- 정책 엔진 연동(Read only/Guard/Full + 규칙), 승인/질문 API 객체화(PP-11), 도구 훅에서 강제(PP-12), 감사 로그.
- `openside-vault`: 저장·origin 검증·`login`/`otp` 호스트 도구·redaction(C-1~C-5), incognito(B-10), CUA 폴백(B-4), 샌드박스(D-3), 스코프 토큰(PP-20), MCP 서버(E-2).
- 종료 조건: 시크릿 canary 100%, 프롬프트 인젝션 레드팀 스위트(결제/전송 유도) 차단률 100%(알려진 케이스 집합), 샌드박스 탈출 테스트 통과.

### M3 — 에이전트 기능 (L)
- 서브에이전트 API/UI·승인 라우팅(A-5), 스킬 관리/설치(A-6), Ultrabrowse(A-9), 루틴(A-10), 태스크 산출물(A-12), 첨부/이미지/오디오(PP-14), compaction 장기 테스트(A-3).
- 종료 조건: 속성 테스트로 PP-24 불변식 위반 0, 루틴 겹침/pause 시나리오 통과.

### M4 — 원격과 메모리 (L)
- 웹/PWA(F-1), Web Push(F-2), 페어링·기기 토큰(F-3), 서비스화·절전 inhibit(PP-4), 계정 대시보드, Memory v1(A-11), 인라인 시각화(A-15), 설정 전체 API화(PP-13), UI↔API 매핑표.
- 종료 조건: 폰에서 승인→재개 E2E(PP-22), 오프라인 후 재접속 재개, 읽기 토큰으로 steer 거부(PP-27).

### M5 — 브라우저 UX와 연동 (XL)
- MV3 확장: 사이드패널(Ask), 탭 그룹(Agent Tabs), omnibox Ask AI, 새 탭 페이지, 컨텍스트 첨부(`@`), Handle-with 버튼(A-14), 탭 이름/정리 등 가능 항목 (B-8).
- 데이터 import(B-1), 채널 봇(A-13), PM 확장 호환(C-7, PP-19).
- 종료 조건: B-8 항목별 AC 중 Stage A 가능 항목 100% 통과, 1Password/Bitwarden 확장 동작.

### M6 — 동기화·인증·벤치 (L)
- E2E 암호화 동기화(B-9), passkey/WebAuthn(C-2), 하드웨어 잠금(C-6), 벤치 러너·재현 스크립트(E-5, PP-26), 성능 최적화, 24h 소크.
- 종료 조건: 벤치 결과 공개, 두 기기 동기화 AC 통과.

### M7 — 패치 Chromium와 이식 (XL, 팀 리소스 필요)
- Stage C 패치 시리즈(완전한 비포커스 백그라운드 탭, 뷰포트 고정, 지문/봇 탐지 대응, 세로 탭·분할 보기 등 브라우저 크롬), 자동 rebase CI.
- macOS/Windows 이식(Vault 플랫폼 백엔드, 샌드박스 대체).
- 종료 조건: 해당 B-5/B-6/B-8 잔여 AC 통과.

### M8 — 작업 모델과 브라우저 경험 확장 (L)
- 프론트 에이전트와 Task 분리(A-16), 단일 작업 공간 및 Task별 탭 소유권(B-12), 브라우저 상태와 카드 UI(B-13), 브라우저 드라이버 고도화(B-15), 지연 시간 단축(B-16).
- 공통 글래스 시각 체계(F-6).
- 이미 구현된 항목: 스냅샷 뷰포트 범위(B-14), 로그인 폼 도구(C-9, 채널 링크 검증 제외), 격리 브라우저 공간(B-11, 단 기준 ③ 은 검토에서 발견된 결함 세 가지를 고치기 전까지 통과로 보지 않는다).
- `browser_repl` 내 페이지 JavaScript 허용(ADR-022; B-2 ③, B-15 ⑦) 및 Vault·로그인 명세 내 redaction/노출 관련 문구 반영
- PWA 네이티브 사용성 개선(F-7), 원격 전송 방식 지원(F-8), 외부 에이전트 CLI/MCP 위임 및 직접 제어(E-6), 원격 CLI/MCP 접속(E-7)
- 종료 조건: M8 에 속한 모든 요구사항의 AC 행이 `pass`.

## 2b. 브라우저 트랙 (M2 와 병행)
- BR-0 ungoogled-chromium 핀 빌드 성공(`docs/spikes/BR-0.md`). 종료: `chrome --version` 실행, 증분 빌드 시간 기록.
- BR-1 P1~P3 (AgentGateway, FocusArbiter, TabOwnership). 종료: 타일링 WM 에서 에이전트 작업 중 활성 창 불변 + 탭 장부 API.
- BR-2 P5~P7 (백그라운드 렌더/1440x900/자동화 아티팩트 제거). 종료: 자동화 아티팩트 점검 페이지 세트 통과.
- BR-3 P8~P9 (사이드패널·주소창·세로 탭/분할). BR-4 P10 (Vault autofill). BR-5 CI 자동 리베이스·릴리스.
- M7 은 BR-5 의 잔여(macOS/Windows 이식, 패치 유지보수 자동화)로 축소.

## 3. 의존 그래프(요지)
M0 → M1 → {M2, M3} → M4 → M5 → M6 → M7. M2 는 M1 의 데몬 툴 파이프라인(정책·티켓)에 의존. M5 의 확장 UI 는 M4 의 API 가 선행.

## 4. 검증 방식
- 각 AC 는 `acceptance.csv` 의 `test_path` 열로 테스트(현재는 크레이트·`agent/tests` 테스트)를 가리킨다. `tests/acceptance/<ID>_*` 1:1 배치와 CI 의 상호 검증은 계획.
- 실제 사이트 대상 E2E 는 불안정하므로 (1) 로컬 목업 사이트 스위트(결정적) + (2) 라이브 사이트 주간 회귀(비차단) 로 분리.
