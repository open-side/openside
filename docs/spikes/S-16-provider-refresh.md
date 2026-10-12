# S-16 broker 모드의 Codex refresh 차단

> 이 문서에 나오는 `spikes/` 아래 임시 스크립트와 내부 개발 기록(`docs/GOAL.md`, `docs/PROGRESS.md`, `docs/archive/`)은 공개 저장소에 포함되어 있지 않으며, 실행 명령과 출력 결과는 작업 과정을 확인하는 기록용으로 남겨 둔다.

- 작성일: 2026-10-11
- 대상: Python sidecar 유지보수자

## 검증 범위 및 환경
- 실행 환경: `pydantic-ai-slim 2.55.0`, `httpx2 2.13.1` 설치 환경.
- 검증 방식: 모의 인증 정보(synthetic credentials) 및 `httpx2.MockTransport` 기반 오프라인 단위 검증이다. 실제 네트워크 소켓 연결, 실서버 LLM 호출, 실제 broker 통신은 일절 배제하며 토큰 값 자체도 출력하지 않는다.
- 소스 코드 인용 기준: 별도 언급이 없는 한 인용 경로는 `agent/.venv/lib/python3.13/site-packages/pydantic_ai/` 기준 상대 경로이며, Anthropic SDK 인용은 상위 `agent/.venv/lib/python3.13/site-packages/` 기준이다. 단독 콜론 행 번호(`:line`) 표기는 직전에 언급된 파일을 가리킨다.
- 스파이크 스크립트: `spikes/sidecar/s16_refresh.py`
- 재현 명령(저장소 루트 기준): `agent/.venv/bin/python spikes/sidecar/s16_refresh.py` (종료 코드 0 확인 완료).
- 측정 범위: 프로바이더의 HTTP 인증 제어 흐름을 원시 http-client POST 요청으로 검증한다. 실제 `OpenAICodexModel`의 텍스트 생성, 페이로드 직렬화, 스트리밍 처리를 실행한 것은 아니다. `models/openai_codex.py:27-74`를 전수 검토한 결과(SOURCE INSPECTED), 해당 클래스는 `OpenAIResponsesModel`을 상속받아 세션 고정(session affinity) 설정만 특화할 뿐 자체 토큰 갱신 로직을 두지 않으며, 모든 갱신 처리는 프로바이더 인증 계층(`providers/openai_codex.py`)에 위치한다.

## 기본 동작(Stock Provider) 분석
기본 `OpenAICodexProvider(credential_source=...)`는 센티널(sentinel) 값을 감지하여 분기하는 것이 아니라, 이를 일반 refresh token으로 취급하여 갱신 절차를 그대로 실행한다.

1. **프로토콜의 한계**: `credential_source` 프로토콜은 저장소 인터페이스만 공급할 뿐 갱신 정책(refresh policy)을 통제하지 않는다(`providers/openai_codex.py:303-325`).
2. **지연 로드 및 만료 판정**: 지연 초기화(lazy init) 시 1회 로드한다(`:598-605`). `_prepare_request_credentials`는 먼저 자격 증명을 로드(`_load_if_needed`)한 뒤 만료 여부를 판정(`_refresh_if_stale`)하고 리비전과 실패 횟수를 캡처한다(`:582-605`). JWT exp 필드가 30초 이내이면 stale 상태로 간주하지만, 이는 서명 검증을 거치지 않은(signature unverified) 디코딩 결과에 기반한 힌트일 뿐 권위 있는 판정 기준이 아니다(`:74-75, 682-686`).
3. **자체 갱신 루틴 및 락**: `_refresh_if_stale`은 첫 번째 이벤트 루프에 바인딩된 프로바이더 인스턴스 락을 획득하고 다시 조건을 점검한다(`:607-611, 613-630`). `_refresh_locked`는 소스를 다시 로드한다(`:656-675`). 만약 저장소에서 읽어온 자격 증명 데이터클래스 전체가 변경되었다면, `_replace(stored)`를 통해 인메모리 교체만 수행하고 OAuth 엔드포인트 요청을 건너뛴다. 반면 자격 증명이 동일하다면 `_refresh_credentials`를 호출하여 token 엔드포인트에 `refresh_token`을 실어 POST 요청을 보낸다(`:282-300, 668`). 이어서 `_replace(...)`를 호출해 인메모리 자격 증명을 교체하고 리비전을 1 올린 뒤(`:677-680`), 저장소의 `save()`를 호출한다(`:669-675`). 즉 `OAuth -> in-memory replacement 및 revision 증가 -> save` 순서로 진행된다.
4. **결론**: `save()` 단계에서 예외를 던지는 방식은 외부 갱신 요청이 이미 전송된 뒤이므로 차단 시점으로 너무 늦다. 또한 빈 문자열이나 센티널 refresh token을 넘기더라도 외부 갱신 자체를 차단하지 못한다. 모의 환경에서 관측된 `invalid_grant`는 `httpx2.MockTransport`가 반환한 결과이며 실제 서버가 항상 동일한 오류를 낸다는 증거는 아니다.

### 선제적 갱신(Proactive)과 401 갱신의 오류 처리 차이
- **선제적 갱신 오류 무마**: 선제적 갱신(`_refresh_if_stale`) 중 발생하는 일반 갱신 오류는 내부에서 무마(swallow)된다. 오직 `CredentialsPersistenceError`만 상위로 전달되며, 내부 `_refresh_failures` 카운터는 증가하지 않는다.
- **401 응답 경로의 실패 캐싱**: 401 처리 경로(`:632-654`)는 갱신 실패 시 `_refresh_failures`를 증가시키고 예외를 캐싱한다. 이 캐시된 예외는 갱신 진행 중 겹쳐서 대기하던 요청(overlapping waiters) 사이에만 공유된다. 동일 리비전의 후속 요청이라 하더라도 업데이트된 `_refresh_failures`를 정상적으로 포착하면 다시 갱신을 시도할 수 있다.
- **다중 프로세스 한계**: HTTP 전송 계층은 대상 HTTPS 호스트를 엄격히 제한하고 본문을 버퍼링하며 401 응답 시 1회 재전송을 허용한다(`:412-454`). 프로바이더 인스턴스별 루프 바운드 락(`:460-463, 607-611`)으로 단일 런타임 내 동시 401 요청은 제어하지만, 소스 프로토콜에 경고되어 있듯(`:310-313`) 다중 프로세스 환경의 레이스 컨디션은 방어하지 못한다.

### 기본 동작 관측 결과
- **만료 임박 및 소스 미변경**: load 2회, save 0회, OAuth 요청 1회 발생, sentinel 제출(`submitted=True`), Codex 1회 호출, 최종 200 반환 (선제적 갱신의 `invalid_grant` 실패가 무마됨).
- **만료 임박 후 소스 변경(로테이션)**: load 2회, OAuth 0회, Codex 1회 호출 성공.
- **401 수신 및 소스 미변경**: load 2회, OAuth 1회 발생, `CredentialsRefreshError` 발생, `_refresh_failures` 1 기록.
- **저장(save) 거부 시점**: load 2회, save 1회(예외 발생), OAuth 1회 이미 발생함, Codex 0회, `CredentialsPersistenceError` 발생.
- **동시 401 발생**: load 2회, OAuth 1회 발생, Codex 2회, 두 요청 모두 `CredentialsRefreshError` 발생, `_refresh_failures` 1 기록.

## 확정된 회피 설계 (Public Opt-Out)
Main이 확정한 바와 같이 내부 메서드를 오버라이드하거나 런타임을 몽키패치하지 않고, 공식 생성자 규약에 따른 opt-out 방식을 채택한다. 생성자 수준에는 갱신을 비활성화하는 별도의 공개 콜백이나 플래그가 존재하지 않는다.

```python
http = httpx2.AsyncClient(auth=BrokerOnlyAuth(source))
client = AsyncOpenAI(
    base_url=CODEX_URL,
    api_key="unused-placeholder",
    http_client=http,
    max_retries=0,
)
provider = OpenAICodexProvider(openai_client=client)
```

1. **공식 생성자 규약**: `OpenAICodexProvider(openai_client=...)` 형태로 주입하면 기본 자격 증명 주입 및 자체 갱신 로직이 완전히 비활성화된다(`providers/openai_codex.py:520-536`). 이때 `credentials`, `credential_source`, `http_client` 인자는 상호 배타적이므로 함께 전달할 수 없다.
2. **비 opt-out 주입 제약**: opt-out이 아닌 기본 생성 모드에서는 auth가 구성된 `http_client`를 넘길 경우 생성자 검증에서 거부된다(`:552-561`).
3. **상태 분리**: 소스는 프로바이더가 아닌 애플리케이션 `BrokerOnlyAuth`에 직접 연결된다. 프로바이더 수준의 `provider.credentials` 속성은 조회할 수 없다(`:483-492`).
4. **모델 주입 규약**: 사용자 정의 프로바이더를 넘길 때 `OpenAICodexModel(configured_model_id, provider=custom_provider)` 형태로 구성할 수 있음은 소스 코드상 확인되었다(SOURCE INSPECTED, `models/openai_codex.py:37-53`). 다만 실제 모델 런타임 동작이 완벽히 동일하게 유지되는지는 본 스파이크에서 실측 검증하지 않았다(unverified).

### 애플리케이션 레벨 BrokerOnlyAuth 구현 규약
- **대상 제한 및 헤더 주입**: 허용된 Codex HTTPS 호스트로만 요청을 제한한다. Bearer 토큰과 계정 헤더(account header)를 주입하며, 토큰 로테이션 시 두 헤더를 함께 갱신한다. `originator` 헤더 주입 책임을 직접 진다.
- **선제적 만료 점검**: 만료 시각이 30초 이내이면 소스를 다시 읽어온다. 만료된 스냅샷이 그대로 유지되어 있다면 즉시 요청을 차단한다(fail-closed).
- **401 재시도 제어**: 401 응답 시 소스를 다시 읽어 완전히 변경된 자격 증명 세트로 교체한 뒤 단 1회만 재요청한다. 변경이 없거나 만료된 상태라면 즉시 실패 처리한다.
- **단일 비행 락**: 인스턴스 및 이벤트 루프 범위의 `asyncio.Lock`을 두어 동시 401 상황에서 단일 비행으로 소스를 반영한다. 이는 프로세스 간 상호 배제를 보장하지는 않는다.
- **외부 갱신 차단**: OAuth 엔드포인트 호출 및 `source.save()` 호출을 일절 수행하지 않으며 로컬 CLI fallback을 배제한다. 기본 프로바이더의 자체 갱신 코드는 opt-out 구조상 호출 경로에서 완전히 배제된다.
- **예외 매핑 및 주의사항**: 스파이크 코드의 가상 JWT 파서는 검증용 픽스처일 뿐이며 실제 환경에서는 broker 스냅샷의 만료 메타데이터를 사용해야 한다. 스파이크는 fail-closed 상황에서 `CredentialsRefreshError`를 발생시키며, 제품 수준에서 이를 `AuthUnavailable`로 매핑하는 규칙은 미검증(unverified) 상태이다. 제품 수준의 클라이언트 수명 주기 관리, 고정(pinning) 및 선택 로직 역시 아직 구현되지 않았다(unverified).

### Broker 모드 검증 결과
- **만료 직전 소스 변경**: load 2회, OAuth 0회, save 0회, Codex 호출 1회 성공(200).
- **만료 상태 소스 미변경**: load 2회, OAuth 0회, Codex 0회 (요청 차단 및 fail-closed).
- **401 수신 후 소스 변경**: load 2회, OAuth 0회, Codex 2회 (초기 401 후 갱신본으로 200 반환), Bearer 토큰과 계정 헤더 동시 갱신 확인.
- **401 수신 후 소스 미변경**: load 2회, OAuth 0회, Codex 1회 (재요청 차단 및 fail-closed).
- **동시 401 발생**: load 2회, OAuth 0회, Codex 4회 (동시 401 수신 후 동시 갱신 재요청으로 둘 다 200 성공).
- **외부 호스트 및 평문 요청**: 자격 증명을 일절 주입하지 않으며 소스도 읽지 않음 (모의 전송 계층의 일반 HTTP 요청 처리와 소스 접근 여부를 엄격히 분리하여 확인).

### 대안 비교
- **내부 `_refresh_locked` 재정의**: 기본 락 및 401 흐름을 재활용할 수 있으나 비공개 메서드 결합으로 취약하며, 생성자에 갱신 비활성화 콜백이나 플래그가 공개되어 있지 않다.
- **몽키패치**: 런타임 안정성을 해치므로 기각한다.
- **수시 변경 가상 자격 증명 트릭**: 실제 정책을 왜곡하며 401 갱신을 안정적으로 차단하지 못한다.
- **`save()` 예외 발생**: 토큰 요청 전송 이후에 호출되므로 차단 시점으로 부적합하다.
- **선택**: 공식 API인 `openai_client` 주입 opt-out을 채택한다.

## Anthropic
`AnthropicProvider`는 생성자에서 `auth_token`이나 기본 헤더를 직접 받는 인자를 제공하지 않는다(`providers/anthropic.py:144-196`). 대신 클라이언트를 직접 주입할 수 있다(`:179-182`).

```python
dedicated_http = httpx2.AsyncClient()
dedicated_client = AsyncAnthropic(
    auth_token=access_token,
    default_headers=configured_headers,
    http_client=dedicated_http,
    max_retries=0,
)
provider = AnthropicProvider(anthropic_client=dedicated_client)
```

1. **클라이언트 주입 규약**: `anthropic/_client.py:599-615`(상위 site-packages 기준)의 `AsyncAnthropic` 생성자에 `auth_token`을 넘기면 환경 변수 조회가 비활성화되며(`:657-669`), `Authorization: Bearer` 헤더가 설정된다(`:812-822`).
2. **헤더 주의사항**: OAuth 토큰을 `api_key`에 넣어서는 안 된다. `api_key`는 `X-Api-Key` 헤더를 생성하므로 OAuth 규격과 맞지 않는다.
3. **토큰 교체 방식(SOURCE INSPECTED ONLY)**: `provider._client`를 비공개로 직접 수정하는 방식을 피하고, broker 토큰이 변경될 때 구성된 모델 ID를 사용하여 새 SDK 클라이언트, `AnthropicProvider`, `AnthropicModel` 객체를 다시 조립한다. 기존 헤더와 시스템 프롬프트 설정을 유지한 채 요청 경계에서 교체하며, 이전 클라이언트는 진행 중인 요청이 끝난 뒤 닫는다. 이 구성 및 호출 절차는 소스 코드 검토 기반이며 본 스크립트에서 실제 호출을 실행하지 않은 미검증(unverified) 영역이다. docs/08 및 S-14의 요청 성형(shaping) 분리 보존 여부 역시 실측 검증되지 않았다(unverified).

## 재현 실행 결과
아래 출력은 `agent/.venv/bin/python spikes/sidecar/s16_refresh.py`를 실행하여 얻은 결과이다.

```text
pydantic-ai-slim: 2.55.0 httpx2: 2.13.1
stock expired unchanged: outcomes=200 loads=2 saves=0 oauth-hits=1 codex-hits=1 sentinel-submitted=True
  order: load -> load -> oauth-refresh -> codex-request
  refresh-failures: 0
stock near-expiry unchanged: outcomes=200 loads=2 saves=0 oauth-hits=1 codex-hits=1 sentinel-submitted=True
  order: load -> load -> oauth-refresh -> codex-request
  refresh-failures: 0
stock expired source rotated: outcomes=200 loads=2 saves=0 oauth-hits=0 codex-hits=1 sentinel-submitted=False
  order: load -> load -> codex-request
  refresh-failures: 0
stock 401 unchanged: outcomes=CredentialsRefreshError loads=2 saves=0 oauth-hits=1 codex-hits=1 sentinel-submitted=True
  order: load -> codex-request -> load -> oauth-refresh
  refresh-failures: 1
stock save refusal too late: outcomes=CredentialsPersistenceError loads=2 saves=1 oauth-hits=1 codex-hits=0 sentinel-submitted=True
  order: load -> load -> oauth-refresh -> save
  refresh-failures: 0
stock concurrent 401 single-flight failure: outcomes=CredentialsRefreshError,CredentialsRefreshError loads=2 saves=0 oauth-hits=1 codex-hits=2 sentinel-submitted=True
  order: load -> codex-request -> codex-request -> load -> oauth-refresh
  refresh-failures: 1
broker expired source rotated: outcomes=200 loads=2 saves=0 oauth-hits=0 codex-hits=1
  order: load -> load -> codex-request
  foreign/plaintext bypass: no credentials and no source reads
broker near-expiry source rotated: outcomes=200 loads=2 saves=0 oauth-hits=0 codex-hits=1
  order: load -> load -> codex-request
  foreign/plaintext bypass: no credentials and no source reads
broker expired unchanged fail-closed: outcomes=CredentialsRefreshError loads=2 saves=0 oauth-hits=0 codex-hits=0
  order: load -> load
  foreign/plaintext bypass: no credentials and no source reads
broker 401 source rotated: outcomes=200 loads=2 saves=0 oauth-hits=0 codex-hits=2
  order: load -> codex-request -> load -> codex-request
  replay updated bearer and account headers: True
  foreign/plaintext bypass: no credentials and no source reads
broker 401 unchanged fail-closed: outcomes=CredentialsRefreshError loads=2 saves=0 oauth-hits=0 codex-hits=1
  order: load -> codex-request -> load
  foreign/plaintext bypass: no credentials and no source reads
broker concurrent 401 single-flight adoption: outcomes=200,200 loads=2 saves=0 oauth-hits=0 codex-hits=4
  order: load -> codex-request -> codex-request -> load -> codex-request -> codex-request
  replay updated bearer and account headers: True
  foreign/plaintext bypass: no credentials and no source reads
```

## 미검증 영역 (Unverified)
- 실제 운영 broker의 갱신 주기 및 네트워크 실패 상황 대응 (unverified)
- 실제 Anthropic/OpenAI 엔드포인트를 통한 LLM 생성, 페이로드 직렬화, 스트리밍, 재시도, 리다이렉트 처리 (unverified)
- Anthropic SDK 기반의 클라이언트 조립 및 실시간 OAuth 토큰 로테이션 라이브 검증 (unverified)
- broker 만료 메타데이터의 형식 변환 및 `AuthUnavailable` 예외 매핑 규칙 (unverified)
- 데몬 프로세스 비정상 종료 시 재시작 및 인증 상태 복구 흐름 (unverified)