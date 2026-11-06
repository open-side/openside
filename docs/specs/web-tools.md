# `websearch` / `webfetch` — 웹 검색과 페이지 읽기

상태: **구현됨**. 코드: `crates/opensided/src/tools/web.rs`(도구·라우팅·직접 HTTP), `crates/opensided/src/tools/web_text.rs`(HTML→텍스트 변환·페이지 스크립트), `crates/openside-config/src/lib.rs`(`[search]`), `agent/src/features/research.ts`(계획 게이트·방문 출처). 테스트: `crates/opensided/src/tests/web_tools.rs`, `tools/web.rs`·`web_text.rs` 단위 테스트, `core_gaps.rs` redaction 검증.

## 1. 도구

두 도구 모두 `read` 등급이며, 정의되지 않은 인수 필드는 거부한다.

- `websearch {query}`: 검색어의 연속된 공백은 하나로 합치며 길이는 1~400자다. `{query, provider, engine, results:[{id, title, url, snippet, published}], note}`를 반환한다. 결과는 최대 8개까지 중복을 제거해 담으며, `http(s)` URL만 허용한다. 마크업을 제거한 뒤 `title`은 300자, `snippet`은 500자 이내로 자른다. 결과를 읽지 못했을 때는 브라우저 도구로 어느 페이지를 직접 열어야 하는지, API를 쓰지 않았을 때는 왜 쓰지 않았는지를 `note`에 그대로 적는다.
- `webfetch {url}`: 자격 증명이 없는 4096자 이하의 절대 `http(s)` URL만 받으며 프래그먼트는 버린다. `{url, title, status, text, truncated}`를 반환한다. `url`은 리다이렉트를 거친 최종 URL이다. HTML은 일반 텍스트로 변환한다. `script`·`style`·`noscript`·`template`·`svg`·`iframe`·`nav`·`button`·`select` 요소는 버리고, 제목은 `#` 줄로, 목록 항목은 `- `로 바꾸며, `<main>`이나 `<article>` 본문이 200자 이상이면 그 텍스트를 쓴다.
- `text`는 24,000자 또는 JSON 문자열로 이스케이프했을 때 24 KiB 중 먼저 닿는 한도에서 자른다(잘렸을 때 `truncated`는 `true`다). 한중일(CJK) 문자나 따옴표가 많은 페이지에서도 `url`과 `title`을 합친 전체 결과가 호스트의 32 KiB 출력 한도 안에 들어 인용 가능한 상태로 남도록 하기 위해서다. 결과에는 다른 페이지 읽기 도구(`browser_user_tab_snapshot`, `memory_search`)와 같은 데이터 전용 봉투인 `untrusted: true`가 붙으며, 페이지 본문은 `text` 안에 인용된 데이터일 뿐 지시문으로 취급하지 않는다. `websearch` 결과에도 마찬가지로 `untrusted: true`가 붙는다.
- 노출 조건: `webfetch`는 항상 노출하고, `websearch`는 에이전트 브라우저가 있거나 API 제공자가 설정됐을 때 노출한다. 두 도구 모두 `browser-agent`와 `ultrabrowse` 프로필에 들어간다.

## 2. `websearch` 백엔드

- `browser`(기본값): 일시적인 백그라운드 에이전트 탭을 열어 `[search] engine`(또는 브라우저 기본 검색 엔진 / DuckDuckGo)의 결과 페이지로 이동한다. 격리된 실행 환경(isolated world)에서 외부 사이트로 향하는 링크 안팎의 제목을 읽고, 검색 엔진 자체의 리다이렉트 링크(`uddg`, `url`, `q`, Bing base64 `u`)를 원래 주소로 복원한 뒤 탭을 닫는다. 정책 행동은 결과 페이지 origin에 대한 `Browse`다.
- `brave` / `tavily`: 시크릿 세션이 아니고, Vault가 잠금 해제 상태이며, API origin과 일치하는 Vault 항목이 정확히 하나일 때만 쓴다. 키는 감사가 기록되는 Vault 입력 경로(`AuditAction::Fill`)로 읽어 해당 origin으로만 보내며, 도구 결과나 이벤트에는 남지 않고 모델에도 전달하지 않는다. 정책 행동은 엔드포인트 URL에 대한 `Fetch`와 API origin에 대한 `VaultUse`다.
- 경로는 정책 검사 전에 결정하고 실제 실행 시점에 다시 확인한다. 그사이에 경로가 바뀌었으면 `web_route_changed`로 호출을 거부한다.
- 데몬이 보유한 실시간 시크릿 값이 검색어에 들어 있으면 호출을 거부한다(`websearch_secret_in_query`).

## 3. `webfetch` 경로

- 직접 요청(Direct): `[network] allow` 또는 루프백이 URL을 허용할 때 쓴다. 쿠키나 자격 증명을 보내지 않고 프록시도 거치지 않으며, `identity` 인코딩으로 최대 2 MiB까지 읽는다. DNS 응답도 모두 허용 범위에 들어야 한다. 루프백 이름은 루프백 주소로만 연결되고, 그 외 이름은 호스트가 `network.allow`에 명시되지 않은 한 공인 주소로만 연결되며, 소켓은 검증을 통과한 주소에 고정한다. 홉이 `network.allow` 밖으로 벗어나거나 새로운 승인이 필요하면 요청을 멈추고 새 URL로 `webfetch`를 호출하라는 안내와 함께 거부한다. 정책 행동은 해당 URL에 대한 `Fetch`다.
  - 첫 요청을 포함한 모든 홉은 실제 I/O 시점에 실시간 시크릿 포함 여부, `network.allow`, 실시간 정책(`Fetch`)을 다시 검사한다. 호출이 먼저 허용됐더라도 그 뒤 `PUT /v1/policy`로 거부 규칙이 반영되면 요청을 즉시 멈춘다. 리다이렉트는 최대 5번까지 따라간다. `https`→`http` 전환은 루프백 호스트끼리만 허용하며(루프백→루프백, 예: 로컬 개발 서버), 그 외에 보안 수준이 낮아지는 리다이렉트는 모두 거부한다(`webfetch_insecure_redirect`). 루프백 호스트 판정은 `Egress`가 허용하는 기준(`localhost`, `*.localhost`, 끝 마침표, IPv4 매핑 루프백)을 그대로 따른다.
  - 주소 검사는 `net_guard::public_ip`를 쓴다. 호스트가 `network.allow`에 명시되지 않은 한, 일반 호스트 이름이 사설, 링크 로컬, CGNAT, 문서용(documentation), NAT64, 6to4, IPv4 매핑 사설 주소로 해석되면 연결을 거부한다.
  - DNS 조회, 주소별 연결 시도, 리다이렉트를 모두 합쳐 직접 요청 한 번의 전체 제한 시간은 20초(`web_timeout`)다. 각 홉은 주소를 최대 4개까지 시도하며 주소당 연결 타임아웃은 5초다.
  - 텍스트 형식(HTML, `text/*`, JSON, XML)만 읽는다. `Content-Type` 헤더가 없는 응답 본문은 첫 1024바이트를 검사해 HTML이거나 제어 문자가 없는 UTF-8 텍스트일 때만 읽으며, 그 외의 경우나 허용되지 않은 `Content-Type`이 선언된 경우에는 모두 `webfetch_unsupported_content`로 실패한다.
- 브라우저(Browser): 직접 요청 대상이 아니면서 에이전트 브라우저가 있을 때 쓴다. 일시적인 에이전트 탭에서 렌더링된 텍스트를 읽는다. 브라우저 탐색 정책이 매 리다이렉트를 검사하고 본문을 읽기 전에 최종 origin을 다시 검사하며, OTP 설정 페이지는 브라우저 도구와 동일하게 보호한다. 정책 행동은 해당 origin에 대한 `Browse`다.
  - 탭 로드는 45초 안에, 본문 읽기도 45초 안에 끝나야 한다(`web_page_timeout`). 페이지 스크립트는 문서 전체의 `innerText`를 한꺼번에 만들지 않고 화면에 보이는 텍스트 노드를 순회하며, 24,001자 또는 텍스트 노드 50,000개에 도달하면 멈춘다. 페이지 제목은 페이지 밖으로 내보내기 전에 1000자에서 자른다.
- 두 경로 모두 쓸 수 없으면 `webfetch_not_allowed`로 거부한다.
- 실시간 시크릿이 담긴 URL은 요청을 보내기 전에 거부한다.

## 4. 인용 기록 (`tool.ended` 결과 = `for_app`)

- `websearch`: `{status:"searched", query, provider, engine, count, sources:[{id, title, url, site, origin, published, favicon_ref}]}`.
- `webfetch`: `{status:"fetched", url, title, site, origin, favicon_ref, http_status, chars, truncated, via:"direct"|"browser"}`. 페이지 본문 텍스트는 이벤트에 들어가지 않는다.
- `favicon_ref`는 인라인 `data:image/png` 참조다(공개 아이콘 라우트는 카드가 참조하는 origin만 제공한다). 에이전트 탭에서 페이지를 읽었을 때는 그 탭의 아이콘을 쓰고, 그 외에는 세션에 이미 캐시된 아이콘을 쓴다. 인용 정보를 만들려고 인용 대상 사이트에 접속해 아이콘을 가져오지는 않는다.
- 결과의 URL에서는 자격 증명과 프래그먼트를 제거하며, 쿼리 키가 자격 증명을 가리키면 쿼리도 제거한다.

## 5. 리서치 모드 (`ultrabrowse`)

- `websearch`와 `webfetch`도 브라우저 도구와 마찬가지로 `Plan:` 게이트를 거친다.
- `webfetch`로 읽은 페이지는 인용 검사에서 방문한 출처로 치지만, `websearch` 결과는 치지 않는다(요약 조각은 페이지를 읽은 것이 아니다).
- 프롬프트는 여러 주제를 조사할 때 `websearch`로 시작해 `webfetch`나 브라우저로 출처를 읽도록 안내한다.

## 6. 안전 장치

- 두 도구 모두 브라우저 도구처럼 브라우저 게이트를 거친다. 페이지와 검색 결과 텍스트는 신뢰할 수 없는 데이터로 다루고, 레지스트리 redactor가 도구 결과·이벤트·거부 응답을 마스킹하며, 서브에이전트 진행 상태에는 `"Searched the web"` / `"Read a page"`를 표시한다.

## 7. 결정 사항

- 브라우저 경로는 `webfetch` 직접 요청 규칙이 아니라 브라우저 도구의 규칙을 의도적으로 따른다. 최상위 프레임 이동과 리다이렉트마다 실시간 origin 정책(`Browse`)을 검사하고, 탭이 최종 도달한 페이지에서 한 번 더 검사한다. 에이전트 브라우저에는 별도의 사설 주소 차단 장치가 없어 `browser_navigate`도 같은 정책 아래 사설 주소와 루프백 주소를 열 수 있으므로, 브라우저를 거치는 `webfetch`가 기존 브라우저 도구로 열 수 없던 곳에 새로 닿는 일은 없다. 사설 대역 제한은 `[policy]`(`deny_origins` 또는 규칙)가 담당하며 두 도구에 똑같이 적용된다.
- 검색 API 엔드포인트(`[search] api_url` 또는 제공자 기본값)는 사용자가 직접 작성한 신뢰할 수 있는 설정이므로 사설 주소 검사를 거치지 않고 `network.allow`에 등록할 필요도 없다. Vault 키는 설정된 바로 그 origin(스킴·호스트·포트)을 허용 origin으로 적은 Vault 항목이 정확히 하나일 때만 해당 origin으로 전송하며, API 리다이렉트는 절대 따라가지 않는다. 설정을 고칠 수 있는 사람은 이미 키를 보낼 대상을 정할 수 있으므로, Vault 항목의 허용 origin이 두 번째 독립 검증 역할을 한다.
