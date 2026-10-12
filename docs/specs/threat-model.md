# 위협 모델

openside 는 한 사용자의 로컬 데몬이다. 아래 표는 막는 위협과 막는 위치, 검증 테스트를 정리한다. 막지 않는 위협은 마지막 절에 따로 적는다.

## 신뢰 경계

1. 모델 출력: 신뢰하지 않는다. 모든 툴 호출은 데몬의 하네스 훅(`host.rs`)을 거친다.
2. 웹 페이지 내용: 신뢰하지 않는다. 프롬프트 인젝션이 들어 있다고 가정한다.
3. 클라이언트: 토큰 스코프만큼만 믿는다. Unix 소켓 호출자는 같은 사용자로 본다.
4. Python 사이드카: 툴은 데몬의 Host 훅을 거쳐서만 실행하고, 비밀은 받지 않는다. 다만 사이드카 프로세스 자체는 샌드박스에서 돌지 않는 한 신뢰하는 코드다. 외부 전송 검사는 그 프로세스 안에서 도는 장치이고 커널 수준 격리가 아니다.

## 위협과 대응

| 위협 | 대응 | 검증 |
|---|---|---|
| 페이지가 모델에게 결제·전송을 지시 | 키워드로 분류한 결제·구매·삭제·전송·게시 행동(`sensitive`)은 항상 승인이 필요하다. 승인 전에는 실행하지 않는다. 주입 문구는 에이전트가 스냅샷으로 읽어 대화에 들어간 뒤에도 같은 결과다. 승인한 호출은 성공한다(대조군) | `openside-e2e/tests/redteam.rs` |
| 페이지가 데이터를 다른 origin 으로 빼내도록 지시 | `allow_origins`/`deny_origins` 를 도구 호출 단계와 브라우저 컨트롤러에서 강제한다. 컨트롤러는 에이전트 탭의 모든 최상위 문서 요청(리다이렉트 매 홉, 링크·폼, 팝업)을 요청이 나가기 전에 판단하고, 소유자를 확인하지 못한 요청은 거부한다. `Ask` 승인은 그 호출이 실행되는 동안에만 유효하다 | `redteam.rs`, `opensided/src/tests/policy.rs::denied_origin_is_unreachable_through_any_agent_navigation`, `::approved_ask_navigation_does_not_outlive_the_approved_call` |
| 모델이 승인 없이 정책을 우회 | 판단은 프롬프트가 아니라 데몬 훅에서 한다. `Deny` 는 승인으로도 풀리지 않는다 | `opensided/src/tests.rs::policy_deny_cannot_be_approved` |
| 비밀 입력 티켓을 연 뒤 대상 origin 이 거부됨 | 값을 채우는 순간 현재 정책으로 다시 판단한다. `Deny` 면 403 `policy_denied`, 필드는 그대로다 | `tests/policy.rs::secret_release_rechecks_live_policy` |
| 정책 변경이 동시에 들어오거나 요청이 끊김 | 저장과 반영을 한 단위로 하나씩 처리한다. 실행 중인 정책, 디스크, 재시작 뒤 정책이 같다 | `tests/policy.rs::concurrent_policy_puts_leave_live_and_disk_identical`, `::aborted_policy_put_still_commits_live_and_disk_together` |
| 워크스페이스 밖 파일 접근(`..`, 심볼릭 링크, 절대 경로) | `guard` 는 승인을 묻는다. 끊긴 링크에 쓰기는 거부한다. `bash` 는 bubblewrap 안에서 실행한다 | `opensided/src/tests/policy.rs::sandbox_escape_suite_never_touches_outside_paths` |
| 스코프가 좁은 토큰으로 권한 상승 | 라우트마다 스코프를 검사한다. 토큰 발급은 `admin` 이고 Unix 소켓으로만 된다. TCP 는 `admin` 토큰도 403 `local_socket_required` 다 | `tests/policy.rs::every_route_rejects_tokens_lacking_its_scope`, `::token_minting_is_socket_only`, `openside-e2e/tests/scopes.rs` |
| 샌드박스 안 클라이언트의 소켓 접근 | 런타임 디렉터리를 샌드박스에 넣지 않으면 소켓에 닿지 않는다. TCP 는 토큰이 필요하다. 검증은 사용자 네임스페이스 안의 클라이언트로 한다(아래 한계 참고) | `scopes.rs` |
| 기본 설치의 외부 통신 | 데몬에는 외부 클라이언트가 없다. 사이드카의 HTTP 전송, 채널 어댑터, 모델 다운로드는 `network.allow` 와 설정한 엔드포인트 밖으로 나가는 요청(리다이렉트 포함)을 거부한다. Chromium 백그라운드 통신은 끄고, 끌 수 없는 GCM 및 컴포넌트 업데이트 요청은 로컬 포트로 돌린다 | `openside-e2e/tests/egress.rs`, `agent/tests/providers/test_egress.py` |
| 텔레메트리 | 전송 코드를 두지 않는다. 알려진 SDK 이름 목록으로 의존성을 검사하고, 네트워크 관찰로 허용 밖 전송이 없음을 확인한다 | `egress.rs::telemetry_has_no_sender_in_rust_or_python_dependencies`, `egress.rs::default_install_contacts_only_loopback_and_allow_listed_peers` |
| 정책 파일 손상 | 시작을 거부한다(fail closed) | `tests/policy.rs::policy_api_validates_persists_and_hot_reloads` |

## 막지 않는 것

- 같은 사용자 권한의 다른 프로세스. 소켓·데이터 디렉터리 권한(0700/0600)으로만 분리한다. 사용자 네임스페이스 안에서 uid 를 바꾼 프로세스도 커널에서는 같은 uid 다. 그래서 `scopes.rs` 는 "네임스페이스 클라이언트" 검증이지 OS 권한 분리 검증이 아니다. 다른 커널 uid 로 실행하는 검증에는 root 나 별도 계정이 필요해서 자동 테스트에 넣지 않았다.
- 샌드박스에서 돌지 않는 사이드카. 사이드카나 그 의존성이 직접 여는 소켓은 외부 전송 검사로 막지 못한다. 알려진 텔레메트리 SDK 가 의존성에 없다는 것과 네트워크 관찰에서 나간 패킷이 없다는 것까지가 보장이다.
- 페이지 자체의 스크립트가 보내는 요청과 Chromium 의 페이지 로드. 정책은 에이전트의 행동만 판단한다. 사용자가 그 사이트를 연 것과 같다. 페이지 로드는 `network.allow` 를 거치지 않고 origin 목록으로만 제한한다.
- 키워드에 걸리지 않는 위험한 버튼. 민감 행동은 이름·역할 키워드로 분류하므로, "Continue" 같은 이름의 결제 버튼은 `sensitive` 로 보이지 않는다.
- 이미 실행 중인 호출. 정책 변경은 다음 툴 호출부터 적용된다. 실행 중인 REPL 스니펫은 시작할 때의 정책으로 끝까지 돈다. 브라우저 이동 검사만 요청마다 현재 정책을 읽는다.
- 같은 데몬의 다른 세션. `browse` 토큰은 세션이 아니라 데몬 전체에 대한 권한이다.
- 사용자가 승인한 민감 행동의 결과.
- LLM 제공자에게 보내는 대화 내용. 제공자 선택은 사용자 몫이다.
