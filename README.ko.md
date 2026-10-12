<picture>
  <source media="(prefers-color-scheme: dark)" srcset="docs/assets/logo-dark.svg">
  <img src="docs/assets/logo.svg" alt="openside" width="200">
</picture>

# openside

[English](README.md)

openside는 로컬 우선으로 동작하는 에이전트 친화적인 브라우저입니다. Rust 데몬(`opensided`)이 세션, 정책 제어, 비밀 저장소, Chromium 인스턴스를 직접 관리하고 모든 도구를 자체 실행합니다. Python 사이드카(`agent/`, Pydantic AI 기반)는 모델 루프를 돌며 도구 호출을 다시 데몬으로 중계합니다. 사용자는 `openside` CLI, 웹/PWA UI, 일반 Chromium용 MV3 확장 프로그램, MCP 또는 챗봇을 통해 에이전트와 상호작용합니다. 전용 패치된 Chromium 빌드는 선택 사항입니다.

openside는 독자적인 프로젝트이며 Aside나 at-inc와 아무런 관계가 없습니다.

## 개발 현황

프리알파(Pre-alpha) 단계입니다. API, 설정 방식, 디스크 저장 형식은 예고 없이 변경될 수 있습니다.

- Linux: 지원합니다.
- macOS: 컴파일은 되나 실행 테스트를 거치지 않았습니다.
- Windows: 컴파일은 되나 런타임을 지원하지 않습니다.

자세한 내용은 [docs/PLATFORMS.md](docs/PLATFORMS.md)를 참고하세요.

## 빌드 및 실행

필수 요구 사항: [rustup](https://rustup.rs)을 통한 Rust 안정(stable) 버전(저장소 설정에 버전이 고정되어 있음), C 컴파일러, [uv](https://docs.astral.sh/uv/)(Python 3.13 자동 설치), pnpm 9를 갖춘 Node.js 22 이상, Chromium 기반 브라우저가 필요합니다. [bubblewrap](https://github.com/containers/bubblewrap)은 선택 사항이며, 설치되어 있으면 셸 도구가 샌드박스 안에서 실행됩니다.

```bash
cargo build --release -p opensided -p openside-cli
uv sync --project agent
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
target/release/openside doctor                                         # Python, uv, 브라우저, 샌드박스, 자격 증명 상태 점검
OPENSIDE_UI_DIR=web/dist target/release/opensided --listen 127.0.0.1:7777   # 데몬을 실행 상태로 유지
# 다른 터미널에서 실행:
target/release/openside "이 README를 요약해 줘"
target/release/openside pair    # http://127.0.0.1:7777 에 있는 웹 UI와 페어링
```

openside는 기본적으로 설치되어 있는 일반 Chromium이나 Chrome을 씁니다. [browser/](browser/BUILD.md)에 설명된 패치 브라우저는 선택 사항이며 별도의 Chromium 소스 코드 체크아웃이 필요합니다.

추가 안내 문서: [docs/INSTALL.md](docs/INSTALL.md), [docs/CONFIGURATION.md](docs/CONFIGURATION.md), [docs/REMOTE.md](docs/REMOTE.md)

## 보안

에이전트는 사용자를 대신해 웹 브라우징을 하고 폼을 입력하며 셸 명령을 실행할 수 있습니다. 정책 모드와 작업별 승인 절차를 통해 이를 제한할 수 있지만, 에이전트에 작업 영역, 저장된 비밀 정보, 원격 접속 권한을 부여하기 전에 반드시 [docs/specs/threat-model.md](docs/specs/threat-model.md)를 확인하시기 바랍니다. 데몬은 루프백 인터페이스에서만 수신 대기하므로 다른 기기에 노출하려면 앞단에 TLS를 구성해야 합니다([docs/REMOTE.md](docs/REMOTE.md)). 보안 취약점 제보는 [SECURITY.md](SECURITY.md)를 참고하세요.

## 관련 문서

- [docs/03-architecture.md](docs/03-architecture.md): 구성 요소 및 데이터 흐름
- [docs/specs/](docs/specs): 프로토콜, API 및 기능 사양
- [docs/reference/](docs/reference): 자동 생성된 CLI 및 HTTP API 레퍼런스
- [CONTRIBUTING.md](CONTRIBUTING.md): 기여하기 전 확인해야 할 검사 항목 (영문 문서)

## 라이선스

이 프로젝트는 [Apache License, Version 2.0](LICENSE-APACHE) 또는 [MIT license](LICENSE-MIT) 중 원하는 라이선스를 선택하여 이용할 수 있습니다. 기여자가 따로 명시하지 않는 한, Apache-2.0 라이선스에 정의된 대로 이 프로젝트에 포함되도록 의도적으로 제출한 모든 기여물 역시 추가 조건 없이 위와 동일하게 이중 라이선스가 적용됩니다.

타사 라이선스 고지 내용은 [NOTICE](NOTICE)에 있습니다. 웹 UI 디자인 시스템은 [aside-relay](https://github.com/mym0404/aside-relay) 및 [aside-remote](https://github.com/esc5221/aside-remote)(MIT)를 바탕으로 수정하여 만들었습니다. 함께 제공되는 폰트인 Wanted Sans와 D2Coding은 SIL Open Font License 1.1을 따릅니다.
