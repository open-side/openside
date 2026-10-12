# openside-agent

openside의 모델 실행 엔진인 Python Pydantic AI 사이드카(sidecar) 컴포넌트다.

## 아키텍처 개요

openside는 세션 관리, 보안 정책, 볼트(vault), 브라우저 제어, 티켓, 도구 실행을 Rust 데몬에서 전담한다. Python 사이드카는 모델 루프를 구동하며, 역방향 RPC(reverse-RPC) 프록시를 통해 데몬 측 도구를 호출한다.

## 설치 및 실행

저장소 루트 디렉터리에서 의존성을 설치한다.

```bash
uv sync --project agent
```

### 사이드카 실행

일반적으로 Rust 데몬이 환경변수(`OPENSIDE_SIDECAR_SOCKET`, `OPENSIDE_SIDECAR_NONCE`)를 주입하여 사이드카 프로세스를 직접 시작한다. 수동 실행 명령은 다음과 같다.

```bash
uv run --project agent python -m openside_agent
```

### 진단 도구 (doctor)

환경 설정 및 인증 소스 상태를 진단한다. 설정 JSON은 `OPENSIDE_DOCTOR_CONFIG` 환경변수로 전달한다.

```bash
OPENSIDE_DOCTOR_CONFIG='{"credentials":{},"models":{}}' uv run --project agent python -m openside_agent doctor --json
```

## 프로바이더 인증 소스 우선순위

인증 소스는 모델 인스턴스 빌드 시점(`build_model`)에 `credentials.order.<provider>` 설정 순서에 따라 결정된다. openside는 자격 증명을 디스크에 별도로 저장하지 않는다.

### 기본 탐색 순서

- `anthropic`: `["omp-broker", "env", "command"]`
- `openai-codex`: `["omp-broker", "codex-cli"]`
- 기타 프로바이더: `["env", "command"]`

### omp-broker 설정 및 우선순위

omp([oh-my-pi](https://github.com/can1357/oh-my-pi))는 별개의 오픈소스 코딩 에이전트 CLI이다. 이 프로젝트의 인증 브로커는 로컬의 다른 프로그램에 모델 OAuth 인증 정보를 제공한다. `omp-broker` 자격 증명 소스는 선택 사항이며, 이미 해당 브로커를 쓰고 있을 때만 유용하다. openside는 브로커 없이도 정상 동작하므로 브로커 소스 코드를 따로 받을 필요는 없다.

기본 엔드포인트(`http://127.0.0.1:8765`)와 토큰 파일 경로(`~/.omp/auth-broker.token`, `Path.home()` 기준)는 omp 사용자를 위한 선택적 후보 경로일 뿐이며, 필수 사항이 아니다. 토큰 파일이 없으면 브로커 소스는 오류 없이 조용히 비활성화된다.

설정값은 다음 우선순위로 적용된다.

- **브로커 URL**: `OPENSIDE_BROKER_URL` > `credentials.omp_broker.url` > 선택적 기본 후보(`http://127.0.0.1:8765`)
- **브로커 토큰**: `OPENSIDE_BROKER_TOKEN` > `credentials.omp_broker.token_env`에 지정된 환경 변수 > `credentials.omp_broker.token_file` > 기본 후보 파일(`~/.omp/auth-broker.token`, 선택적) 순으로 적용된다. 빈 값이면 다음 순서로 넘어간다.

#### 브로커 장애 및 다운그레이드 방지 정책

- 브로커 토큰이 설정되어 있으나 브로커에 접근할 수 없거나, 해당 프로바이더의 OAuth 계정이 모두 차단된 상태라면 즉시 `AuthUnavailable` 예외를 발생시킨다. 로컬 인증 소스(`env`, `command`)로 자동 다운그레이드하지 않는다.
- 브로커 스냅샷에 해당 프로바이더 자체가 없을 때만 빌드 시점의 다음 소스(`env` 등)로 넘어간다.
- 실행 도중 선택된 브로커 연결이 끊어지더라도 런타임 중간에 로컬 소스로 다운그레이드되지 않으며, `AuthUnavailable`은 `FallbackModel`의 대체 예외 대상에서 제외된다.

#### 브로커 동작 방식

- **스냅샷 캐시**: 메모리에 캐시하며 기본 TTL은 15초다(`credentials.omp_broker.snapshot_ttl_seconds`). SSE 실시간 스트림(`credentials.omp_broker.sse=true`)을 지원하며, 요청 타임아웃 기본값은 10초다(`credentials.omp_broker.timeout_seconds`).
- **토큰 갱신 전담**: OAuth 토큰 갱신은 전적으로 브로커가 처리한다. 사이드카는 만료 토큰이나 401 응답을 받으면 스냅샷을 다시 읽을 뿐, 갱신 엔드포인트나 OAuth 서버를 직접 호출하지 않는다.
- **계정 선택 정책**: 활성 OAuth 계정 중 만료되지 않은 차단 계정을 제외하고, 사용량 통계 추적이 가능하면 잔여 쿼터 최솟값이 가장 큰 계정을 선택한다. 세션 핀(`session pins`)은 유지되며, Anthropic 핀은 1시간 동안 유휴 상태가 이어지면 해제된다.
- **핀 직렬화**: `CredentialPins.to_json()` 및 `from_json()`은 프로바이더/세션 키, 크리덴셜 ID, 요청 시각만 포함하며 토큰, 계정 식별자, 이메일 정보는 다루지 않는다.
- **요청 제한(429) 처리**: 429 또는 `rate_limit` 발생 시 `BrokerClient.block`을 호출하여 해당 계정을 차단한 뒤 다른 계정으로 재시도한다. (테스트 환경에서는 가상 브로커를 상대로만 차단을 호출한다.)
- **고정 vs 쿼터**: 다른 계정에 잔여 쿼터가 더 많더라도 기존 세션에 고정(pin)된 계정이 차단되거나 사용할 수 없게 되기 전까지는 기존 고정을 유지한다.
- **전체 차단**: 해당 프로바이더의 모든 계정이 차단되면 `AuthUnavailable`을 반환하며, `env`로 다운그레이드하지 않는다.
- **브로커 쓰기 작업**: 허용된 유일한 쓰기 작업은 범위 한정 차단뿐이다. 사이드카는 자격 증명을 업로드, 갱신, 비활성화, 삭제하지 않는다.

브로커 게이트웨이 사양 및 타입 정의는 [oh-my-pi auth-broker-gateway 문서](https://github.com/can1357/oh-my-pi/blob/eec114a9226da698ebc015fd8996a8ca3bc4625c/docs/auth-broker-gateway.md) (§auth-broker Endpoints) 및 `packages/ai/src/auth-broker/types.ts`를 참고한다.
지원 라우트:
- `GET /v1/snapshot`
- `GET /v1/snapshot/stream`
- `GET /v1/usage`
- `POST /v1/credential/:id/block`

openside가 소비하는 통신 필드는 `docs/specs/omp-broker-wire.schema.json`에 고정되어 있으며 `tests/providers/test_broker_wire.py`로 검증한다. omp 업그레이드 뒤 GET 요청만 보내 확인하는 선택형 점검 명령어는 `uv run pytest -q -m live_broker`이다 (`docs/CONFIGURATION.md` "broker wire 계약 점검" 참고).

### Anthropic Claude Code 셰이핑

Anthropic 전송 계층(`anthropic.py`)에 Claude Code 요청 형태를 격리하여 구현했다.

- 3개의 고유 헤더와 첫 시스템 프롬프트 문장으로 아이덴티티를 구성한다.
- 기본 버전(`DEFAULT_CLAUDE_CODE_VERSION`)은 `2.1.280`이며, `OPENSIDE_CLAUDE_CODE_VERSION` 또는 `credentials.anthropic.claude_code_version`으로 재정의할 수 있다.
- 버전 만료(version-too-old) 응답 수신 시 요구된 상위 버전을 적용하여 요청당 1회에 한해 재시도한다.
- 구독 약관 및 프로바이더 정책은 변경될 수 있으며, 실제 서비스 호출 동작은 검증되지 않았다(`unverified`).

### 기타 인증 소스

- **환경변수 (`env`)**: 기본 변수로 `ANTHROPIC_API_KEY`, `OPENAI_API_KEY`, `OPENROUTER_API_KEY`, `GOOGLE_API_KEY` / `GEMINI_API_KEY`를 참조한다. `credentials.env.<provider>`로 변수 이름을 변경할 수 있다.
- **외부 명령어 (`command`)**: `credentials.command.<provider>`에 셸 없이 실행할 argv 배열을 지정한다. 기본 타임아웃은 10초(`credentials.command.timeout_seconds`)이며 최대 상한은 60초이다. 표준 출력(stdout)은 16 KiB로 제한되고 키는 메모리에서만 쓰며 표준 오류(stderr)는 버리고 결과는 로그에 남기지 않는다. 전용 Unix 프로세스 그룹에서 실행되며 출력 초과, 타임아웃, 취소 시 그룹 전체를 즉시 강제 종료하고 정리한다.
- **codex-cli**: Pydantic AI의 `CODEX_HOME/auth.json` 또는 `~/.codex/auth.json` 위임 처리를 사용한다.
- **내장 소스 (`builtin`)**: `credentials.order`에 명시된 경우 다른 프로바이더의 Pydantic AI 기본 동작에 위임한다.
- **GitHub Copilot**: `github-copilot` 네임스페이스를 통해 동작한다.

## 설정 예시

```json
{
  "order": {
    "anthropic": ["env", "command"]
  },
  "env": {
    "anthropic": "ANTHROPIC_API_KEY"
  },
  "command": {
    "anthropic": ["pass", "show", "services/anthropic"]
  }
}
```

## 핵심 파이썬 API

모델 식별자는 고정 ID 없이 사용자가 명시한 `provider:model` 형식을 따르며, 대체 모델(fallback) 목록 역시 사용자가 직접 설정한다.

```python
async def build_model(
    model_id: str,
    fallback: list[str],
    credentials: Mapping[str, Any],
    *,
    session_id: str,
    pins: CredentialPins | None = None,
) -> Model: ...


def doctor_report(
    credentials: Mapping[str, Any],
    models: Mapping[str, Any],
) -> dict: ...
```

- 세션이 유지되는 동안 Model 및 Agent 컨텍스트를 유지해야 하며, 세션이 종료될 때 이를 닫아야(close) 한다. 브로커 클라이언트는 모델 인스턴스가 소유한다.
- `doctor_report`의 최상위 보고서 키는 `credentials`와 `models`이며, 각 행은 `provider`, `source`, `available`(bool), `reason`, `next_step` 필드를 포함한다.
- `command` 소스를 진단할 때는 바이너리 파일의 실행 가능 여부만 확인할 뿐 명령어를 실행하지 않으므로 실제 실행 결과는 검증되지 않는다(`unverified`).
- `builtin` 소스를 진단할 때는 관련 환경변수나 파일의 단순 존재 여부만 탐지할 뿐 설정의 유효성은 검증되지 않는다(`unverified`).
- doctor 실행 중 브로커 상태 점검은 읽기 전용 `GET` 요청으로 스냅샷만 조회하며 모델 요청을 보내지 않는다.

## 테스트 및 검증 명령어

`agent` 디렉터리에서 실행한다.

```bash
uv run ruff check src/openside_agent/providers tests/providers
uv run ruff format --check src/openside_agent/providers tests/providers
uv run pyright src/openside_agent/providers
uv run pytest tests/providers -m 'not live' -q
```

결정론적 단위 테스트는 임시 `HOME`, `XDG`, `CODEX_HOME`을 설정하고 `OPENSIDE_*` 및 프로바이더 API 키를 비운 격리(hermetic) 환경에서 로컬 가상 브로커와 `MockTransport`만을 대상으로 실행된다.

실제 프로바이더를 호출하는 라이브 테스트(`@pytest.mark.live`)는 별도로 분리되어 있으며 `OPENSIDE_LIVE_MODEL` 환경변수를 통해 명시적으로 참여(opt-in)하지 않는 한 건너뛴다. 라이브 테스트 실행 시에만 격리 환경의 예외로 실제 `HOME` 및 환경변수를 참조하며, 자격 증명 JSON은 `OPENSIDE_LIVE_CREDENTIALS`로 전달된다.
