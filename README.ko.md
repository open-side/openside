<picture>
  <source media="(prefers-color-scheme: dark)" srcset="docs/assets/logo-dark.svg">
  <img src="docs/assets/logo.svg" alt="openside" width="200">
</picture>

# openside

[English](README.md)

openside는 로컬 우선으로 동작하는 에이전트 친화적인 브라우저입니다. Rust 데몬(`opensided`)이 세션, 정책 제어, 비밀 저장소, Chromium 인스턴스를 직접 관리하고 모든 도구를 자체 실행합니다. Bun/TypeScript 사이드카(`agent/`, omp pi-agent-core 기반)는 모델 루프를 실행하고 도구 호출을 데몬으로 중계합니다. 사용자는 `openside` CLI, 웹/PWA UI, 일반 Chromium용 MV3 확장 프로그램, MCP 또는 챗봇을 통해 에이전트와 상호작용합니다. 전용 패치된 Chromium 빌드는 선택 사항입니다.

openside는 독자적인 프로젝트이며 Aside나 at-inc와 아무런 관계가 없습니다.

## 개발 현황

프리알파(Pre-alpha) 단계입니다. API, 설정 방식, 디스크 저장 형식은 예고 없이 변경될 수 있습니다.

- Linux: 지원합니다.
- macOS: 컴파일은 되나 실행 테스트를 거치지 않았습니다.
- Windows: 컴파일은 되나 런타임을 지원하지 않습니다.

자세한 내용은 [docs/PLATFORMS.md](docs/PLATFORMS.md)를 참고하세요.

## 빌드 및 실행

필수 요구 사항: [rustup](https://rustup.rs)으로 설치한 Rust stable(저장소에서 버전 고정), C 컴파일러, [Bun](https://bun.sh) 1.4.4 이상, pnpm 9를 갖춘 Node.js 22 이상, Chromium 기반 브라우저가 필요합니다. [bubblewrap](https://github.com/containers/bubblewrap)은 선택 사항이며, 설치되어 있으면 셸 도구가 샌드박스 안에서 실행됩니다.

```bash
cargo build --release -p opensided -p openside-cli
(cd agent && bun install --frozen-lockfile)
pnpm -C web install && pnpm -C web build                # 선택 사항: 웹 UI
pnpm -C extension install && pnpm -C extension build    # 선택 사항: 확장 프로그램 (extension/dist 폴더를 압축 해제된 확장 프로그램으로 로드)
```

`$XDG_CONFIG_HOME/openside/config.toml` 파일에 모델과 자격 증명 소스를 지정합니다.

```toml
[models]
default = "anthropic:<model-name>"

[credentials.order]
anthropic = ["env"]   # ANTHROPIC_API_KEY 환경 변수를 읽음
```

설정을 마친 후 다음 명령을 실행합니다.

```bash
target/release/openside doctor                                         # Bun, 사이드카 의존성, 브라우저, 샌드박스, 자격 증명 상태 점검
OPENSIDE_UI_DIR=web/dist target/release/opensided --listen 127.0.0.1:7777   # 데몬을 실행 상태로 유지
# 다른 터미널에서 실행:
target/release/openside "이 README를 요약해 줘"
target/release/openside pair    # http://127.0.0.1:7777 에 있는 웹 UI와 페어링
```

openside는 기본적으로 설치되어 있는 일반 Chromium이나 Chrome을 씁니다. [browser/](browser/BUILD.md)에 설명된 패치 브라우저는 선택 사항이며 별도의 Chromium 소스 코드 체크아웃이 필요합니다.

추가 안내 문서: [docs/INSTALL.md](docs/INSTALL.md), [docs/CONFIGURATION.md](docs/CONFIGURATION.md), [docs/REMOTE.md](docs/REMOTE.md)

## 활용 방법

- **원격 접속**: Tailscale Serve, Cloudflare Tunnel 또는 자체 릴레이를 통해 스마트폰이나 다른 컴퓨터에서 데몬에 접속할 수 있으며, `openside remote setup` 명령어로 설정합니다([docs/REMOTE.md](docs/REMOTE.md) 참고).
- **MCP 및 위임**: `openside mcp`는 브라우저와 에이전트 도구를 다른 에이전트에 제공하고, `openside exec`은 openside 브라우저 에이전트에 목표 작업을 위임합니다([docs/specs/mcp.md](docs/specs/mcp.md) 참고).
- **첫 실행 안내**: 웹 UI를 처음 열면 `/welcome` 화면이 표시되어 설치된 에이전트 감지, 브라우저 프로필 가져오기, 볼트(vault) 설정을 차례로 진행합니다([docs/specs/onboarding.md](docs/specs/onboarding.md) 참고).
- **스마트폰 앱**: 웹 UI는 공유 대상(share target), 바로가기, 대기 중인 티켓 배지, 오프라인 전송함을 지원하는 PWA로 설치해 쓸 수 있습니다.

## CLI 명령어

`openside [options] [prompt...]`는 `openside exec`과 똑같이 작업을 실행하며, 프롬프트가 http(s) URL뿐이면 해당 페이지를 새 탭에서 엽니다. 사용자가 티켓에 응답하는 동안 작업은 계속 진행되고, 실행이 마무리되면 명령어가 종료됩니다. 실행을 따라가는 도중(`exec` 또는 `session resume`) Ctrl-C를 누르면 작업은 멈추지 않고 터미널만 분리됩니다. 이때 세션을 다시 이어받거나 중지하는 방법을 출력한 뒤 종료 코드 130으로 끝나며, Ctrl-C를 한 번 더 누르면 즉시 종료됩니다. 전체 명령어는 `openside --help`로 확인할 수 있고, 자세한 레퍼런스는 [docs/reference/cli.md](docs/reference/cli.md)에 있습니다. 명령어 이름은 Aside CLI에 맞춰 바뀌었으며, 예전 이름은 별칭으로 남아 있지 않습니다. 하위 명령어 없이 일반 단어로 시작하면 곧바로 프롬프트로 인식하므로 `openside run tests`는 이제 단순한 "run tests" 프롬프트가 됩니다. 예전 방식 중 대체 항목을 안내하며 실패하는 것은 삭제된 `--remote` 플래그를 쓰거나 `OPENSIDE_HOST` 없이 `OPENSIDE_REMOTE`를 설정한 경우뿐입니다. 이전 명령어와 새 명령어의 대응 관계는 아래 표와 같습니다.

| 이전 | 현재 |
|---|---|
| `openside run GOAL`, `openside chat GOAL` | `openside exec PROMPT` 또는 `openside PROMPT` (항상 새 세션으로 시작) |
| `run --session ID GOAL --wait` | `openside session resume ID PROMPT` |
| `steer`, `cancel`, `tasks` | `session steer\|queue`, `session stop [--task ID]`, `session tasks` |
| `connect` | `login` (및 `logout`) |
| `remote profiles list\|rm` | `host list`, `host use NAME`, `host status`, `logout --host NAME` |
| `--remote NAME`, `OPENSIDE_REMOTE` | `--host NAME`, `OPENSIDE_HOST` |
| `--mode read-only\|guard\|full` (새 세션) | `--permission read-only\|ask\|guard\|full-access` |
| `--browser-space isolated` | `--profile isolated` (또는 `--profile incognito`) |
| `accounts …` | `models accounts …` |
| `repl ID --eval CODE`, `repl ID --interactive` | `repl --session ID CODE`; 코드가 없으면 표준 입력에서 한 줄씩 스니펫으로 실행 |
| 쿼리 없는 `memory search` | `memory list` |
| MCP `agent_run {goal, mode, browser_space}` | MCP `exec {prompt, permission, profile_id}` |
| MCP `browser_repl` | MCP `repl` |

`routine add --mode`와 `policy test --mode`는 기존 이름과 값을 그대로 유지합니다. 작업 옵션(`-m`, `-p`, `-s`, `--effort`, `--permission`, `--profile`, `--account`, `--workspace`)은 일반 프롬프트 앞이나 `exec` 뒤에 와야 하며, 다른 명령어 앞에 두면 사용법 오류가 발생합니다.

## 보안

에이전트는 사용자를 대신해 웹 브라우징을 하고 폼을 입력하며 셸 명령을 실행할 수 있습니다. 정책 모드와 작업별 승인 절차를 통해 이를 제한할 수 있지만, 에이전트에 작업 영역, 저장된 비밀 정보, 원격 접속 권한을 부여하기 전에 반드시 [docs/specs/threat-model.md](docs/specs/threat-model.md)를 확인하시기 바랍니다. 데몬은 루프백 인터페이스에서만 수신 대기하므로 다른 기기에 노출하려면 앞단에 TLS를 구성해야 합니다([docs/REMOTE.md](docs/REMOTE.md)). 보안 취약점 제보는 [SECURITY.md](SECURITY.md)를 참고하세요.

릴레이를 거치는 CLI와 MCP 연결은 Noise로 종단간 암호화되며, 페어링 링크에서 받은 데몬 키를 고정합니다. 릴레이에서 불러온 PWA는 수동적인 감청으로부터 트래픽을 보호하지만 코드를 배포하는 릴레이 운영자를 신뢰해야 하므로, 변조된 스크립트가 페어링 코드와 토큰, 평문을 가로챌 수 있습니다. 따라서 자체 호스팅 릴레이를 쓰거나, 릴레이 대신 본인의 데몬이나 Tailscale 오리진에서 PWA를 직접 불러와 연결하세요. 로그인 값은 에이전트의 도구 실행 결과에서 제외되지만, 에이전트가 실행하는 페이지 스크립트에까지 숨겨지지는 않습니다. 확인 요청 승인, 선택지 응답, 인증기 등록, 로그인 저장에는 페어링된 사람 기기가 필요합니다. 에이전트 클라이언트와 로컬 소켓은 이러한 작업을 거절하거나 취소할 수는 있어도 승인할 수는 없습니다. 데몬 호스트에서 `openside pair --agent`로 에이전트 페어링 링크를 만든 뒤 `openside login`으로 등록하세요. 일반 스마트폰이나 브라우저용 페어링 링크는 CLI에서 등록하더라도 사람 기기의 승인 권한을 그대로 유지하므로, 해당 토큰을 에이전트에 넘기지 마세요.

## 관련 문서

- [docs/03-architecture.md](docs/03-architecture.md): 구성 요소 및 데이터 흐름
- [docs/specs/](docs/specs): 프로토콜, API 및 기능 사양
- [docs/reference/](docs/reference): 자동 생성된 CLI 및 HTTP API 레퍼런스
- [CONTRIBUTING.md](CONTRIBUTING.md): 기여하기 전 확인해야 할 검사 항목 (영문 문서)

## 라이선스

이 프로젝트는 [Apache License, Version 2.0](LICENSE-APACHE) 또는 [MIT license](LICENSE-MIT) 중 원하는 라이선스를 선택하여 이용할 수 있습니다. 기여자가 따로 명시하지 않는 한, Apache-2.0 라이선스에 정의된 대로 이 프로젝트에 포함되도록 의도적으로 제출한 모든 기여물 역시 추가 조건 없이 위와 동일하게 이중 라이선스가 적용됩니다.

타사 라이선스 고지 내용은 [NOTICE](NOTICE)에 있습니다. 웹 UI 디자인 시스템은 [aside-relay](https://github.com/mym0404/aside-relay) 및 [aside-remote](https://github.com/esc5221/aside-remote)(MIT)를 바탕으로 수정하여 만들었습니다. 함께 제공되는 폰트인 Wanted Sans와 D2Coding은 SIL Open Font License 1.1을 따릅니다.
