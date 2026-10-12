# S-12. `browser_repl` JS 엔진 선택

작성: 2026-10-11. 결정: **rquickjs (QuickJS-ng)**, ADR-021. 구현: `crates/openside-repl`.

## 조건
- 외부 Node/Bun 런타임 의존 금지(이식성). 엔진은 openside 바이너리에 정적으로 들어가야 한다.
- 비동기 호스트 함수: Playwright 문법은 모든 동작이 `await` 이고, 그 뒤에는 CDP 왕복(Rust future)이 있다. top-level `await` 가 필요하다.
- 샌드박스: `require`·`child_process`·파일·네트워크 API 가 **구성상** 없어야 한다(지우는 방식이 아니라 처음부터 없는 방식).
- 시간·메모리·출력 상한.

## 측정
같은 머신에서 엔진마다 별도 crate(`tokio` current-thread 포함)를 만들고 `cargo clean` 후 `cargo build --release -j 8` 을 측정했다. 프로그램은 호스트 함수 하나를 등록하고 `(async () => { await host(); return [typeof require, typeof process, typeof fetch, …] })()` 를 실행한다. 크기는 `strip=true` 릴리스 바이너리.

아래 숫자는 스파이크 작성자가 한 번 측정해 보고한 값이다. 측정용 crate는 저장소 밖 임시 디렉터리에 만들었고 지금은 남아 있지 않다. 다시 재려면 엔진마다 빈 바이너리 crate를 만들고 위 프로그램을 넣은 뒤, `cargo clean && time cargo build --release -j 8`(빌드 시간), `strip` 뒤 `stat -c %s`(크기), `cargo tree -e normal --prefix none | sort -u | wc -l`(의존 수), `du -sh target`(`target/` 크기)를 실행한다. 기동+평가 시간은 프로그램 안에서 `Instant`로 잰다. 머신과 버전에 따라 값은 달라진다.

| 항목 | rquickjs 0.14 | boa_engine 0.22 | deno_core 0.412 (V8) |
| --- | --- | --- | --- |
| 클린 릴리스 빌드 | 17.8초 | 23.5초 | 14.1초 + 미리 빌드된 `librusty_v8.a` 186,655,126바이트 다운로드(첫 빌드) |
| 바이너리 크기(strip) | 1,912,400 B | 13,319,184 B | 50,747,032 B |
| 의존 crate 수(`cargo tree -e normal`) | 35 | 176 | 172 |
| `target/` 크기 | 105 MB | 372 MB | 809 MB |
| 기동+평가 | 11.2 ms(이 중 10 ms 는 호스트 `sleep(10)`) | 0.58 ms | 5.1 ms |
| async 호스트 함수 | `prelude::Async` 가 Rust future 를 JS Promise 로 바꿈. `eval_promise` 가 top-level `await` 와 마지막 식의 값(`{value}`)을 돌려줌 | `NativeFunction::from_async_fn` + 직접 job queue 구동, top-level await 는 모듈 평가 필요 | `#[op2(async)]` + 이벤트 루프. 성숙함 |
| 전역에 보이는 것 | `require`·`process`·`fetch`·`std`·`os` 모두 `undefined` (quickjs-libc 를 링크하지 않음) | `require`·`process`·`fetch` `undefined` | `Deno.core.ops` 가 `object` — 내부 op 표면을 지워야 함 |
| 상한 | 메모리 상한(`set_memory_limit`), 스택 상한, interrupt 핸들러(시간·취소) | `RuntimeLimits`(루프 반복·재귀·스택). 메모리 상한 없음 | V8 heap 상한, `terminate_execution` |
| 라이선스 | MIT (rquickjs), MIT (QuickJS-ng) | MIT/Unlicense | MIT (deno_core), BSD-3 (V8) |
| 빌드 도구 | C 컴파일러(`cc`, 번들 바인딩, bindgen 불필요) | 순수 Rust | 미리 빌드된 V8 정적 라이브러리(플랫폼별) |

## 판단
- **deno_core**: 가장 빠르고 성숙하지만 바이너리 +49 MB, 플랫폼별 186 MB V8 정적 라이브러리 다운로드, 그리고 `Deno.core` 표면이 기본으로 노출된다. 샌드박스를 "지워서" 만들어야 하므로 구성상 부재 조건에 맞지 않는다.
- **boa**: 순수 Rust 라 이식성은 좋지만 크기 13 MB, 메모리 상한이 없고 async 호스트 함수·top-level await 지원이 수작업이다.
- **rquickjs**: 크기 1.9 MB, 의존 35개, 메모리·시간 상한과 async 호스트 함수·top-level await 가 모두 기본 제공된다. Playwright 스니펫은 짧고 대부분 CDP 대기 시간이라 인터프리터 속도는 병목이 아니다.

## 구현에 반영한 제약
- rquickjs 0.14 는 Rust 1.87 이상을 요구한다. `openside-repl` crate 만 `rust-version = "1.87"` 을 선언한다(워크스페이스 기본값은 그대로).
- 비병렬(기본) rquickjs 런타임은 `Send` 가 아니므로 REPL 한 번 실행마다 전용 OS 스레드와 current-thread Tokio 런타임을 만든다. 데몬 작업 스레드를 JS 계산이 막지 않는다.
- 당시 결정 사항은 '브라우저 동작 시 페이지 스크립트를 주입하지 않고, prelude는 QuickJS 내에서만 실행하며, 페이지에는 `PAGE_COMMANDS`에 포함된 DOM/Accessibility/Input/Page 명령만 전달한다'였으나, 이후 ADR-022를 통해 페이지 스크립트를 허용하도록 변경되었다(`docs/specs/browser-repl.md`). 관련 구현은 `crates/openside-repl/src/driver/script.rs`에 있으며, 본 스파이크에서 결정한 엔진 구성과 제한 수치는 그대로 유지한다.
