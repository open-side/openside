# 스냅샷 크기 벤치 (B-3)

`browser_snapshot` 이 에이전트에게 보내는 바이트를 agent-browser 와 비교한다. 같은 페이지와 같은 작업 순서로 두 도구를 돌린다.

## 무엇을 재나

- **페이지**: `pages/*.html` 정적 페이지 12개(블로그 목록, 결제 폼, 대시보드 표, 문서, 받은편지함, 포럼, 로그인, 뉴스, 상품 목록, 검색 결과, 설정 폼, 위키). 127.0.0.1 로만 띄운다.
- **작업 순서**: `workflow.json`. 페이지를 연 뒤 3단계를 페이지 JS 로 실행한다. 첫 텍스트 칸에 입력하고, 첫 체크박스를 켜고 상태 문구를 붙이고, 첫 제목에 문구를 덧붙인다. 두 도구 모두 같은 JS 를 실행하므로 DOM 변화가 같다.
- **판정 지표(합격 기준)**: 작업 전체에서 에이전트가 받는 스냅샷 바이트 합계. openside 는 처음에 전체 트리를 받고, 이후 단계마다 같은 문서의 diff 만 받는다(실제 `browser_snapshot` 기본 동작). agent-browser 는 단계마다 기본 `snapshot` 전체를 받는다. openside 합계 / agent-browser 합계 ≤ 0.35 를 페이지별로, 그리고 전체로 확인한다.
- **같이 공개하는 수치**
  - 스냅샷 1회 크기 비율(openside 전체 트리 / agent-browser 기본 스냅샷).
  - 원본 접근성 트리(CDP `Accessibility.getFullAXTree` JSON) 대비 openside 전체 트리 비율. ≤ 0.10 을 확인한다.
- **정확도**: 크기를 줄이느라 대상이 빠지면 안 된다. 각 페이지에서 로드 직후와 작업 후에 `a[href], button, input:not([type=hidden]), select, textarea, summary, [role=tab]` 에 해당하는 DOM 요소마다 스냅샷에 `[eN]` ref 가 있고 그 ref 가 같은 노드로 풀리는지 확인한다. 단계마다 diff 를 직전 스냅샷에 적용하면 새 전체 트리와 같아야 하고, ref 와 문서 루트는 바뀌지 않아야 한다.

## 결과

agent-browser 0.39.0, 시스템 Chromium 기준. 바이트는 UTF-8 기준이다.

| 항목 | openside | agent-browser | 비율 |
|---|---:|---:|---:|
| 작업 전체(로드 + 3단계) | 121,713 | 765,234 | **0.159** |
| 스냅샷 1회(로드 직후, 12페이지 합) | 114,339 | 190,630 | 0.600 |
| 원본 접근성 트리 대비(로드 직후) | 114,339 | 3,656,911(원본) | 0.031 |

- 작업 전체 비율은 페이지별로 가장 나쁜 곳(로그인 폼, 작은 페이지라 첫 전체 트리 비중이 크다)도 0.187 이다.
- 스냅샷 1회 비율은 페이지에 따라 0.36~0.75 다. 한 번 찍는 크기만 보면 0.35 를 넘는다. 이 페이지들은 본문 텍스트가 대부분이고, 텍스트는 두 도구 모두 그대로 보내야 하기 때문이다. openside 는 구조 쪽(목록 마커, 이름을 되풀이하는 정적 텍스트, 이름 없는 단일 자식 래퍼, 같은 이름의 라벨 텍스트, 내용에서 나온 이름)을 줄이고, 텍스트 노드는 ref 없이 따옴표 문자열로 보낸다. 0.35 이하는 diff 가 들어가는 작업 단위에서만 나온다.
- 대략적인 토큰 수(바이트/4)로는 작업 전체가 약 30K 대 191K 다.

## 다시 재기

openside 쪽(기준값 파일과 비교하고 표를 출력한다):

```
cargo test -j 8 -p openside-browser --test snapshot_bench -- --nocapture
```

Chromium 을 찾지 못하면 이유를 출력하고 건너뛴다.

agent-browser 기준값(`agent-browser-baseline.json`)을 다시 만들 때:

```
npm install agent-browser@0.39.0   # 임시 디렉터리에서
AGENT_BROWSER=<agent-browser 실행 파일> CHROMIUM=<chromium 실행 파일> \
  node bench/snapshot/agent-browser-baseline.mjs
```

스크립트는 페이지를 127.0.0.1 의 임시 HTTP 서버로 띄우고, `HOME` 을 새 임시 디렉터리로 돌려서 사용자 프로필과 설정을 건드리지 않는다. 페이지마다 `open` → `snapshot` → (단계마다 `eval` → `snapshot`) 를 실행해 각 `snapshot` 출력 바이트를 기록한다. 버전, 명령, 환경은 JSON 파일에 같이 적힌다.
