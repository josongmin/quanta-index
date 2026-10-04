# O4-I0-02 — 최종 source의 Contract·SDK·CI 검증

| 항목 | 값 |
| --- | --- |
| 에픽 / 담당 | [I0 — 단일 통합 담당·source 검증·release 게이트](../epics/I0-integration-and-release-gates.md) / 단일 통합 담당 |
| 우선순위 / 종류 | P0 / `PROOF_AND_BUILD` |
| 기준 웨이브 | [W3 — 소스 통합·검증·admission ISSUE](../waves/W3-source-validation-and-admission.md) |
| 실행 상태 | full Clippy·owner972·daemon213/process26·bounded wire fuzz 및 source107 formal Contract788/191·fresh release SDK27·receipt replay `VERIFIED`; hosted CI·release scale qualification `NOT_RUN` |
| 선행 결과 | [O4-I0-01](O4-I0-01-ownership-and-contract-freeze.md) |

[전체 지도](../README.md) · [티켓 인덱스](INDEX.md)

## 2026-10-04 source107 fresh release SDK proof 완료

- `VERIFIED`: clean `1071692b2dd4d5a77db54f79ecd0e80a1a20b2a7`에서 `just retrieval-sdk-proof-fresh /Users/songmin/Documents/code-new/qi-oct4-sdk-proof-fresh-20261004-v2` — exit0, 비어 있는 fresh target/release/all-features, SDK27 selected/run/passed,0 failed/skipped,tests19.140s. 전체 배치에서 Nextest stdio leak 표시는 없었다. 앞선 debug leak1 관측을 소급 삭제하지 않는다.
- `VERIFIED`: 같은 checkout의 `uv run --frozen --extra dev python tools/benchmark/retrieval/portable_proof.py verify --receipt /Users/songmin/Documents/code-new/qi-oct4-sdk-proof-fresh-20261004-v2/execution-context.json` — 별도 재검증 exit0. matching runner SHA는 `e6c1ff016c13a374eb52378f52af4ea0b3afcee08f1b58284653a1f3eb294f51`, searchd SHA는 `8e8439ed3b439f5874089b0a3b2d2dcacc8ce29d0be9a40d9ca3112c55de27b5`다.
- source107 Contract v3와 SDK fresh v2를 후속 admission/capture의 실제 입력으로 사용한다. Darwin/hash-dev SDK seam을 learned retrieval 품질·performance·Linux release 또는 hosted CI로 승격하지 않는다.
- exact source107의 `gh run list --commit 1071692b2dd4d5a77db54f79ecd0e80a1a20b2a7 --limit 20 --json databaseId,headSha,name,status,conclusion,url,createdAt`는 exit0/`[]`였다. 해당 hosted CI scope는 `NOT_RUN`이다.

## 목적과 증거 경계

선택한 capture/performance/release epoch에 포함할 변경과 그 영향 범위를 먼저 고정한 뒤, 해당 source의 테스트·실제 SDK seam과 matching binaries를 발행한다. 미착수한 다른 에픽 전체를 capture 선행 조건으로 만들지 않는다.

## 배경과 현재 상태

과거 frozen7ff/181 SDK25/25, d1a1b709 captures·hardening owner tests는 현재 successor source의 증거가 아니다. current registry collection은 test 실행과 별개다. SDK proof는 scale harness binaries도 증명하지 않는다.

## 2026-10-04 중앙 검증 epoch

- 포함 구현: native completed timers, Semble parent phases, name-span producer/scorer, repository admission/required-cell controller, atomic Active response/SDK binding, admitted publish timeout/restart fixtures, cooperative maintenance disk budget.
- Python 중앙 배치: E2 255 passed / E1-controller 786 passed / cache-policy 23 passed. 각 owning ticket에 실제 명령·scope를 기록했다. 같은 테스트가 배치 사이에 중복되므로 1,064 unique tests로 계산하지 않는다.
- `VERIFIED`: `just rust-hexagonal`, `just rust-wire-inventory`, `just rust-cargo-modules`. `just rust-public-api`는 7 selected-active-head response fields의 intended drift로 먼저 실패했다. 해당 14 reexport lines만 baseline에 반영한 뒤 재실행은 PASS였고 SDK public API bytes는 변하지 않았다.
- Python required identity registry는 실제 collection 772→788로 반영했다. collection은 실행/proof receipt가 아니다. Rust/SDK registry와 final-source formal proofs는 아직 `NOT_RUN`이다.
- `VERIFIED`: `just rust-fuzz-smoke` — IPC request/response, corpus ingest, LQ pipeline 네 target의 실제 60초 smoke가 모두 exit 0이었다. 전체 fuzz qualification은 아니다.
- Rust workspace lib/bin 첫 실행은 SDK positive mocks의 selected head 누락 2건, 다음 실행은 maintenance worker의 supervisor 인계 뒤 premature stop으로 harness 8건이 실패했다. 두 원인을 source/fixture에서 수리했다. E4 disk fixture의 `unused-results` compile refusal도 반환값 binding으로 수리했다.
- `FAILED`: `./scripts/cargow --lane test-fast-lane test --workspace --lib --bins --all-features --locked`는 searchd lib105 passed/1 ignored 후 harness146 passed/1 failed로 중단했다. 새 E4 lifecycle fixture에서 delete 후 fresh rebuild와 score bits가 달랐다. 뒤쪽 runtime/lib 등은 실행되지 않았다. 별도 unequal-token L2 oracle도 같은 source/path/candidate/hash를 유지하면서 BM25 score mismatch로 실패했다. stale-segment compaction 수리 후 영향을 재검증한다.
- `VERIFIED`: matching debug searchd를 명시한 `./scripts/cargow --lane test-fast-lane test --workspace --test sdk_roundtrip --all-features --locked` — actual daemon SDK26 passed/0 failed. 새 Text/Symbol one-RPC selected head와 successor activation 뒤 stale-token refusal을 포함한다. daemon profile·fresh formal Contract/SDK·hosted CI·release/scale gates는 `NOT_RUN`이다.
- 최초 stale-only seal compaction은 L2 short30 pass 뒤 long L2의 token total178/188 및 full harness score mismatch를 남겼다. pinned engine의 exact live posting statistics 수리와 lexical/history old-format refusal을 준비 중이다. 이 영향 source를 중앙 재검증한 뒤 formal proof를 발행한다.
- `VERIFIED`: `./scripts/cargow --lane test-fast-lane nextest run --workspace --lib --all-features --locked -E 'package(quanta-index-searchd-runtime)'` — admitted publish timeout/reassembly owner1 selected/1 passed, 2.662s. 최초0755 temporary state root refusal을 기존0700 helper로 수리했다. 이는 runtime library owner이며 실제 별도 daemon process/전체daemon profile/기본30s duration 증거가 아니다.
- source-derived Rust registry의 stale request-event 4 IDs를 현재 one-RPC test 2 IDs로 교체하고 SDK live Active test 1 ID를 추가했다(Rust188 / SDK26). 실제 Nextest collection equality는 아직 `NOT_RUN`이다.
- 코드 입력이 실제로 바뀐 scope만 재검증한다. 위 owner 결과를 전체 suite, final benchmark, 배포/운영 qualification으로 승격하지 않는다.

## 2026-10-04 최종 수리 source의 중앙 관측

- Source `904043302f1db8406302a5a62bcffdc0d9412267`에서 `./scripts/cargow --lane test-fast-lane test --workspace --lib --bins --test l2_file_mutation --test sealed_manifest --all-features --locked`가 exit 0이었다. lexical296, L2 31, lexical sealed37, semantic sealed23, search-plane537, SDK lib125, searchd105/1 ignored, harness147, runtime lib1을 포함한다. 전체 unique test 합계는 산출하지 않았다.
- Tantivy standalone exact-live-token regression 1개가 locked online rail에서 통과했다. 미봉인 marker의 missing/wrong/malformed/oversized/symlink refusal, 새 index/reopen, legacy empty seal refusal 및 compactor admission failure 뒤 unsealed discard/rebuild를 current workspace 결과가 포함한다.
- `VERIFIED`: `just rust-public-api`, `just rust-cargo-modules`, `just rust-hexagonal`, `just rust-wire-inventory`, `just rust-cargo-toml-hygiene`; `just rust-fuzz-smoke 10`의 네 target exit 0. 10초는 요청된 smoke budget이며 초기화 등을 포함한 모든 실제 wall이 10초라는 뜻은 아니다.
- Source-stable E2 producer 회귀는 `test_live_lexical_external.py test_sourcegraph_parity_inventory.py test_sourcegraph_translator_export.py` 169 passed/347.08s였다. 중간 producer format 변경으로 실패한 앞선 168 pass/1 fail 결과는 이 새 전체 실행으로 대체했으며 실패 기록을 지우지 않았다.
- `VERIFIED`: source904의 `just rust-profile test-daemon` — exit0, 213 passed/1 skipped, tests259.597s. G3 physical retirement fixture는 `runtime_fast_suite`에 있고 같은 process의 runtime/real UDS scope다.
- `VERIFIED`: `./scripts/cargow --lane test-daemon-lane nextest run -p quanta-index-searchd-runtime -p quanta-index-lexical --test process_readiness_owner_v1 --test l3_exact_source --all-features --locked --test-threads 4 --success-output final` — exit0, 56 passed/0 skipped, tests26.073s. process owner26(14 OS-child scenarios/12 helper), independent exact source L3 30이다. 앞선 no-run admission timeout은 보존한다.
- `qi-oct4-contract-proof-20261004-v1` 및 `qi-oct4-sdk-proof-fresh-20261004-v1`은 resource admission 300초 대기 초과로 명령이 실패했다. source closure/collection만 있고 authoritative execution-context 및 성공 receipt는 없다. 최초 Clippy admission refusal와 뒤의 실제 source 실패를 구별한다. 필요한 formal 검증은 모든 포함 source 수리 뒤 새 root에서 순차 실행한다.
- `FAILED`: 후속 실제 `just rust-clippy`는 contract의 optional selected-head field-count arithmetic/validation/ingest grouping 8건으로 exit101이었다. 같은 의미의 explicit field count/checked refusal와 고정 JSON/CBOR field-list 회귀로 수리했다. root 실제 contract lib172 및 integration54 passed다. 다음 full Clippy는 IPC timing field naming/doc/reborrow 및 lexical doc/auto-deref 8건으로 exit101이었다. downstream lint 수리와 재실행은 진행 중이며 lint allowance로 우회하지 않는다.
- 현재 별도 managed checkout `/Users/songmin/.codex/worktrees/oct4-qualified-source/quanta-index`는 clean904를 보존한다. final included source 수리·실제 gates 후 새 HEAD로 retarget하고 그 checkout에서 source-bound formal proof를 실행한다. main 문서 자동 commit 또는 old904 receipt를 새 product source 증거로 승격하지 않는다.
- `gh run list --commit 904043302f1db8406302a5a62bcffdc0d9412267 --limit 20 --json databaseId,headSha,name,status,conclusion,url,createdAt`는 `[]`였다. 조회 성공을 hosted CI PASS로 표시하지 않는다.

## 2026-10-04 Clippy 후속 수리와 영향 재검증

- 후속 workspace Clippy는 IPC timing Rust 필드/호출부, lexical 문서, SDK binding/mock fixture, benchmark record/event assertions 및 search-plane owner test에서 추가 source 오류를 드러냈다. JSON `*_ns` 출력 키와 기존 실패 predicate는 유지하고 typed fixture builder·checked mutation·명시 enum arms로 수리했다. 아직 전체 Clippy PASS를 주장하지 않는다.
- `VERIFIED`: `./scripts/cargow --lane test-fast-lane test -p quanta-index-contract -p quanta-index-sdk -p quanta-index-retrieval-bench --lib --bins --test sdk_binding_owner_v1 --all-features --locked` — exit0. contract lib172, SDK lib125/binding owner15, retrieval-bench lib124/runner15 passed. 실제 daemon integration 및 release proof를 대신하지 않는다.
- `VERIFIED`: `./scripts/cargow --lane test-fast-lane test -p quanta-index-search-plane --lib -p quanta-index-contract --test ipc_query_result_v2_contract --all-features --locked` — exit0, contract lib172/integration54 및 search-plane537 passed. head/cursor option 교차 조합의 고정 JSON/CBOR oracle와 selected-G1 retirement pre-open refusal을 포함한다. 첫 명령의 없는 `wire_strict` selector refusal는 test 실행 실패와 구분한다.
- `VERIFIED`: 현재 후속 source의 `just rust-public-api`, `just rust-cargo-modules`, `just rust-hexagonal`, `just fmt-check`. 공개 contract/SDK API와 감시 대상 module tree는 baseline과 일치했다.
- maintenance production Clippy는 poison을 `into_inner`로 조용히 회복하고 worker/owner loss를 panic으로 처리하는 경로를 드러냈다. fatal readiness 상태, original timer의 terminal channel 및 owned cancel/join guard로 구조를 수리했다. `VERIFIED`: `./scripts/cargow --lane test-daemon-lane nextest run -p quanta-index-searchd --lib --all-features --locked -E 'test(/^app::(maintenance|supervisor)::/)' --success-output final` — exit0, 13 selected/13 passed/97 filtered, tests0.174s. poison/lost owner·worker panic·callback unwind 동안 실제 walker cancel/join·missing terminal과 정상 handoff를 포함한다.
- `FAILED`: runtime/SDK/IPC 후속 Nextest 배치는 231 selected 중 runtime supervisor의 `missing_report_during_drain_is_named_failure_not_escalation`이 실패했고 10개가 실행되지 않았다. 새 shutdown disk-meter panic owner는 실제로 통과했다. 좁힌 실패 selector의 재실행은 1 passed/24 filtered였지만, 이것만으로 전체 실패를 대체하지 않는다. fixture가 startup 이전 child 종료와 drain 도중 종료를 경합시키는 것을 확인했다. 기존 startup `RequiredChildLost` oracle를 유지하고 drain stop callback 뒤에만 fixture child가 종료하도록 고정했다. 전체 영향 재검증은 진행 중이다.
- harness sampler의 오류 문맥·checked count/시간 경계와 typed refusal JSON을 수리했다. 새 resource timing bundle은 millisecond 값을 유지하며 JSON `*_ms`/IPC `*_ns` 출력 키와 기존 exact predicate는 바꾸지 않았다. 후속 full Clippy는 추가 compile/lint 오류를 계속 수리 중이며 아직 PASS가 아니다.
- Semantic/Hybrid/HybridSeed/History/RuntimeMetadata의 새 실제-daemon Active fixture는 G41/G42 positive 결과와 stale G41 token refusal를 모두 요구한다. registry는 SDK27로 갱신했다. `nextest list -p quanta-index-retrieval-bench --test sdk_roundtrip --all-features --locked --message-format json`은 actual test-count27을 수집했고 `proof_inventory.py --verify /private/tmp/qi-oct4-sdk27-inventory-20261004-v1.json --role sdk`도 exit0으로 source authority equality를 확인했다. 새 live scenario 및 final-source formal proof는 아직 `NOT_RUN`이다.

## 2026-10-04 full Clippy 수리 완료

- `VERIFIED`: `just rust-clippy` — exit0, build10.09s. 시작 HEAD `10d6379c7ecabdefcfedc3a849db326118d77312`와 open-loop cfg(test) 두 파일의 root-owned overlay를 검사했다. upstream Tantivy dependency의 기존 warning8개는 남고 workspace lint 오류는 없었다. 앞선 실패 결과를 보존하며 lint allowance를 추가하지 않았다.
- open-loop negative fixture의 대상·기대값은 유지했다. JSON `pointer` 조회는 필드 누락을 거절하고 explicit null만 null oracle를 만족한다. 마지막 Clippy 실패는 test helper의 불필요한 `Value` 소유 인수1개였고 borrowed expected value로 수리했다.
- `VERIFIED`: 같은 source의 `just fmt-check` 및 `git diff --check` — exit0. 이 결과는 behavioral/release 검증이 아니다.
- current matching debug daemon build는 exit0이었다. 영향을 받은 core/harness/SDK/IPC/runtime/lexical owner 배치는 실행 중이며, 이전 runtime231 실패를 아직 전체 PASS로 대체하지 않는다. 새 SDK27·daemon/process·formal proof 결과는 완료 후 별도로 기록한다.

## 2026-10-04 sourcebf owner 배치와 freeze

- `VERIFIED`: `./scripts/cargow --lane test-daemon-lane nextest run -p quanta-index-core -p quanta-index-searchd-harness -p quanta-index-sdk -p quanta-index-ipc -p quanta-index-searchd-runtime -p quanta-index-lexical --lib --bins --test runtime_supervisor_owner_v1 --test sdk_binding_owner_v1 --test l2_file_mutation --test l3_exact_source --test sealed_manifest --test text_authority_shards --all-features --locked --no-fail-fast` — exit0, 28 binaries, 972 selected/run/passed, 0 skipped, tests552.768s. 동일 source에 해당하는 dirty test overlay가 실행 중 commit `bf9066391ad1e6002eff77d1b16bc535a62d3299`에 반영됐고 product/test bytes는 실행 도중 바뀌지 않았다.
- runtime supervisor25와 admitted publish timeout/reassembly owner1이 모두 통과했다. 이전 runtime/SDK/IPC231 실패 및 10 NOT_RUN 범위는 이번 전체 영향 배치로 재검증했다. bounded walker, SDK binding, IPC partial-frame, harness typed refusal/accounting, L2/L3/sealed/text authority를 포함한다. 972는 이 선택의 실행 수이며 중복 module test를 제거한 unique behavior 합계가 아니다.
- text-authority의 6,194문서 one-scope delta/inode oracle267.472s와 4,098문서 boundary delta 대 independent full rebuild446.447s가 통과했다. 시간은 debug/공유 host의 실제 관측이며 release performance 또는 capacity 수치로 사용하지 않는다.
- 관리 checkout `/Users/songmin/.codex/worktrees/oct4-qualified-source/quanta-index`를 clean `bf9066391ad1e6002eff77d1b16bc535a62d3299`로 retarget했다. formal Contract/SDK 및 matching release binaries는 이 epoch에서 발행한다. main의 후속 문서 SHA를 이 source의 proof SHA로 대체하지 않는다.
- exact sourcebf의 `gh run list --commit bf9066391ad1e6002eff77d1b16bc535a62d3299 --limit 20 --json databaseId,headSha,name,status,conclusion,url,createdAt`는 exit0/`[]`였다. hosted CI는 `NOT_RUN`이다.
- matching debug daemon을 pin한 live SDK27은 실행 중이다. current daemon/process/fuzz 및 sourcebf formal proof는 아직 완료 결과가 아니다.

## 2026-10-04 SDK27 실제 실행과 fixture 수리

- SDK27 최초 실제 실행은26 pass/1 fail였다. History symbolic-rev의 필수 tag shard를 helper가 게시하지 않은 것이 원인이었다. generation-bound tag 이름을 `CommitRecord.tags`와 실제 `.tag_upsert`에 같은 SHA로 넣고 branch query·G41/G42 head/pin/row/1RPC·stale token oracle는 유지했다.
- 수정 후 matching debug daemon을 pin한 SDK27 전체 Nextest는 exit0,27 passed,0 skipped,19.169s였다. Nextest stdio leak1건은 runner record test에서 관측했고, 해당 test 단독 재실행은 ordinary PASS1/26 skipped,9.236s였다. original leak 관측과 causal uncertainty를 보존하며 formal proof의 machine result와 구분한다.
- sourcebf는 이 fixture 수정이 포함되기 전 epoch다. 관리 checkout을 최종 수정 commit으로 다시 retarget한 뒤 formal proof를 발행한다. product source가 바뀌지 않은 owner972의 scope를 무조건 무효화하지 않는다.

## 2026-10-04 formal 실행 source 고정

- 관리 checkout을 clean `f3f7c68993f383e4ac5fdca761c111fe3d0edc3b`로 retarget했다. 이 source에는 실제 History tag shard를 게시하는 SDK fixture가 포함된다. 후속 main의 ticket 문서 변경은 이 source의 proof SHA를 바꾸지 않는다.
- `VERIFIED`: fixture 수정 뒤 `just rust-clippy`는 exit0, full workspace/all-targets scope였다. `just fmt-check` 및 `git diff --check`도 exit0이다. 기존 vendor warnings를 workspace lint 성공으로 제거하거나 숨기지 않았다.
- `VERIFIED`: current `just rust-profile test-daemon` — exit0, 213 run/passed, 1 skipped, tests231.161s. 유지보수 fatal ownership 수리 뒤 실제 selected head/retirement 및 runtime lifecycle 범위를 다시 실행했다. SDK fixture commit은 이 daemon selector의 product bytes를 바꾸지 않는다. process owner 및 bounded wire fuzz는 뒤에서 순차 실행하며 완료 전에는 통과로 판정하지 않는다.
- `VERIFIED`: current `./scripts/cargow --lane test-daemon-lane nextest run -p quanta-index-searchd-runtime --test process_readiness_owner_v1 --all-features --locked --test-threads 4` — exit0,26 passed/0 skipped,tests16.869s. 별도 daemon OS-child14와 helper12의 owner 범위이며 learned semantic·Linux release·모든 scale tier restart를 판정한 결과가 아니다.
- 새 Contract/SDK fresh proof 출력은 각각 `/Users/songmin/Documents/code-new/qi-oct4-contract-proof-20261004-v2`, `/Users/songmin/Documents/code-new/qi-oct4-sdk-proof-fresh-20261004-v2`를 사용한다. 아직 실행 결과가 없으며 기존 v1 실패 root를 재사용하지 않는다.

## 2026-10-04 bounded fuzz 및 Contract collection 거절 수리

- `VERIFIED`: `just rust-fuzz-smoke 10` — exit0, `ipc_request_decode`175,274회/11s, `ipc_response_decode`209,301회/11s, `search_corpus_ingest_decode`1,301,335회/11s, `lq_parse_pipeline`12,340회/13s, crash 없음. 각 target10초 예산의 smoke이며 exhaustive decode/fuzz qualification이 아니다. LQ elapsed에는 기존 corpus 초기화가 포함된다.
- `FAILED`: clean sourcef3의 `just retrieval-contract-proof /Users/songmin/Documents/code-new/qi-oct4-contract-proof-20261004-v2`는 Rust collection equality에서 exit1로 거절됐다. 실제191 IDs(lib124/chunking25/parser27/runner15)와 required188의 차이는 새 name-inventory1 및 local-name capture2뿐이고 제거된 ID는 없다. Python/Rust test execution, receipt 및 authoritative execution-context가 발행되기 전 실패다.
- 위3개의 source fixtures를 확인했다. declaration/usage 구분, UTF-8 byte offset 및 dotted namespace terminal의 고정 oracle와 partial/foreign/wrong-byte inventory 거절을 유지한다. required registry에 그3 ID만 추가하며 기존188을 제거·완화하지 않았다.
- `VERIFIED`: current `uv run --frozen --extra dev python tools/benchmark/retrieval/proof_inventory.py --verify /Users/songmin/Documents/code-new/qi-oct4-contract-proof-20261004-v2/rust-collection.stdout --role rust` — exit0, 실제191과 수정 registry의 정확한 equality. 이는 collection 검증이며 미실행 test를 PASS로 바꾸지 않는다. registry commit을 새 source로 고정하고 fresh Contract v3 및 SDK fresh v2를 순차 실행한다. 실패 v2 root를 보존한다.
- source 고정: 관리 checkout은 clean `1071692b2dd4d5a77db54f79ecd0e80a1a20b2a7`이다. `just retrieval-contract-proof /Users/songmin/Documents/code-new/qi-oct4-contract-proof-20261004-v3`는 실제 Python788/Rust191 collection equality를 통과한 뒤 tests를 실행 중이다. 완료 receipt 전에는 formal proof가 아니다. 후속 SDK fresh v2·admission·matching captures도 이 source를 사용하며 main의 ticket 문서 SHA로 대체하지 않는다.

## 2026-10-04 source107 Contract proof 완료

- `VERIFIED`: clean source107의 `just retrieval-contract-proof /Users/songmin/Documents/code-new/qi-oct4-contract-proof-20261004-v3` — exit0, Python788 selected/executed/passed(308.04s), Rust191 selected/executed/passed(0.723s), failures0/skipped0. source-controlled required identity equality, raw JUnit/Nextest inventory/events와 source closure를 결속해 schema-v2 execution-context 및 두 receipts를 발행했다.
- `VERIFIED`: 같은 checkout의 `uv run --frozen --extra dev python tools/benchmark/retrieval/portable_proof.py verify --receipt /Users/songmin/Documents/code-new/qi-oct4-contract-proof-20261004-v3/execution-context.json` — exit0. 기존 v2 collection 실패는 보존하며 새 root의 결과로만 판정한다.
- 이어 `just retrieval-sdk-proof-fresh /Users/songmin/Documents/code-new/qi-oct4-sdk-proof-fresh-20261004-v2`를 같은 source에서 실행 중이다. isolated fresh target·SCCACHE0·release/all-features daemon 및 SDK tests/runner의 source/binary 결속이 완료되기 전 SDK formal PASS나 matching capture 가능 상태를 발행하지 않는다.
- Contract proof는 이 선택의 Python/Rust 계약 범위이며 hosted CI, 실제 모델/외부 native query, five-product qualification, Linux release 및 operations proof를 포함하지 않는다.

## 착수 입력

- I0-01의 epoch 범위: 포함할 product/driver/scorer 변경, 해당 patch-ready owner 결과·독립 oracle·mandatory surfaces, 미포함 티켓의 이유와 후속 epoch
- E1-01/02/03의 review/admission, E2-01/03/05/06의 timer/controller/phase/warmup 등 **해당 epoch에 반영하는 모든 코드 변경**. 티켓 CLOSED 여부보다 실제 반영된 source를 검사한다.
- final current source/lock/config, exact registry, canonical toolchain/resource admission
- checkout 밖 fresh Contract/SDK/source-closure/output roots

## 어떤 파일을 어떻게 수정할지

`OWNED`는 에픽 담당 통합, `SHARED`는 I0 반영, `READ`는 기존 구현 소비다. 재현된 결함이나 채택된 계약 변경이 있을 때만 product source를 수정한다. 구현 파일과 독립 검증 파일을 함께 지정한다.

| 파일 | 함수 / 경계 | 구체적인 변경 또는 검증 | 모드 |
| --- | --- | --- | --- |
| [tools/benchmark/retrieval/proof_inventory.py](../../../../tools/benchmark/retrieval/proof_inventory.py) | actual pytest/Nextest selectors and verify | actual collected identities를 source authority와 검사하고 zero/missing/skipped를 구분한다. | READ |
| [tools/benchmark/retrieval/contract_proof.py](../../../../tools/benchmark/retrieval/contract_proof.py) | canonical contract execution | 등록된 Python/Rust rail과 raw machine result를 final source에서 실행한다. | READ |
| [tools/benchmark/retrieval/sdk_proof.py](../../../../tools/benchmark/retrieval/sdk_proof.py) | real-daemon SDK proof | matching runner/searchd build·live SDK tests·context evidence를 발행한다. | READ |
| [tools/benchmark/retrieval/portable_proof.py](../../../../tools/benchmark/retrieval/portable_proof.py) | verify execution-context | independent source/commands/results/binary/raw replay를 한다. | READ |
| [Justfile](../../../../Justfile) | retrieval-contract-local / retrieval-contract-proof / retrieval-sdk-proof-fresh / rust profiles | 기존 front door를 사용하며 actual gap가 입증될 때만 recipe를 수정한다. | SHARED |
| [tools/ci/source_closure.py](../../../../tools/ci/source_closure.py) | final source closure | scope에 필요한 source/dependency roots를 bind하고 source drift를 거절한다. | READ |
| [.github/workflows/ci.yml](../../../../.github/workflows/ci.yml) | hosted current-source checks | 현재 source check/run와 결과를 조회하고 source-local proof와 분리한다. | SHARED |

## 실행 단계

1. source를 PREPARE→VALIDATE→ISSUE로 처리한다. 각 owner가 독립 fixture·변경 patch와 영향 목록을 제출하고 I0가 선택한 epoch의 코드·schema·registry·dependencies를 먼저 통합한다. 아직 조사하지 않은 optional optimization은 PLANNED로 남긴다; NOT_APPLICABLE을 합성하지 않는다.
2. focused tests와 AGENT_PLAYBOOK surface별 escalation을 통합 source에서 실행한다.
3. source/runtime/lockfile과 actual test identity inventory를 고정하고 canonical Contract·SDK proof를 새 외부 root에서 실행한다.
4. portable proof verifier로 source/binary/input/results를 독립 재생하고 실제 selected/executed/passed/failed/skipped를 확인한다.
5. 필요한 hosted CI를 exact source에서 관측하고 remote result가 없으면 NOT_RUN/BLOCKED로 기록한다.
6. matching runner/daemon binary와 producer/controller source·config identity를 E1-03의 ISSUE 단계와 E2-04·E4-05/06에 넘긴다. ISSUE 뒤 코드 수정은 새 epoch의 PREPARE로 돌아가 영향받는 gates를 재실행한다. 이미 확인된 correctness failure는 optional로 분류해 우회하지 않는다.

## 검증 계획 — NOT_RUN

아래는 실행할 명령/시나리오다. 본 문서에서 통과를 주장하지 않는다. `<...>`와 외부 root는 실행 전에 실제 값으로 확정한다. test filter는 실제 수집 ID를 확인하고 0 tests를 성공으로 표시하지 않는다.

- `just retrieval-contract-local`
- `just retrieval-contract-proof <fresh-external-contract-root>`
- `just retrieval-sdk-proof-fresh <fresh-external-sdk-root>`
- `uv run --frozen --extra dev python tools/benchmark/retrieval/portable_proof.py verify --receipt <fresh-external-sdk-root>/execution-context.json`
- public SDK/contract: just rust-public-api; wire/decode: just rust-fuzz-smoke; module: just rust-hexagonal + just rust-cargo-modules; selection/state/ingress: just rust-profile test-daemon.

## Epoch admission과 후속 작업

- 선택한 source의 SDK/Contract/registry 실패, 필요한 raw/입력/반례 누락은 해당 capture scope를 BLOCKED/FAILED로 남긴다.
- E3 selection/timeout/health 또는 E4 storage/scanner/token 변경을 epoch에 포함하면 그 owner oracle와 surface gates를 mandatory로 소비한다. 새 source가 실제 요구받는 계약을 깨뜨리는 알려진 반례는 미포함으로 숨길 수 없다.
- 기존 두 RPC 경로나 warmup1을 유지하는 baseline epoch도 그 계약·입력·출력·timing boundary를 명시해 검증할 수 있다. single-RPC/token-index/pack 도입은 baseline capture의 자동 선행 조건이 아니다.
- I0-02는 epoch별로 실행할 gate다. E1-03 source PREPARE, E1-07 scorer, E4-07 policy 등 후속 변경은 같은 티켓의 새 epoch로 재검증한다.

## 완료 조건

- exact final source/command/selector/binary의 raw results가 authoritative verifier를 통과한다.
- 필요한 CI/SDK/contract surfaces와 제외한 provider/Linux/release/scale scope를 명시한다.

## 중단·거절·재개 조건

- host admission refusal·build-lock timeout·interrupted/partial runner를 테스트 성공으로 표시하지 않는다.
- <fresh-external-...>는 실행 전 결정할 placeholder이며 기존 root로 재실행하지 않는다.
- 필요한 입력 부재는 `BLOCKED`, 미실행은 `NOT_RUN`, 실제 실행 실패는 `FAILED`로 기록한다. 조건 미성립 `NOT_APPLICABLE`에는 실제 판단 근거가 필요하다.
- 변경이 source/input/query/unit/result에 영향을 주면 [I0 source gate](O4-I0-02-matching-source-proof.md)와 영향받는 capture/report를 다시 판정한다.
- 일회성 raw/log/capture/receipt는 checkout 밖 새 root에 둔다. 기존 외부 terminal을 덮어쓰지 않는다.

## 인계 결과

- 실제 source/dirty ownership, 변경 파일과 계약, 실행한 명령/selector, 관측 결과 및 제외 범위.
- raw/model/runtime/binary/input identity는 해당 실행 계약이 요구하는 범위에서 기록한다.
- 완료 조건별 `VERIFIED`/`FAILED`/`BLOCKED`/`NOT_RUN`/`NOT_APPLICABLE`과 후속 티켓에 넘길 입력을 발행한다.
