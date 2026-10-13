# browser_snapshot 스펙

## 개요

`browser_snapshot`은 탭의 접근성 트리를 간결한 텍스트로 직렬화해 반환한다. 노드마다 한 줄씩 `[eN] role "name"` 형태로 나타나며, 여기서 eN은 `browser_act`에서 쓸 수 있는 안정적인 ref다. 따옴표로 묶인 줄은 페이지 본문 텍스트를 뜻한다. 캡처할 때마다 새 `snapshot_id`를 발급한다. 결과는 직렬화한 JSON 기준 30 KiB 예산에 맞추므로 호스트가 중간에 자르지 않는다. diff(`since`), 경고(`warning`), 페이징, epoch 관련 일반 규칙은 `docs/09-tool-design.md`(B-3 항목)를 따른다. 이 문서는 새로 추가된 `viewport` scope의 동작과 측정 수치를 다룬다.

## 캡처 id 와 diff

온전한 캡처 결과는 첫 줄이 `[snapshot_id <id>]`로 시작하는 일반 텍스트다. `since` 옵션은 같은 대화 안에서 온전하게 전달된 캡처를 대상으로만 줄 단위 diff를 계산한다. 부분 캡처나 잘린 결과는 어떤 경우에도 diff의 기준점으로 삼을 수 없다.

## 페이지 읽기(paging)

예산(30 KiB)을 넘긴 결과에는 `warning` 필드가 붙는다. 나머지는 해당 시점의 불변 캡처를 가리키는 `snapshot_id`와 함께 `from_line`, `from_char`, `ref` 중 하나를 넘겨 이어서 읽는다.

## `scope: "viewport"`

### 인자 명세

| 인자 | 값 | 설명 |
| --- | --- | --- |
| `scope` | `"page"` (기본값) | 문서 전체를 캡처한다. |
| `scope` | `"viewport"` | 현재 화면(시각적 뷰포트)에 걸치는 노드와 그 조상 노드만 캡처한다. 새 캡처를 생성할 때만 지정할 수 있다. |

### 포함 대상

캡처 시점에 시각적 뷰포트(visual viewport)와 레이아웃 박스가 겹치는 노드만 남긴다. 트리의 계층 구조와 들여쓰기를 유지하기 위해 이들 노드의 모든 조상 노드와 루트 문서 노드도 함께 남긴다. `display: contents`처럼 레이아웃 박스가 없는 노드는 보존 대상 노드의 조상일 때만 포함한다. `fixed`나 `sticky` 요소는 실제 화면에 그려진 위치를 기준으로 판정하므로, 페이지를 스크롤해도 뷰포트 안에 남아 있다면 캡처에 그대로 포함된다.

### 수집 방식

페이지 안에서 스크립트를 실행하지 않고 CDP만 이용한다. `Accessibility.getFullAXTree` 직후 데몬이 `Page.getLayoutMetrics`(`cssVisualViewport`: `pageX`, `pageY`, `clientWidth`, `clientHeight`)와 `DOMSnapshot.captureSnapshot`(메인 문서 기준 문서 좌표계 레이아웃 경계)을 연달아 읽는다. 그런 다음 백엔드 DOM 노드가 뷰포트 사각형과 겹치는 접근성 노드만 걸러낸다. 라벨 병합이나 래퍼 압축 같은 축약(compaction) 작업은 걸러낸 노드를 대상으로만 실행한다.

### Ref 처리

남겨진 노드는 같은 문서의 전체 캡처와 완전히 같은 ref를 부여받는다. 제외된 노드는 새 ref 번호를 소비하지 않는다. 이전에 화면에 보여 ref를 발급받았던 노드가 스크롤 등으로 화면 밖으로 벗어나더라도, 노드가 DOM에 존재하는 한 `browser_act`에서 해당 ref를 그대로 지정해 조작할 수 있다(단지 현재 캡처 텍스트에 나타나지 않을 뿐이다). 반면 모델이 한 번도 본 적 없는 노드의 ref는 캡처에 출력된 적이 없으므로 호출할 수 없다.

### 반환 형식

예산 안에 들어온 온전한 뷰포트 캡처 결과는 첫 줄이 `[snapshot_id <id> viewport]`로 시작하는 일반 텍스트다. 뷰포트 캡처조차 예산을 초과하면 객체 형태로 반환하며, `snapshot_id`, `snapshot`, `warning`과 함께 `"scope": "viewport"` 필드를 추가로 포함한다.

### Diff 동작

`scope: "viewport"`를 지정하면 `since` 옵션은 무시한다. 스크롤 위치에 따라 뷰포트 영역이 바뀌므로 이전 캡처와 줄 단위 diff를 계산하면 서로 다른 페이지 영역이 뒤섞이기 때문이다. 따라서 이 경우 항상 현재 뷰포트 전체 내용을 반환한다. 뷰포트 캡처는 전체 트리를 온전히 담지 않으므로 이후 다른 캡처의 `since` 기준점으로 삼을 수 없다. 후속 호출에서 뷰포트 캡처의 `snapshot_id`를 `since`로 지정하면 diff 대신 새 전체 트리를 반환한다. 뷰포트 캡처 자체도 diff 형식으로 출력되지 않는다.

### 페이징 동작

뷰포트 캡처 데이터는 일반 캡처와 마찬가지로 데몬에 그대로 저장된다. `snapshot_id`와 `from_line`/`from_char`/`ref`를 함께 넘기면 해당 시점의 불변 뷰포트 캡처를 읽어 들이며, 뷰포트 표시(`[snapshot_id <id> viewport]` 헤더 또는 `"scope": "viewport"`)도 그대로 유지된다. 캡처 시점에 뷰포트 밖에 있던 노드는 페이징으로도 읽을 수 없다. 페이징 읽기는 기존 캡처의 scope를 유지하므로, `snapshot_id`와 `scope`를 동시에 전달하면 새 캡처 전용 인자라는 안내와 함께 요청을 거부한다.

### 보안 및 스크롤 처리

비밀값 입력 후 텍스트 필드 값을 마스킹하는 보안 처리는 전체 캡처와 동일하게 적용된다. 뷰포트 캡처는 문서 전체가 아닌 일부 조각이므로 방문 기록의 페이지 요약으로 색인하지 않는다(페이지 이동 기록 자체는 정상 반영된다). 스크롤한 뒤에는 새 뷰포트 캡처를 떠야 하며, 스크롤하더라도 기존 ref는 무효화되지 않는다.

## 측정

측정 환경: `bench/snapshot/pages`에 포함된 세 종의 긴 페이지 대상, 헤드리스 Chromium, 에이전트 뷰포트 크기 1440x900. 스크롤 0px 및 최대 스크롤 가능 구간의 중간 지점에서 전체 캡처와 뷰포트 캡처 크기를 비교했다. 크기 단위는 compact 스냅샷 텍스트의 바이트 수다(토큰 수는 바이트 수를 4로 나눈 추정치일 뿐이다).

| 페이지 | 스크롤 (px) | 전체 바이트 | 전체 줄 수 | 뷰포트 바이트 | 뷰포트 줄 수 | 뷰포트/전체 |
| --- | --- | --- | --- | --- | --- | --- |
| `product-listing.html` | 0 | 12043 | 447 | 3693 | 138 | 0.307 |
| `product-listing.html` | 1254 | 12043 | 447 | 4106 | 150 | 0.341 |
| `wiki-article.html` | 0 | 13149 | 199 | 2729 | 72 | 0.208 |
| `wiki-article.html` | 1492 | 13149 | 199 | 4133 | 31 | 0.314 |
| `email-inbox.html` | 0 | 11353 | 283 | 7194 | 186 | 0.634 |
| `email-inbox.html` | 310 | 11353 | 283 | 8065 | 188 | 0.710 |
| **합계** | | **73090** | | **29920** | | **0.409** |

해당 페이지들에서 뷰포트 캡처는 전체 텍스트 크기의 약 41% 수준으로 줄어든다(59% 감소, 6회 측정 합산 기준 약 43,000 바이트 및 11,000 토큰 절감). 절감 효과는 길이가 길고 본문 텍스트가 많은 페이지(`wiki-article.html` 상단 기준 21%)에서 가장 크며, 스크롤 길이가 짧은 페이지(`email-inbox.html`은 최대 스크롤이 620px에 불과해 뷰포트가 페이지 대부분을 덮음)에서는 상대적으로 작다.

상단 고정 헤더, 100px 간격의 버튼 60개, 피드 스크롤, 푸터로 구성한 긴 인공 페이지에서는 전체 캡처가 2606 바이트(107줄)인 반면, 뷰포트 캡처는 최상단에서 273 바이트(13줄), 3000px 스크롤 위치에서 314 바이트(14줄)로 전체 대비 10~12% 수준에 불과했다.

재현 명령:
```bash
cargo test -p openside-browser --test snapshot_bench viewport_snapshot -- --nocapture
```
(Chromium 호환 브라우저 필요. 테스트는 3개 페이지 전체 캡처 대비 뷰포트 캡처 합계가 50% 미만인지 검증한다.)

## 테스트

- `crates/openside-browser/src/snapshot.rs`:
  - `viewport_keeps_visible_nodes_with_ancestors_and_old_refs_resolve`
  - `nodes_in_view_uses_document_coordinates_and_main_document_only`
- `crates/openside-browser/tests/viewport.rs` (헤드리스 Chromium 실기 테스트): 고정 헤더 처리, 스크롤 위치별 캡처, ref 안정성 검증, 화면 밖 노드의 기존 ref 해석 검증, 이후 전체 캡처 일관성 확인
- `crates/openside-browser/tests/snapshot_bench.rs`:
  - `viewport_snapshot_is_a_fraction_of_the_full_snapshot_on_tall_pages`
- `crates/opensided/src/tests/browser_snap.rs`:
  - `browser_snapshot_scope_viewport_is_partial_never_a_diff_base_and_keeps_refs` (데몬 수준: 마커 확인, `since` 무시, diff 기준점 배제, ref 페이징, scope+snapshot_id 전달 차단, 뷰포트 ref로 act 호출, 긴 페이지 페이징 대비 뷰포트 크기 비교)

## 한계

- 좌표 판정은 부모 스크롤 컨테이너의 클리핑을 반영하지 않고 문서 좌표계 상의 자체 레이아웃 박스만 기준으로 삼는다. 내부 스크롤 컨테이너 안에서 스크롤되어 가려졌더라도 박스 좌표가 뷰포트 영역 안에 걸치면 캡처에 남는다. 반대로 내부 컨테이너 내용물이라도 뷰포트 아래에 위치하면 제외된다.
- 접근성 트리가 메인 프레임 기준이므로 레이아웃 정보도 메인 문서의 것만 읽는다.
- 페이지 상태와 캡처 시점이 완벽하게 동결되지 않는다. 접근성 트리를 가져온 직후 뷰포트 좌표를 읽기 때문에, 두 호출 사이에 페이지가 스크롤되거나 레이아웃이 바뀌면 불일치가 발생할 수 있다. 이 경우 캡처를 다시 수행해야 한다.
- `overflow: hidden`이나 `opacity: 0`을 기준으로 별도 클리핑하지 않는다. 숨겨진 노드는 이미 접근성 트리 단계에서 걸러진다.
