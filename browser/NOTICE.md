# 설계 참고와 라이선스 고지

## xplorer

[daniel-farina/xplorer](https://github.com/daniel-farina/xplorer)의 revision `64b3ef1c40e899aa9b9d14657514a5a47838f250`을 포커스 중재, `WebContents` 수명 소유권, 숨은 탭 캡처의 설계 참고로 읽었다. 브리프와 `docs/07`은 xplorer를 BSD-3 (`BSD-3-Clause`) 참고 출처로 분류한다.

**xplorer 코드는 복사하거나 이 오버레이에 재배포하지 않았다.** C++ 저장소·판정 함수와 openside 프로토콜은 새로 작성했다.

## 라이선스 확인 상태

상태는 **`unverified`**다. clone한 위 revision에는 루트 `LICENSE` 파일이 없다. `focus_arbiter.{h,cc}`·`tab_ownership.{h,cc}` 헤더는 `Copyright 2026 The Xplorer Authors`와 BSD-style 라이선스를 표기한다. 그러나 [`README.md`의 License 절](https://github.com/daniel-farina/xplorer/blob/64b3ef1c40e899aa9b9d14657514a5a47838f250/README.md#license)은 Chromium의 BSD-3-Clause와 별개로 xplorer 오버레이 조건을 프로젝트 소유자가 정한다고 설명한다.

이 자료만으로 오버레이 전체의 코드 복사 허가를 확정하지 않는다. BSD-3 설계 참고 표기는 라이선스 확인 완료의 뜻이 아니다. 후속 코드 포팅은 명시적인 라이선스 조건을 확인한 뒤 판단한다. 이번 작업은 설계만 참고하므로 코드 포팅을 하지 않는다.

## 직접 읽은 경로

모든 링크는 위 revision에 고정한다.

- [`src/chrome/browser/agent_gateway/agent_gateway.cc`](https://github.com/daniel-farina/xplorer/blob/64b3ef1c40e899aa9b9d14657514a5a47838f250/src/chrome/browser/agent_gateway/agent_gateway.cc)
- [`src/chrome/browser/agent_gateway/browser_api.cc`](https://github.com/daniel-farina/xplorer/blob/64b3ef1c40e899aa9b9d14657514a5a47838f250/src/chrome/browser/agent_gateway/browser_api.cc)
- [`src/chrome/browser/agent_gateway/focus_arbiter.h`](https://github.com/daniel-farina/xplorer/blob/64b3ef1c40e899aa9b9d14657514a5a47838f250/src/chrome/browser/agent_gateway/focus_arbiter.h)·[`focus_arbiter.cc`](https://github.com/daniel-farina/xplorer/blob/64b3ef1c40e899aa9b9d14657514a5a47838f250/src/chrome/browser/agent_gateway/focus_arbiter.cc)
- [`src/chrome/browser/agent_gateway/tab_ownership.h`](https://github.com/daniel-farina/xplorer/blob/64b3ef1c40e899aa9b9d14657514a5a47838f250/src/chrome/browser/agent_gateway/tab_ownership.h)·[`tab_ownership.cc`](https://github.com/daniel-farina/xplorer/blob/64b3ef1c40e899aa9b9d14657514a5a47838f250/src/chrome/browser/agent_gateway/tab_ownership.cc)
- [`src/chrome/browser/agent_gateway/tab_screenshot.cc`](https://github.com/daniel-farina/xplorer/blob/64b3ef1c40e899aa9b9d14657514a5a47838f250/src/chrome/browser/agent_gateway/tab_screenshot.cc)
- [`src/chrome/browser/agent_gateway/BUILD.gn`](https://github.com/daniel-farina/xplorer/blob/64b3ef1c40e899aa9b9d14657514a5a47838f250/src/chrome/browser/agent_gateway/BUILD.gn)
- [`README.md`](https://github.com/daniel-farina/xplorer/blob/64b3ef1c40e899aa9b9d14657514a5a47838f250/README.md)

Chromium·ungoogled-chromium의 라이선스와 제3자 고지는 외부 베이스 소스에 그대로 남긴다. 이 문서는 해당 고지를 대체하거나 라이선스를 바꾸지 않는다.

## P1–P7 코드 출처

`agent_gateway.{h,cc}`, `openside_dialog_hook.{h,cc}`, `patches/0002`–`0005`는 Chromium 155 공개 API를 직접 읽고 새로 작성했다(clean-room). 제3자 코드를 복사하지 않았다.

P7(`0005`)은 자동화 표식 세 가지를 제거한다.

- `navigator.webdriver`: `--headless`, `--remote-debugging-pipe`, `--remote-debugging-port=0`만으로는 켜지지 않는다. 명시적인 `--enable-automation`은 그대로 켠다.
- User-Agent: headless 모드에서도 `HeadlessChrome` 토큰을 붙이지 않는다.
- 자동화 infobar: `--openside-agent-session-id`가 붙은 전용 브라우저에서는 표시하지 않는다.

지문 위조·스텔스 패치는 기본 빌드에 없다. 넣는다면 기본값이 꺼진 별도 GN 인자 뒤에만 둔다. 현재 그런 인자와 패치는 없다.

## P8–P11 provenance

`0006` changes one Chromium profile preference default. `0007` adds an ordinary
loopback daemon-UI NTP override. Native status, omnibox keyword registration, and
the gateway entry point are original openside code. `0008` adds document/origin
checks and a document-bound variant of Chromium's native autofill Mojo path;
it modifies upstream BSD-style Chromium sources and copies no third-party fork
implementation. Chromium's license and upstream notices remain in the external
build tree.
In particular, no GPL/MPL/Source-Available or unverified xplorer code is included.

