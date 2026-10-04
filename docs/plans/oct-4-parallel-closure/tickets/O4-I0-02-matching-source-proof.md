# O4-I0-02 — 최종 source의 Contract·SDK·CI 검증

| 항목 | 값 |
| --- | --- |
| 에픽 / 담당 | [I0 — 단일 통합 담당·source 검증·release 게이트](../epics/I0-integration-and-release-gates.md) / 단일 통합 담당 |
| 우선순위 / 종류 | P0 / `PROOF_AND_BUILD` |
| 기준 웨이브 | [W3 — 소스 통합·검증·admission ISSUE](../waves/W3-source-validation-and-admission.md) |
| 실행 상태 | source904 workspace·daemon213·process/L3 56 `VERIFIED`; 후속 Clippy 실제 source 실패 수리 중. formal Contract/SDK 첫 명령 admission timeout `FAILED`, proof `NOT_RUN`; hosted CI·release/scale `NOT_RUN` |
| 선행 결과 | [O4-I0-01](O4-I0-01-ownership-and-contract-freeze.md) |

[전체 지도](../README.md) · [티켓 인덱스](INDEX.md)

## 목적

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
- maintenance production Clippy는 poison을 `into_inner`로 조용히 회복하고 worker/owner loss를 panic으로 처리하는 경로를 드러냈다. worker failure를 typed readiness와 supervisor terminal outcome에 전달하는 구조 수리를 진행 중이다. serving/shutdown 실패와 정상 cooperative stop/join을 재검증한 뒤 포함 source를 freeze한다.
- Semantic/Hybrid/HybridSeed/History/RuntimeMetadata의 새 실제-daemon Active fixture는 G41/G42 positive 결과와 stale G41 token refusal를 모두 요구한다. registry는 SDK27로 갱신했다. `nextest list -p quanta-index-retrieval-bench --test sdk_roundtrip --all-features --locked --message-format json`은 actual test-count27을 수집했고 `proof_inventory.py --verify /private/tmp/qi-oct4-sdk27-inventory-20261004-v1.json --role sdk`도 exit0으로 source authority equality를 확인했다. 새 live scenario 및 final-source formal proof는 아직 `NOT_RUN`이다.

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
