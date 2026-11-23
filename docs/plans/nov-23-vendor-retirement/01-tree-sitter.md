# 01 · 벤치마크 Tree-sitter 레지스트리 전환

상태 **진행 중; 확인한 배포 후보는 일부 FAILED**. Rust/Python 공식 배포본 전환을 구현하고 Cargo lock의 crates.io source·checksum까지 통합했다. 이전 Rust `symbols::tests` 실행의 프로세스와 관찰 핸들이 사라져 결과는 **NOT_RUN**으로 남겼다. 재실행은 병렬 RepoMap 변경의 `search-plane` 컴파일 오류로 테스트 본문 0이다. 검사한 Go/JavaScript/TypeScript 후보는 아래 계약에서 실패해 현재 로컬 문법을 유지한다. 다른 독립 파서 대안은 아직 평가하지 않았다. 약 108만 LoC의 생성 parser C 전체 삭제는 **NOT_RUN**이다.

재실행 첫 시도 `CARGO_BUILD_JOBS=2 QUANTA_INDEX_TARGET_GC=0 ./scripts/cargow test -p quanta-index-retrieval-bench --lib symbols::tests --locked --offline`은 `quanta-index-contract/src/native_protocol.rs`의 `RepoMapNodeRef` 미사용 import로 컴파일 **FAILED**, exit 101, 테스트 본문 0이다. 이 import는 병렬 RepoMap 변경에서 이후 제거된 상태이므로 현 소스로 다시 실행한다.

같은 선택자의 두 번째 실행에서는 registry Rust/Python grammar 컴파일을 시작했으나, 병렬 RepoMap 퇴역과 공유하는 `quanta-index-search-plane`에서 미사용 import 2건, 삭제된 `activate_prepared_search_corpus_generation_v1` 호출, `ReadIdentityV2` 초기화 필드 누락으로 컴파일 **FAILED**, exit 101, 테스트 본문 0이다. 해당 파일의 동시 변경이 정리된 후 같은 선택자를 다시 실행한다.

현재 트리에서 앞서 지적된 import 제거와 activation method 복구를 확인해 동일 선택자를 다시 admission 대기열에 넣었다. 이후 내부 디스크 여유가 약 16 GiB까지 줄고 타 작업의 빌드가 진행 중인 것을 확인해 해당 대기를 직접 중단했다. admission 전 exit 130이며 새 실행의 컴파일·본문은 **NOT_RUN**이다. 용량과 공용 레인이 확보되면 동일 선택자를 재실행한다.

## 2026-11-23 실행 결과

| 문법 | 공식 배포본에서 확인한 계약 | 판정 |
| --- | --- | --- |
| Rust 0.24.2 | Python wheel의 `&raw const` fixture와 선언 census 통과. Cargo registry crate는 lock 통합 후 별도 확인 필요. | Python 평가 경로 전환; Rust 빌드 **NOT_RUN** |
| Python 0.25.0 | 개별 공식 wheel에서 template string `t"..."` 유효·미완성 문자열 거절, 선언 census 통과. language-pack 0.10.0은 이 구문을 파싱하지 못함. | Python 평가 경로 전환; Rust 빌드 **NOT_RUN** |
| Go 0.25.0 | 개별 wheel 0.25.0과 language-pack 0.10.0·1.25.2에서 `new(expression)`, shadowed `new/make` 호출에 parse error. | 검사 후보 **FAILED**; 기존 패치 유지 |
| JavaScript 0.25.0 | 개별 wheel 0.25.0은 `export { local as const }`를 거절. language-pack 0.10.0·1.25.2는 별칭을 수용하지만 `function const() {}`도 수용해 malformed 거절 계약을 위반. | 검사 후보 **FAILED**; 기존 패치 유지 |
| TypeScript/TSX 0.23.2 | 개별 wheel 0.23.2와 language-pack 0.10.0·1.25.2에서 `export type *`/`export type * as Ns`에 parse error. 지원 변경은 업스트림 PR 상태. | 검사 후보 **FAILED**; 기존 패치 유지 |

Rust/Python 준비 파일: `Cargo.toml`과 `Cargo.lock`의 workspace dependency path를 제거했고 `./scripts/cargow metadata --locked --offline --format-version 1`은 통과했다(루트 통합 결과). `benchmarks/retrieval/{Cargo.toml,build.rs,src/symbols.rs}`, `tools/benchmark/retrieval/{declaration_parsers.py,symbol_coverage.py,source_oracle.py,retrieval_contract.py}`, `tools/benchmark/corpus_binding.py`, `pyproject.toml`, `uv.lock`, 관련 Python 테스트를 수정했다. 새 Rust 정책은 lockfile의 crates.io package version·checksum을 사용하고 Python 정책은 설치된 wheel의 extension binary SHA-256 및 project/lock pin을 사용한다. 둘은 다른 배포 채널이므로 하나의 동일 바이너리 identity로 합치지 않는다.

초기 language-pack Rust/Python/JavaScript 전환 시 실행한 세 Python 테스트 파일은 **169 passed, 3 failed 후 중단**했다. 실패는 Python template string, JavaScript malformed binding, 새 테스트 NameError였다. 위 호환성 실패에 따라 Python은 개별 wheel로 바꾸고 JavaScript는 기존 문법을 유지했으며 테스트 NameError도 수정했다. 후속 검증: 선언 census와 source oracle 두 파일 **102 passed**; parser fixture·runtime/source focused **13 passed**. `.venv/bin/python -m pytest -q tools/ci/tests/test_corpus_binding.py`는 **VERIFIED 71 passed**, 1 warning, exit 0이다. symbol/source-closure 검사 첫 묶음은 **25 passed, 1 failed** (`test_symbol_total_timeout_binds_requested_preflight_and_replay`), 실패 원인은 확인하지 못했다. 같은 테스트 단독 재현과 source-closure 묶음은 **78 passed**였지만 최초 실패를 전체 통과로 재분류하지 않는다. Rust/Cargo 수용 검증은 현재 **NOT_RUN**이며 이전 169 passed가 전체 통과 근거는 아니다.

추가 재실행 `.venv/bin/python -m pytest -q tools/ci/tests/test_retrieval_benchmark.py -k symbol`은 **VERIFIED 44 passed**, 550 deselected, exit 0이다. 앞선 간헐 실패의 원인은 여전히 미확정이다.

추가 로더 변경: `declaration_parsers.py`에서 개별 wheel의 설치된 `_binding`을 import하기 전에 설치 distribution의 RECORD에 적힌 크기·SHA-256을 확인하고, 로드된 extension 경로가 확인한 경로와 같은지 검사한다. 테스트는 실제 설치 파일을 수정하지 않고 가짜 읽기 결과로 변조를 주입한다. 이후 `.venv/bin/python -m pytest -q tools/ci/tests/test_declaration_census.py tools/ci/tests/test_source_oracle_suite.py` **VERIFIED 102 passed**, `test_corpus_binding.py -k 'gold_producer or gold_runtime'` **VERIFIED 7 passed**, `test_benchmark_source_closure.py` **VERIFIED 77 passed**, `test_retrieval_benchmark.py -k symbol` 재실행은 **VERIFIED 44 passed**다. 이전의 corpus 전체 71개 실행은 이 로더 변경 이전 결과다. 설치 RECORD도 로컬 파일이므로 이것만으로 wheel 원본 체크섬과 실제 확장 바이너리의 공급망 결속을 증명하지 않는다.

`uv lock --check --offline`은 **VERIFIED**, 94 packages, exit 0이다. 이 검사는 dependency lock freshness이며 실제 설치 wheel의 출처 증명이 아니다.

2026-11-23 추가 검증: `.venv/bin/python -m pytest -q tools/ci/tests/test_declaration_census.py tools/ci/tests/test_source_oracle_suite.py tools/ci/tests/test_corpus_binding.py tools/ci/tests/test_benchmark_source_closure.py tools/ci/tests/test_retrieval_benchmark.py -k 'not live'`는 **VERIFIED 838 passed, 6 deselected, 1 warning**, exit 0이다. `-k`로 제외된 6개와 Rust benchmark 본문은 이 결과에 포함되지 않는다. `get_parser`가 실제 로드한 Rust 0.24.2와 Python 0.25.0 `_binding.abi3.so`는 설치 distribution `RECORD`의 크기·SHA-256과 일치했고 경로·바이트 digest는 각각 `036d4fbc2c633714f93663c7bb1e1cf338f79b828ef7a20456e2c9d68521e572`, `ab3ca8f4a7efab817edacf9aacf16a8c6ed9087f92125c991f58729d6025d668`, ABI 15였다. 이 digest는 해당 로컬 설치의 결과이지 모든 플랫폼 wheel의 기대값이 아니다.

`/tmp`의 독립 Rust Cargo consumer에서 `tree-sitter 0.25.10`, `tree-sitter-rust 0.24.2`, `tree-sitter-python 0.25.0`의 정상·malformed fixture 2/2가 **VERIFIED**다. 세 패키지의 crates.io checksum은 현재 Q `Cargo.lock`과 각각 일치했다. 이 프로브는 벤치마크 `symbols.rs` 자체의 컴파일·query/record 결과는 검증하지 않는다.

추가로 현재 트리의 비-vendor tracked Rust 1,706개·Python 348개, 총 2,054개 파일(각 1 MiB 이하)에서 기존 로컬 C 문법과 공식 wheel의 `declaration_census` 결과를 직접 대조했다. 이름·kind·정의 byte span 또는 parse refusal 여부가 다른 파일은 0개였다. 비교 코드는 일회성으로 `/tmp/qi-ts-vendor-comparison/compare.py`에 두었고 저장소 산출물은 생성하지 않았다. 이 Python oracle 비교는 Rust `extract_parsed_symbols`의 owner·qualified name·chunk·취소 계약을 대체하지 않는다.

남은 결정: Go/JavaScript/TypeScript 계약을 충족하는 업스트림 릴리스가 나올 때까지 세 로컬 문법을 유지하거나, 별도 공식 parser로 Rust symbol producer와 Python oracle의 동일 계약을 재구현해 독립 fixture·코퍼스 분모를 검증한다. 기존 계약을 약화하거나 parse error 파일을 묵시적으로 제외하는 방식은 수용하지 않는다.

Go 대안 선행 확인: 로컬 `go1.25.4` 표준 `go/parser.ParseFile`을 임시 프로그램에서 직접 호출해 `new(1)`, shadow된 `new`/`make`의 일반·variadic 호출과 일반 variadic call 네 입력은 parse 성공, 잘못된 함수 선언 한 입력은 parse 오류로 확인했다. 이 선택 구문 결과는 **VERIFIED**지만 전체 코퍼스·기존 의미와 동등성을 뜻하지 않는다. 기존 독립 census [`go_checker.go`](../../../tools/benchmark/retrieval/census_checkers/go_checker.go)는 선언 이름·종류·시작 offset만 반환하므로 Rust producer의 owner·전체 정의 byte/line span·chunk 경계·취소/timeout을 아직 대체하지 못한다. 이 경로를 채택하려면 공식 Go toolchain version·배포/프로세스 custody를 고정하고 그 계약을 명시적으로 구현한 뒤 기존 Go AST fixture와 코퍼스 분모를 비교해야 한다. 기존 독립 oracle을 그대로 생산자로 돌려 검증 독립성을 잃지 않도록 별도 기대값도 필요하다.

Go AST 기반 source projection 실험은 `/tmp/qi-ts-vendor-comparison/go_projection.go`와 `compare_go.py`에서 수행했고 제품 경로에 연결하지 않았다. 현재 checkout의 tracked Go 파일은 **3개**다. 위 2,054개 차등 비교는 Rust/Python만 포함하며 Go accepted corpus 수를 뜻하지 않는다. `go1.25.4`의 `go/parser`가 원본 전체를 먼저 파싱한 뒤 `ast.CallExpr`의 `new`·`make` 식별자 위치만 같은 UTF-8 byte 길이의 일반 이름으로 바꾸는 후보를 만들었다. `make([]int, 2)` 같은 built-in type 인자는 그대로 둬야 한다. 모든 `make`를 바꾸는 초기 실험은 오히려 공식 문법에 parse error를 만들었다.

tracked Go **3개(선언 22개)**와 방법·값 수신자, interface method, 내부 type, shadowed/variadic 호출, 불합리한 builtin arity, Unicode 주석·CRLF, type 인자 등을 포함한 유효 fixture **9개(선언 22개)**를 기존 로컬 문법과 공식 0.25.0으로 비교했다. Go parser가 승인한 이 12개에서는 선언 kind·local name·owner/qualified container·name/definition byte range·정의 시작/끝 line의 차이가 **0개**였다. 6개는 원본 공식 parser 그대로, 6개는 AST에서 확인한 callee offset만 길이 보존 projection한 뒤 성공했다. 적대적 malformed 입력 **4개**는 Go parser가 거절했고 두 Tree-sitter 문법도 parse error였다. 이 범위는 **VERIFIED 12/12 유효, 4/4 거절**이다. 실제 벤치의 외부 admitted Go corpus와 Rust `SymbolRecord`/chunk 결과, Go expression/type 전체 공간, 실행 중 취소·timeout·메모리 회계는 **NOT_RUN**이다. 일회성 비교기는 제품 구현이나 최종 독립 oracle이 아니다.

실제 교체 설계: 벤치 전용 Go helper를 별도 소스로 구현하여 원본 UTF-8 source·경로·SHA-256을 제한된 stdin으로 받아 `go/parser`의 syntax 승인을 먼저 수행한다. 출력은 schema/version, 원본 digest, 정렬된 `(start,end,replacement)` 편집과 helper binary identity만 갖는 제한된 JSON으로 발급한다. 호출자는 각 offset이 원본의 `new`/`make` 호출 식별자에 해당하고 길이가 그대로이며 편집이 겹치지 않고 source digest가 입력과 같은지 검증한다. 공식 문법이 원본을 수용하면 원본을 사용하고, 오류가 있는 경우에만 승인된 projection을 파싱한다. 두 번째 파싱도 오류면 typed coverage failure이며, 원본에 없는 이름·선언을 조립하지 않는다. 출력 record의 name span이 편집과 겹치면 producer defect로 거절한다. 정의 span·line·chunk는 원본 파일의 byte/line model에서 계산하고 실제 parser에 제공한 projection digest와 편집 목록 digest를 grammar identity에 결속한다.

Go helper는 `go1.25.4`/`GOTOOLCHAIN=local GOPROXY=off go build -trimpath -buildvcs=false` 및 고정 source에서 생성한 **플랫폼별 바이너리 digest**를 벤치 패키지·평가 환경에 고정해야 한다. 실행 중 `go` 명령을 동적으로 찾아 빌드하지 않는다. 전 파일 1 MiB 상한과 기존 per-file/total deadline을 child 시작·stdin 전송·응답 읽기·종료까지 적용하고, cancellation 시 process group을 종료한다. stdout 크기·JSON 필드·edit 수를 제한한다. Go syntax 거절은 `ParseFailure`, deadline은 `TimedOut`, cancellation은 `Cancelled`, 출력/메모리 한도는 `ResourceLimit`, 프로세스 시작·종료·JSON·바이너리 digest 오류는 `ProducerDefect`로 전파한다. 메모리 상한과 macOS/Linux signed/install 배포를 검증해야 한다. 독립 oracle은 기존 `go_checker.go`의 원본 Go AST에서 전체 선언 owner/범위를 별도로 발급하고 fixed gold 및 기존 vendor 결과와 대조한다. helper와 oracle에 같은 projection 구현을 공유하지 않는다. 외부 admitted Go corpus의 파일 수·parse failure·skipped file·symbol/owner/chunk 분모와 성능을 구 기준선에서 동일하게 확인한 뒤에만 Go 사본을 삭제한다.

**Go 후보 구현:** `tools/benchmark/retrieval/go_projection/main.go`는 원본을 Go parser로 검사한 후 1 MiB 입력·65,536 edit 상한의 schema JSON을 내고, `go_projection.py`는 기대 binary SHA-256과 Go version을 확인하여 사설 임시 디렉터리에 동일 바이트를 복사한 뒤 실행한다. 호출자는 원본 SHA-256, 응답 필드, `new→foo`/`make→fooo`의 동일 byte 길이·정렬·비중첩 offset을 검사한다. timeout·실행 중 cancellation은 child process group을 종료하고 typed error를 반환한다. 이 코드는 Python 평가/벤치 기본 파서에 아직 연결되지 않았다. 로컬 검증 `GOTOOLCHAIN=local GOPROXY=off go test tools/benchmark/retrieval/go_projection/main.go tools/benchmark/retrieval/go_projection/main_test.go`는 **3/3 VERIFIED**다. 이 소스로 만든 `/tmp` 후보 바이너리의 SHA-256은 `4c42272a9e621c629d3273e14f19275d6d9cbe7f5f891c182959f64c43c15c2e`이며 릴리스·다른 플랫폼 고정값은 아니다. 후보 helper/호출부를 사용한 위 tracked/fixture 차등 비교도 다시 **12/12 유효, 4/4 거절 VERIFIED**다.

공식 Go Python wheel 0.25.0을 `pyproject.toml`/`uv.lock` dev 의존성에 추가했고 기존 Rust/Python pin은 보존했다. `uv lock --check --offline`은 **VERIFIED 95 packages**다. `get_released_go_candidate_parser()`는 설치 distribution `RECORD` 크기·SHA-256과 실제 로드된 `_binding` 경로·바이트를 재확인하고 명시적 호출에만 공식 parser를 반환한다. 이 머신의 설치 extension SHA-256은 `e7be46029b8773f5559c1dd561e4982fa65ddc131427d2e43ae9fee35bf4c917`, ABI 15다. `candidate_go_declaration_census()`는 helper 승인 결과를 공식 parser로 검사하고 원본 바이트에서 선언 범위를 읽으며, 수정된 callee와 이름 span이 겹치면 거절한다. 기존 `get_parser("go")` 및 source oracle 기본 경로는 vendor를 계속 사용한다. `.venv/bin/python -m pytest -q tools/ci/tests/test_declaration_census.py tools/ci/tests/test_source_oracle_suite.py tools/ci/tests/test_go_projection.py`는 **VERIFIED 110 passed**, Ruff는 **VERIFIED**다. 설치 wheel digest는 로컬 `RECORD` 검증값이며 lock의 플랫폼별 다운로드 SHA와 실제 설치 경로를 연결한 signed/reproducible install 수용 증거는 아직 없다.

Rust `symbols.rs`에는 `extract_go_candidate_symbols(path, source, projection, grammar)`를 추가했다. 호출자는 검증된 source/projected/helper digest와 정렬된 동일 길이 edit를 넘겨야 하며, 이 함수는 원본 재구성·digest·이름 span 교집합을 검사하고 **전달받은** grammar에 Go query를 별도로 컴파일한다. `SymbolRecord`의 이름·정의 범위·라인은 원본 source에서 발급한다. vendor 기본 경로는 그대로다. 별도 `tree-sitter-go-official` registry dev alias를 추가했고 후보 fixture는 공식 grammar를 전달해 vendor 기본 기준선과 record를 비교한다. 독립 `/tmp` Cargo metadata에서 같은 이름·버전의 path/registry 두 package ID를 함께 resolve하는 것은 **VERIFIED**다. `rustfmt --check`와 `git diff --check`는 **VERIFIED**지만 Cargo/Rust 본문 테스트는 공유 빌드 락·디스크 제한으로 **NOT_RUN**이다. 특히 공식 Go Rust fixture 본문, helper JSON에서 Rust 구조체로 들어오는 호출 경로, Rust 전 파일 deadline/취소 및 chunk 결과가 남았다. Python 후보 구현만으로 Rust 기본 생산자 퇴역을 주장하지 않는다.

Rust `GO_QUERY` 원문을 공식 Go Python wheel의 tree-sitter ABI 15 `Query`에 직접 컴파일한 경량 호환성 검사는 **VERIFIED** (6 patterns, `@def`/`@name` 두 capture)다. 이는 Rust `Query::new` 실행이나 `SymbolRecord` 본문 통과를 대신하지 않는다.

**배포 gate:** 평가 환경별 Go helper 바이너리, Go 1.25.4 toolchain/source digest, wheel/Cargo registry 다운로드 digest, 설치된 extension digest 및 실제 실행·로드 경로를 하나의 lock/manifest에 묶는다. 바이너리 누락·digest/ABI 불일치·unsupported OS는 실행 전 producer defect로 실패시키고, Go syntax 거절·child timeout·취소·응답/메모리 초과는 각각 독립 실패로 검증한다. 현재 `go_projection.py`는 응답 크기 초과를 producer defect로 분류하며 메모리 상한을 강제하지 않으므로 최종 오류 계약의 `ResourceLimit` 수용은 **NOT_RUN**이다. 외부 admitted Go corpus, 독립 AST oracle, Rust `SymbolRecord`/owner/chunk 차등 비교와 지원 플랫폼 설치 검증 전에는 registry 기본 전환 및 `vendor/tree-sitter-go` 삭제를 하지 않는다.

JS/TS 대안 선행 확인: 기존 [`ts_checker.package-lock.json`](../../../tools/benchmark/retrieval/census_checkers/ts_checker.package-lock.json)의 TypeScript 5.9.3을 `/tmp`에서 `npm ci --ignore-scripts`로 설치해 compiler `createSourceFile`의 parse diagnostics를 검사했다. `export type *`, `export type * as Ns`, JS reserved export alias, generic import의 선택 입력은 diagnostics 0이고, `function const() {}`는 parse 오류였다. 이 5개 선택 입력만 **VERIFIED**다. 최초 `npm ci --prefix`는 별도 작업 디렉터리 설정 오류로 실패했고 임시 디렉터리에서 다시 실행해 exit 0이었다. 기존 [`ts_checker.mjs`](../../../tools/benchmark/retrieval/census_checkers/ts_checker.mjs)는 이름·종류·시작 offset만 내므로 Rust producer의 정의 끝·owner·chunk·취소/메모리 계약 및 독립 평가를 보존하지 않는다. parser 통과만으로 사본을 삭제하지 않는다.

## 현재 결속

- Rust 벤치마크의 Cargo 경로 의존성에 더해 build script가 vendor 디렉터리의 실제 바이트를 해시한다. 심볼 추출에는 Go·TypeScript 패치 ID에 대한 컴파일 참조가 있다. Cargo 경로만 바꾸면 기존 검증이 깨진다. [벤치 매니페스트](../../../benchmarks/retrieval/Cargo.toml) · [build script](../../../benchmarks/retrieval/build.rs) · [심볼 추출](../../../benchmarks/retrieval/src/symbols.rs)
- Python의 선언 파서는 로컬 문법 소스를 참조하고, 심볼 커버리지 정책도 vendor 소스 digest를 읽는다. Python 경로가 살아 있는 동안 문법 사본 삭제는 완료할 수 없다. [Python 파서](../../../tools/benchmark/retrieval/declaration_parsers.py) · [심볼 정책](../../../tools/benchmark/retrieval/symbol_coverage.py)

## 빠지는 기능과 대체 구현

| ID | 직접 삭제할 때 깨지는 동작 | 대체 구현과 판정 |
| --- | --- | --- |
| TS-01 | Rust의 5개 path dependency, Go·TS 패치 ID 상수, C 소스 해시를 주입하는 build script가 빌드에 필요하다. | 다섯 레지스트리 package ID·version·checksum을 실제 resolve에서 바인딩한다. TS와 TSX는 같은 패키지의 독립 language entry로 등록한다. build script와 Rust producer는 새 결속을 같이 소비하고 기존 로컬 경로·패치 상수를 제거한다. |
| TS-02 | Python의 C 컴파일·`ctypes` 로더, 외부 parser cache와 소스 digest 검사가 전부 로컬 문법을 전제로 한다. | 지원 플랫폼의 공식 grammar wheel을 lock한다. 패키지별로 명시적인 language export를 고르고 런타임 ABI를 확인한다. 설치 입력의 wheel checksum과 실제 로드한 extension 경로·바이트를 증거에 연결한다. 바이너리 혼입·패키지 누락·ABI 불일치는 해당 실행 실패로 처리한다. Java 등 기존 별도 parser의 공급 경로는 유지한다. [Python binding 자료](https://github.com/tree-sitter/py-tree-sitter/tree/v0.25.2) |
| TS-03 | **Go**의 `new(expression)`과 shadowing·variadic call, **JS**의 reserved export alias와 정상 binding 구분, **TS/TSX**의 generic import 표현식·type-star re-export·줄바꿈 generic call signature가 일반 순정 버전에서 달라질 수 있다. **Rust/Python**도 생성기·ABI 변경으로 노드 종류와 범위가 달라질 수 있다. | 문법별로 실제 릴리스에 fixture를 돌려 기능을 판정한다. 구·신 AST에서 모두 유효한 노드 형상 차이는 기존 extractor의 node/field 처리에서 고친다. 문법이 정상 소스를 `ERROR`로 만들면 그 범위를 텍스트 휴리스틱으로 새 선언으로 만들지 않는다. 업스트림 배포본이나 이미 지원하는 독립 파서가 같은 계약을 만족하는 경우에만 교체한다. [TS 업스트림 변경 제안](https://github.com/tree-sitter/tree-sitter-typescript/pull/360) |
| TS-04 | 컬렉션의 snapshot·gold·source closure가 vendored 소스 digest를 판정 근거로 사용한다. 하드코딩 경로만 제거하면 구·신 parser 결과가 같은 cohort로 섞인다. | 실제 Rust 선택과 Python artifact의 연결을 새 grammar identity로 발급한다. 서로 다른 배포 채널의 동일 upstream source를 확인할 수 없으면 채널별 identity와 독립 census 결과를 함께 기록한다. 이전 캡처 재생은 당시의 source lock과 실행물을 사용한다. [Gold 결속](../../../tools/benchmark/corpus_binding.py) · [Source oracle](../../../tools/benchmark/retrieval/source_oracle.py) |

## 구현

1. 기존 다섯 문법의 resolved package, ABI, 소비자, 고유 패치와 기존 fixture를 열거한다. 기존 Rust/Python 런타임과 호환되는 배포 버전을 각 lock에 고정한다. 새 정책 식별자는 실제 선택한 패키지와 무결성 정보에 결속한다.
2. Rust 파서는 정식 Cargo 패키지를 사용한다. Python도 출시된 호환 파서 바인딩을 사용하며 독립 평가 경계를 유지한다. 어느 쪽도 레포 안에서 패치된 C를 다시 생성하지 않는다.
3. 다섯 문법의 정상 AST에서 declaration kind·이름·owner·byte span, chunk의 byte range·순서, 심볼 정의를 구·신 두 경로에서 비교한다. 문법 오류 fixture에서는 기존 거절을 확인한다. 코퍼스에 남는 parse failure·skipped file·후보 slice와 평가 분모도 대조한다. 지원 범위를 줄이는 결정은 영향받는 언어·쿼리·벤치 분모를 명시적으로 승인한 뒤 별도 기준선으로 적용한다.
4. 위 조건을 충족한 뒤 다섯 경로와 재생성 지침을 제거한다. 기존 비교 캡처는 원래 grammar identity와 함께 이력으로 남긴다. 새 기준선은 새 grammar identity로 다시 만든다.

## 수용

- [ ] Rust retrieval 패키지가 잠금된 새 문법으로 빌드되고 선언·chunk·심볼 관련 기존 검증을 통과한다.
- [ ] Python 선언 파서·심볼 커버리지·벤치 source-closure 관련 검증이 통과한다. 기존 정책과 다른 grammar digest를 가진 입력은 동일 기준선으로 합산할 수 없다.
- [ ] 기존 코퍼스의 지원 언어와 쿼리 family에서 결과가 사라지거나 잘못된 위치를 가리키는 경우를 판정한다. 지원 문법의 실질적 회귀가 있으면 호환되는 업스트림 릴리스 선정으로 되돌아간다.
- [ ] Cargo와 Python 실행 경로, benchmark 정책에서 다섯 로컬 사본 참조가 0이다. 저장소 제품 런타임 그래프에 parser 의존성이 새로 들어오지 않는다.

기존 검증 표면은 Rust [symbols](../../../benchmarks/retrieval/src/symbols.rs)·[parser fixtures](../../../benchmarks/retrieval/tests/fixtures/l5_parser), Python [declaration census](../../../tools/ci/tests/test_declaration_census.py)·[source oracle suite](../../../tools/ci/tests/test_source_oracle_suite.py)·[corpus binding](../../../tools/ci/tests/test_corpus_binding.py)·[source closure](../../../tools/ci/tests/test_benchmark_source_closure.py)다. 구 캐시 marker 및 교체 방지 테스트의 검증 대상은 새 로드 경계로 옮긴다. fixture나 테스트 ID를 삭제할 때는 해당 계약의 대체 근거 또는 의도한 퇴역 판정을 남긴다.
