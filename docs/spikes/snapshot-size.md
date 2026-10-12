# snapshot size spike (Phase C-3)

## 1. 개요 및 측정 결과

본 스파이크는 `openside-browser`의 압축 접근성 트리(compact AX) 텍스트가 원본(raw) CDP AX JSON 대비 어느 정도 크기로 축소되는지 검증한 결과다. 로컬 픽스처 6종을 대상으로 최종 측정한 결과, snapshot 텍스트의 잘림(truncation) 없이 compact AX 크기는 원본 JSON 대비 약 **1.10% ~ 5.11%** 수준으로 감소했다.

- 측정 환경 Chromium 버전: `Chromium 152.0.7977.82 Arch Linux` (`/usr/bin/chromium --version`)
- 비교 대상: `agent-browser 미설치로 비교 생략` (agent-browser 관련 수치는 측정하지 않음)

### 크기 측정 결과 요약

| fixture | raw AX JSON bytes | compact text bytes | compact/raw |
|---|---:|---:|---:|
| forms.html | 19940 | 781 | 0.0392 |
| popup.html | 3663 | 167 | 0.0456 |
| download.html | 3725 | 188 | 0.0505 |
| dialog.html | 3670 | 160 | 0.0436 |
| long-list.html | 707104 | 36098 | 0.0511 |
| nested-divs.html | 416162 | 4565 | 0.0110 |

## 2. 측정 기준 및 정의

본 평가는 지연 시간(latency)이나 토큰 소모량을 다루는 벤치마크가 아니며, 순수한 바이트 크기(byte size) 비교만을 다룬다.

- **raw bytes**: WebSocket 메시지 봉투(id 및 sessionId 등)를 제외하고, CDP `Accessibility.getFullAXTree`의 결과 객체(`nodes` 포함)를 `serde_json`으로 공백 없이 직렬화(compact serialization)했을 때의 바이트 길이.
- **text bytes**: 슬라이싱이나 잘림 없이 전체 생성된 compact `Snapshot.text`의 UTF-8 바이트 길이.
- **compact/raw 비율**: `text_bytes / raw_bytes`.

## 3. 테스트 환경 및 픽스처 조건

- 대상 픽스처: `crates/openside-browser/tests/fixtures/*.html`
- 서빙 환경: 임시 포트 `127.0.0.1:0`에 바인딩한 axum 로컬 서버
- 브라우저 실행 조건: 매 실행마다 독립된 `/tmp/openside-chromium-*` 임시 user-data-dir을 사용하는 headless Chromium, 뷰포트 크기 1440x900
- 픽스처 로딩 완료 판정: `createTarget` 탐색이 완료되기 전 초기 `about:blank` 페이지가 이미 complete 상태일 수 있으므로, 요청한 정확한 픽스처 URL과 `document.readyState === "complete"` 조건을 둘 다 만족해야 준비 완료로 판정한다.
- 픽스처 검증 조건:
  - `long-list.html`: 버튼 300개를 렌더링하며, 테스트는 300개 버튼과 마지막 항목이 compact 트리에 누락 없이 모두 보존되는지 검증한다.
  - `nested-divs.html`: 20겹의 래퍼 엘리먼트 아래에 감싸인 버튼 60개를 렌더링하며, 60개 버튼이 모두 보존되고 compact 트리상 중첩 깊이(depth)가 5 미만으로 압축되는지 검증한다.

## 4. 트리 압축 및 Diff 규칙

1. **노드 축소 및 압축**:
   - `ignored`, `none`, 이름 없는 generic 노드 및 불필요한 `InlineTextBox` 노드는 제거한다.
   - 자식 노드의 콘텐츠는 버리지 않고 상위로 끌어올린다(lift).
   - 이름이 없는 단일 자식 비대화형(noninteractive) 래퍼 노드는 하나로 축약(collapse)한다.
2. **참조 번호(ref) 부여 및 초기화**:
   - 대화형(interactive) 노드 또는 이름이 있는 노드에는 `backendDOMNodeId`를 기본 키로 사용하여 안정적인 `e1`, `e2` 등의 식별자 참조를 부여한다(`nodeId`는 대체 수단).
   - 문서가 유지되는 동안 해당 노드가 일시적으로 트리에서 사라지더라도 기존 식별자 참조를 유지한다.
   - 비동기로 전달되는 프레임 탐색 이벤트로 인해 오래된 이벤트가 참조를 잘못 초기화하는 문제를 막기 위해, 다음 스냅샷 처리 시 `RootWebArea`의 `backendDOMNodeId`를 대조하여 최상위 프레임 문서 교체를 직접 감지했을 때 참조를 초기화한다.
3. **Snapshot::diff**:
   - 이전 스냅샷과 비교하여 추가(`added`), 제거(`removed`), 변경(`changed`)된 참조 목록을 반환한다.
   - `changed`는 role, name, value, depth 변경을 기준으로 판정한다.
4. **민감 정보 마스킹(redaction)**:
   - 보호된 입력 필드의 값은 compact 텍스트 출력 시 마스킹된다.
   - 원본 raw JSON은 크기 계산에만 사용되며 외부에 출력되지 않는다(raw JSON 자체가 마스킹되는 것은 아님).

## 5. 컨트롤러 구현 세부 사항 및 제약

- **세션 및 연결 모델**:
  - 브라우저 타깃 WebSocket을 사용하며, 계층 구조 없는 수동 연결(flat manual attach) 방식을 취한다.
  - 타깃 생명주기 이벤트 처리는 `Target.setDiscoverTargets`를 사용한다.
  - `Target.setAutoAttach` 호출 시 `autoAttach: false`, `flatten: true`를 지정하여 수면(asleep) 상태의 탭이 자동으로 연결되지 않도록 방지한다.
  - 명시적 타깃 연결(`Target.attachToTarget`) 시 `flatten: true`를 적용하며, 전체 attach 및 설정 future는 타임아웃과 race시켜 작업 전체 시간에 상한을 둔다(기본 데드라인 12 s, 테스트 시 설정 가능).
- **수면 탭 처리 한계**:
  - 순정(stock) Chromium의 `TargetInfo`는 discarded/unloaded 플래그를 외부에 노출하지 않는다.
  - 따라서 컨트롤러는 `mark_asleep` 원장(ledger) 상태를 기록해 연결을 차단하고 실시간 타깃 메타데이터 준비 상태를 대조한다. 이는 OS나 브라우저의 내부 폐기 동작을 자동으로 감지하는 메커니즘이 아니다.
- **탭 수명주기 및 소유권**:
  - 에이전트 탭은 `newWindow: true`, `background: true` 옵션과 고정 뷰포트로 생성된다.
  - 탭 닫기는 소유 세션 및 `opened_by: agent` 상태를 만족해야 허용된다.
  - 팝업은 소유권이 `external`로 분류되지만 라우팅을 위해 부모 소유자 세션을 상속받는다(에이전트가 임의로 닫을 수 없음).
  - 탭 수가 5개 이상(`>=5`)이 되면 경고를 발생시킨다.
  - `BrowserEvent` 채널은 탭 열림/닫힘, 팝업, 다운로드 시작/완료, 대화상자(dialog), 탭 초과 경고 등을 통지한다(데몬 매핑은 후속 작업).
- **스크린샷 및 종료 격리**:
  - 동시 캡처 간 간섭을 방지하기 위해 포커스 전환, 캡처, 리셋 처리는 직렬화되어 실행된다.
  - 스크린샷 캡처 중에는 일시적으로 focus emulation을 활성화하며, 호출자가 작업을 취소하거나 오류가 발생하더라도 별도 태스크가 관리하는 finally 경로에서 비활성화를 시도하고 실패 시 오류를 반환한다. 순정 CDP가 패치된 브라우저 수준의 포커스 중재 보장을 제공하지는 않는다.
  - 명시적 종료 시 Chromium 프로세스 종료를 대기하며, `Drop` 구현체는 브라우저 전용 프로세스 그룹 전체를 정리하고 임시 프로필 디렉터리를 삭제한다.

## 6. 재현 및 검증 방법

### 측정 재현 명령어

저장소 루트 디렉터리에서 아래 명령어를 실행하여 크기 측정을 재현할 수 있다.

```sh
env CARGO_BUILD_JOBS=8 CARGO_TARGET_DIR=/tmp/openside-target/Browser cargo test -p openside-browser --test chromium snapshot_size_measurements -- --exact --nocapture
```

- 실행 결과: `1 passed, 0 failed, 0 ignored, 5 filtered out`
- 해당 테스트는 측정 테이블을 표준 출력에 표시하며, 본 문서 파일을 직접 덮어쓰지는 않는다.
- 시스템에 Chromium이 없으면 건너뛰지(skip) 않고 명확한 안내 메시지와 함께 실패한다.
- 동적 로컬 포트 할당 및 Chromium 버전에 따라 raw 바이트 수치에 미세한 차이가 발생할 수 있으며, 이 테이블은 1회 측정 기록이므로 고정 바이트 회귀 단언 대상이 아니다.

### 검증 실행 이력

다음 검증 명령어를 수행하여 코드 품질과 테스트 통과를 확인했다.

```sh
cargo fmt -p openside-browser
env CARGO_BUILD_JOBS=8 CARGO_TARGET_DIR=/tmp/openside-target/Browser cargo clippy -p openside-browser --all-targets -- -D warnings
env CARGO_BUILD_JOBS=8 CARGO_TARGET_DIR=/tmp/openside-target/Browser cargo test -p openside-browser -- --nocapture
pgrep -af user-data-dir=/tmp
which agent-browser
```

- **코드 검사 및 린트**: 경고 없이 통과.
- **테스트 결과**: 단위 테스트 3건 + Chromium 통합 테스트 6건 전체 통과 (`0 failed, 0 ignored, 0 doc tests`).
  - 단위 테스트 검증 항목:
    - 연결 취소 시 대기 발신자 정리 및 지연 연결 발생 시 정상적인 연결 해제 처리.
    - `screenshot_errors_still_attempt_focus_reset`: 포커스 활성화 실패 및 스크린샷 캡처 실패 상황 모두에서 정상적으로 focus-emulation false 전송을 시도하는지 검증.
  - 통합 테스트 검증 항목: 브라우저 실행/연결/종료, Drop 시 프로세스 그룹 및 임시 프로필 정리, 독립 윈도우/백그라운드 비활성, 뷰포트 설정, `agent|user|external` 소유권 모델, 탭 5개 이상 경고, 원장 수면 게이트, 즉시 타임아웃(zero-duration) 레이스 처리, 안정적인 참조 번호 및 DOM 변경 diff, 비활성 탭 PNG/포커스 리셋, 팝업/다운로드/대화상자/탭 닫힘 이벤트.
- **프로세스 잔여물 확인**: 전체 테스트 실행 후 `pgrep -af user-data-dir=/tmp` 실행 결과 출력이 없으며 종료 코드 1 반환(잔여 브라우저 프로세스 없음 확인).
- **외부 도구 확인**: `which agent-browser` 결과 출력 없음(종료 코드 1). 따라서 `agent-browser 미설치로 비교 생략` 문구를 확정 적용했다.
