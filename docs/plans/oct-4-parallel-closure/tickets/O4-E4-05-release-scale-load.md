# O4-E4-05 — matching release scale·load·restart 실행

| 항목 | 값 |
| --- | --- |
| 에픽 / 담당 | [E4 — 인덱싱·typo 실행 비용·release 성능·scale](../epics/E4-storage-query-and-scale.md) / E4 담당 |
| 우선순위 / 종류 | P2 / `EXECUTION_AND_PROOF` |
| 기준 웨이브 | [W4 — 실제 캡처·성능·scale](../waves/W4-native-capture-performance-and-scale.md) |
| 실행 상태 | source107 small/medium scale·small open-loop `VERIFIED`, large timeout/ANN seal `FAILED`; ANN 수리 집중8·affected97 및 최종source0e6 large4096 OS restart·matching release build `VERIFIED`. 새 large lifecycle 실행 중; 다른 tier·qualified performance 미완료 |
| 선행 결과 | [O4-I0-02](O4-I0-02-matching-source-proof.md), [O4-E4-01](O4-E4-01-index-phase-profile.md) |

[전체 지도](../README.md) · [티켓 인덱스](INDEX.md)

## 목적

현재 release harness가 실제 규모·overload·lifecycle에서 동작하는 범위와 제한 거절을 구분한다.

## 배경과 현재 상태

현 scale history2, open-loop8, default16MiB이며 explicit300s/256MiB는 별도 diagnostic profile이다. old256 lifecycle은 과거 source,4096 timeout/retention exhaustion,32768 posting-cap refusal였다. SDK proof는 scale/open-loop binary를 build/execute한 증거가 아니다.

## 2026-10-04 실행 준비와 owner 검증

- sourcebf의 I0 owner 배치가 972/972 passed였다. 선택에 harness lib/bin의 scale·open-loop source fixture, history/timeout bounds, requested/effective refusal JSON, offered accounting과 existing lifecycle oracle가 포함됐다. 실제 release tier/load를 실행한 결과는 아니다.
- 각 medium256/large4096/xlarge32768 tier는 별도 fresh external root에서 실행한다. default scale timeout30s/history16MiB·2generations와 explicit300s/history256MiB는 구별한다. open-loop request timeout의 허용 상한은30s이며 scale300s 인자를 그대로 적용하지 않는다.
- 현재 `delete_reopen`은 same-process runtime 재개방이다. OS child restart, cold page cache, physical write I/O 및 4,096/32,768문서 독립 ranking gold는 이 harness artifact로 증명하지 않는다.
- 아래 source107 결과 이전의 release build/run은 `NOT_RUN`이었다. 공유 macOS host의 diagnostic 실행에서 frequency/thermal/quiet qualification을 합성하지 않는다.

## 2026-10-04 source107 matching release binaries

- `VERIFIED`: clean `/Users/songmin/.codex/worktrees/oct4-qualified-source/quanta-index`의 `./scripts/cargow --lane release-lane build -p quanta-index-searchd-harness --bin scale_matrix --bin open_loop_matrix --all-features --locked --release` — exit0,14m50s. 실제 binaries의 `--help`도 각각 exit0으로 current CLI를 확인했다. 빌드/usage만으로 tier qualification을 주장하지 않는다.
- target은 `/Users/songmin/Library/Caches/quanta-index/target/222449791cf7286e/release-lane/release`다. scale_matrix SHA `2181bd0c047976859b516d6002220189f073ac83efabf22567d5a9af6cf5d2b0`, open_loop_matrix SHA `8a10e58e2b81f406cf856c3f2e5977f4491ec340ed5236fddab146d1eca37c31`를 실제 bytes에서 계산했다.
- default 각 tier와 diagnostic override는 별도 fresh external root에서 직렬 실행한다. summary 존재·producer exit만 보지 않고 실제 passed/config/tier/phase/point accounting 및 resource refusal를 판독한다. same-process reopen와 OS-process restart를 구분한다.

## 2026-10-04 default small·medium 실제 실행

- `VERIFIED`: 위 scale binary의 `--tier small --seed 5864059738136528177 --out-dir /private/tmp/qi-scale-small-107-v1/result` 및 medium의 `/private/tmp/qi-scale-medium-107-v1/result`를 각각 새 root에서 실행해 exit0이었다. 두 summary는 source107, `detail.passed:true`, result_count10, requested timeout/history null 및 effective30s/16MiB·2generations다.
- small16files/1repo: build1027.826ms, activate44.697ms, first query8.496ms, warm p504.525ms. 여섯 lifecycle resource phases가 존재한다. medium256files/4repos: build5557.130ms, activate87.676ms, first query11.275ms, warm p507.498ms. full ingest/seal·delete seal/activate·same-process reopen을 포함한 열 resource phases가 존재하며 delete seal1161.930ms, same-process reopen218.261ms다.
- 같은 OS process의 재개방이며 실제 process restart를 증명하지 않는다. sampled RSS/allocated-root observations와 logical bytes는 true peak·physical I/O가 아니다. 큰 tier 및 open-loop 결과를 이 두 실행으로 합성하지 않는다.

- `FAILED`: default large4096은 `/private/tmp/qi-scale-large-107-v1/result/refusal.json`에서 `status:failed`, `stage:build_seal`, `ipc Read timed out after 30000 ms`이며 producer exit1이었다. source107,16source repos,8,607,106source bytes, requested overrides null/effective30s·16MiB로 결속됐다. build 완료나 이후 lifecycle 성공을 합성하지 않는다.
- `FAILED`: 별도 `/private/tmp/qi-scale-large-diag-107-v1/result`의 같은 tier/seed, `--client-timeout-ms 300000 --history-max-bytes 268435456` actual run도 exit1이다. refusal의 stage는`delete_seal`이며 `inherited index has 1 segments, the base generation sealed 2`로 semantic seal이 거절됐다. 기본30s timeout과 다른 실패이며 diagnostic을 default PASS로 합치지 않는다.
- current harness는 delta로 바꾼 첫 파일을 다음 단계에서 삭제한다. 기존 ANN append admission은 segment count만 보존된다고 가정했다. 전체 appended segment의 live rows가 삭제될 때 library가 빈 segment를 제거하는 경계를 independent300+60→delete60 fixture 및 forged UUID negative로 재현했다. 후속 수리와 회귀 결과는 아래에 기록한다.

## 2026-10-04 ANN 수리와 영향 검증

- `FAILED`: main에 먼저 추가한 independent regressions를 root가 test-integration-lane에서 실행했다. lib의 `a_dataset_that_disagrees_with_the_inherited_contract_is_refused`는 same-count forged UUID를 기존 seal이 받아들여 exit100이었다. fail-fast로 integration은 그 실행에서 `NOT_RUN`; 별도 `--test vector_index_contract -E 'test(deleting_every_row_of_an_appended_segment_reseals_the_survivors)' --test-threads 1` 실행은1.322s 뒤 inherited1/sealed2 오류로 exit100이었다. 두 원인이 수정 전에 각각 재현됐다.
- 최종 ID/payload 차이만으로 physical deletion을 판정하는 후보 수리는 보류했다. 현재 replacement는 동일 payload도 delete+append하므로 동일 내용 교체를 오거절할 수 있다. 실제 Lance 동일 내용 교체 fixture를 추가하고, surviving UUID·실제 row identity·검증된 immutable base·새 train lineage를 함께 확인하는 수리를 반영했다. 새 source의 formal proof와 release lifecycle은 별도 실행 대상이다.
- `FAILED`: 수정 전 `identical_replacement_of_an_appended_segment_can_reseal`도 별도 actual nextest에서0.337s/exit100, inherited1/sealed2 오류를 재현했다. 동일 logical payload를 새 physical row로 교체하는 경계가 실제로 도달한다.
- 새 수리 epoch `4e3e7395c23ad18fffb14f9ddc87ef7e52c9d76b`: 기반 세대를 기존 sealed open으로 검증하고 그 same manifest의 실제 row root를 재계산한다. 기존 scan에서 얻는 Lance `_rowid`는 임시 비교에만 사용하며 canonical root/manifest schema는 유지한다. physical 삭제·추가 수와 indexed/unindexed coverage가 일치하고 남은 UUID/parameter가 ordered subset일 때만 fresh train과 새 lineage를 발행한다. same-count identity 검사는 policy retrain 선택보다 먼저 수행한다. delta당 기반 rows의 추가 scan 비용을 감춘 speedup 주장은 하지 않는다.
- `VERIFIED`: `./scripts/cargow --lane test-integration-lane nextest run -p quanta-index-semantic --lib --test vector_index_contract --all-features --locked --no-fail-fast -E 'test(deleting_every_row_of_an_appended_segment_reseals_the_survivors) | test(identical_replacement_of_an_appended_segment_can_reseal) | test(multibatch_delete_and_append_retrains_after_segment_contraction) | test(ordinary_delta_refuses_a_forged_base_row_root) | test(contracted_successor_refuses_a_forged_base_row_root) | test(a_dataset_that_disagrees_with_the_inherited_contract_is_refused) | test(a_contracted_index_requires_exact_survivor_identity_and_deleted_rows) | test(a_base_that_is_not_the_policy_retrains)' --test-threads 1` actual8/8 passed,3.047s. 이어 같은 명령에서 `-E`를 제거한 affected lib/vector contract 전체97/97 passed,0skip,17.032s였다. 기존 append/ratio/floor/recall/legacy/index-loss/restart 및 build row-integrity rails도 포함된다. source107 formal Contract/SDK proof를 이 수리의 proof로 재사용하지 않는다. matching release large diagnostic 재실행은 아직 `NOT_RUN`이다.
- `VERIFIED`: `./scripts/cargow --lane test-integration-lane clippy -p quanta-index-semantic --all-targets --all-features --locked -- -D warnings` — exit0,3.20s. 앞선 두 실행의6건+3건 lint 실패를 수정한 뒤 통과했다. `#[allow]`를 추가하지 않았으며 row-root/identity 조건과 overflow 오류 문맥을 유지했다. 이 최종 lint 수정 bytes로 위97-test 명령을 재실행해97/97 passed,0skip,16.628s·exit0이었다. release/formal source proof와는 별도다.

## 2026-10-04 source107 small open-loop 실제 실행

- `VERIFIED`: source107 small open-loop `--tier small --seed 5715144129723191120 --out-dir /private/tmp/qi-open-small-107-v1/result` actual exit0 및 summary readback을 확인했다. 기본25/50/100/200QPS·10s·32workers·queue256·2s timeout에서 offered/served254/490/1041/1958, 합계3743이며 typed/unexpected/timeout/transport/invalid/drop 모두0이다. 네 구간 unsaturated이며 공유 Darwin/hash-dev 단일 진단이다.

## 2026-10-05 large OS process restart fixture 통합

- clean sourcecef `cefb28fa0f6c678d9035cf53c5b89d20581c18a7`에 `binary_large_scoped_corpus_restart_preserves_every_source_and_ranked_page`를 반영했다. 기존 large seed5864059738136528177의16×256 sources를 별도 repo/path 열거·whole source SHA로 대조하고, caller-owned state를 seal/activate/stop한 뒤 실제 binary child 두 개에서 전체4096 rows를 순회한다. 두 process_instance는 달라야 하며 source/pin/count/strict order/candidate ID/score bits는 같아야 한다.
- `top_k256`의 byte-cut 짧은 페이지를 허용하되 모든 페이지가 nonempty/새 identity여야 하고4096 independent source count 이내에 정확히 종료해야 한다. chunk line bound는 `content.lines().count()`의 fallible u32 변환이다. query terminal ResponseWritten 이벤트는5초 안에 관측해야 한다.
- `VERIFIED`: 통합 뒤 `just fmt-check` — exit0. 실제 release owner 테스트와 영향 harness/runtime Clippy는 `NOT_RUN`이다.300s request/readiness·256MiB history는 별도 diagnostic profile이며 기본30s·16MiB PASS나 semantic relevance/ANN recall·cold page cache·Linux performance를 뜻하지 않는다.
- 다음 owner 명령은 root 단일 실행의 `./scripts/cargow --lane release-lane nextest run -p quanta-index-searchd-runtime --test process_readiness_owner_v1 --all-features --locked --release -E 'test(=e2e_process_readiness::binary_large_scoped_corpus_restart_preserves_every_source_and_ranked_page)' --test-threads 1 --success-output final`이다. source107의 same-process reopen와 기존 single-source OS restart proof를 이 새 large owner의 결과로 합성하지 않는다.
- `VERIFIED`: clean sourcecef의 `just rust-test-authority` — exit0, 기존 process owner/extended suite의 source-to-suite authority를 확인했다. 위 release owner 실제 실행은 의존성 빌드 단계이며 테스트 결과는 아직 `NOT_RUN`이다.
- 후속 actual result `VERIFIED`: 위 exact owner 명령이 clean sourcecef에서 exit0으로 끝났다. release build20m30s, selected1/1 passed/66.736s, 나머지 owner26개 filtered out이다. Nextest의60초 slow 표시는 남고 실패0이다. caller-owned large4096 state를 실제 seal/activate/stop한 후 두 binary OS child에서 모든 source SHA·generation pin·strict page order·candidate ID·score bits 및 서로 다른 process instance를 검증했다.
- 이 결과는256MiB history/300s diagnostic의 기능 검증이다. default large30s/16MiB lifecycle, ANN semantic recall/relevance, cold page cache, 정식 latency 비교, 전체 process owner27 및 Linux release를 검증한 결과가 아니다. sourcecef 영향 harness/runtime Clippy는 후속 실행 중이며 아직 PASS를 주장하지 않는다.
- 후속 `FAILED`: `./scripts/cargow --lane clippy-lane clippy -p quanta-index-searchd-harness -p quanta-index-searchd-runtime --all-targets --all-features --locked -- -D warnings` — exit101. 새 fixture의 type_complexity/collapsible_if2건이었다. tuple projection의 local type alias와 같은 predicate의 let-chain으로 정리했으며 lint allowance를 추가하지 않았다.
- 최종 style bytes에서 같은 Clippy 명령 `VERIFIED`: exit0,7.12s. 기존 Tantivy vendor warning8개는 남는다. `just fmt-check`, `just rust-public-api`, `just rust-cargo-modules`, `just rust-hexagonal` 및 `git diff --check`도 exit0이다. 이 검사는 sourcecef 이후 main의 root-owned fixture overlay(SHA `1afc50f4863d4ed008b75ed5094ea9aef04009952fb1491a2fc6307911600ef9`)에 대한 결과다. assertion/query/source 조건은 유지했고 최종 clean source/owner release 재검증을 다음 실행에서 고정한다.
- 최종 clean source `0e6c7e7e9494b63fdb33f4594df059817459d3b1`에서 위 exact release owner 명령을 다시 실행해 `VERIFIED`: exit0, release compile2m49s,1/1 passed·70.352s, owner26개 filtered out. fixture SHA `1afc50f4863d4ed008b75ed5094ea9aef04009952fb1491a2fc6307911600ef9`의 실제 실행이다. hash-dev/300s/256MiB 기능 scope이며 기존 sourcecef66.736s 관측과 별도로 보존한다. 같은 clean source의 scale/open-loop matching binary 빌드를 시작했으며 tier 실행 완료 전에는 lifecycle/성능 통과를 주장하지 않는다.
- 후속 matching release build `VERIFIED`: source0e6의 `./scripts/cargow --lane release-lane build -p quanta-index-searchd-harness --bin scale_matrix --bin open_loop_matrix --all-features --locked --release` — exit0,4m26s. 새 own target `23355b3606c20af3/release-lane/release`의 binary 두 개가 build marker보다 새로웠고 `--help`/clean HEAD도 확인했다. scale SHA `f5f5b896fe56b8172329ebdea5e1c1a492d9fc05928ec6ee5e4b9fb6b8c36f9c`, open-loop SHA `5b6718ab042121c1b03137298eea6e1a7e6a542815d112b88655254088e79c28`다. source107 target/binaries는 보존했다.
- 새 large diagnostic actual root는 `/private/tmp/qi-scale-large-diag-0e6-v1/result`다. 원 seed5864059738136528177과 explicit300000ms/268435456bytes로 root 단일 실행 중이며 완료 producer exit/summary/독립 readback 전에는 통과로 판정하지 않는다.

## 착수 입력과 실제 tier 실행

- I0-02 frozen source/release runner·daemon 외 matching scale/open_loop binaries
- 실제 256/4096/32768 inputs, ScopedOracle와 independent lifecycle 기대값, serial resource host
- default와 explicit diagnostic timeout/history profile

## 어떤 파일을 어떻게 수정할지

`OWNED`는 에픽 담당 통합, `SHARED`는 I0 반영, `READ`는 기존 구현 소비다. 재현된 결함이나 채택된 계약 변경이 있을 때만 product source를 수정한다. 구현 파일과 독립 검증 파일을 함께 지정한다.

| 파일 | 함수 / 경계 | 구체적인 변경 또는 검증 | 모드 |
| --- | --- | --- | --- |
| [crates/quanta-index-searchd-harness/src/scale.rs](../../../../crates/quanta-index-searchd-harness/src/scale.rs) | ScaleRuntimeConfig / generate_scoped_corpus / ScopedOracle / lifecycle 실행 | 현 fixed source oracle/phase resource validation을 사용하고 actual OS restart proof의 누락만 보강한다. | OWNED |
| [crates/quanta-index-searchd-harness/src/open_loop.rs](../../../../crates/quanta-index-searchd-harness/src/open_loop.rs) | schedule / measure_point / run / artifact | fixed arrivals, offered/success/error accounting, saturation vs correctness failure를 검증한다. | OWNED |
| [crates/quanta-index-searchd-harness/src/bin/scale_matrix.rs](../../../../crates/quanta-index-searchd-harness/src/bin/scale_matrix.rs) | current CLI/output | fresh external --out-dir와 requested/effective config를 결속하고 refuse/timeout을 pass로 바꾸지 않는다. | OWNED |
| [crates/quanta-index-searchd-harness/src/bin/open_loop_matrix.rs](../../../../crates/quanta-index-searchd-harness/src/bin/open_loop_matrix.rs) | current CLI/result gates | 동일 resource policy·default/override profile·terminal outcomes를 기록한다. | OWNED |
| [crates/quanta-index-searchd-harness/src/harness.rs](../../../../crates/quanta-index-searchd-harness/src/harness.rs) | boot_in_with_client_request_timeout | lazy daemon 시작 전에 bounded deadline과 caller-owned persisted root를 설정한다. | SHARED |
| [crates/quanta-index-searchd-runtime/tests/e2e_process_readiness.rs](../../../../crates/quanta-index-searchd-runtime/tests/e2e_process_readiness.rs) | large binary restart owner | 독립4096 source oracle·전체 페이지·별도 OS child parity를 실제 검증한다. | SHARED |
| [crates/quanta-index-searchd-runtime/tests/e2e_restart_replay_determinism.rs](../../../../crates/quanta-index-searchd-runtime/tests/e2e_restart_replay_determinism.rs) | runtime_risk_suite restart | same-process reopen와 실제 process restart의 row/source/pin parity를 분리 검증한다. | SHARED |
| [docs/plans/jun-7-search-product-quality/tickets-wave2/J7Q-03-large-corpus-scale-tiers.md](../../../../docs/plans/jun-7-search-product-quality/tickets-wave2/J7Q-03-large-corpus-scale-tiers.md) | scale verdict | current source별 success/refusal/not_run을 갱신한다. | OWNED |
| [docs/plans/jun-7-search-product-quality/tickets-wave2/J7Q-04-latency-tail-hardening.md](../../../../docs/plans/jun-7-search-product-quality/tickets-wave2/J7Q-04-latency-tail-hardening.md) | load verdict | offered/executed/failed/saturation과 tail/resource limits를 갱신한다. | OWNED |

## 실행 단계

1. owner-local harness oracle와 override refusal을 실행한 뒤 matching release harness binaries를 별도로 freeze한다.
2. 256부터 full/delta/delete/no-op/reopen을 실행하고 각 단계에서 independent source expected count/order/hash를 확인한다.
3. default4096/32768를 실행해 success/refusal/timeout capacity boundary를 측정한다.
4. explicit300s/256MiB 필요 시 별도 root/profile로 실행하며 default 통과와 합치지 않는다.
5. real OS process restart/cold-open과 open-loop fixed offered schedule을 실행하고 offered requests를 완전 회계한다.
6. sampled RSS/CPU/gap·logical vs allocated disk·transient high water와 saturation point를 보고한다.

## 검증 계획 — NOT_RUN

아래는 실행할 명령/시나리오다. 본 문서에서 통과를 주장하지 않는다. `<...>`와 외부 root는 실행 전에 실제 값으로 확정한다. test filter는 실제 수집 ID를 확인하고 0 tests를 성공으로 표시하지 않는다.

- `./scripts/cargow test -p quanta-index-searchd-harness --lib --bins --all-features --locked`
- `./scripts/cargow test -p quanta-index-searchd-runtime --test runtime_risk_suite --all-features --locked e2e_restart_replay_determinism`
- actual scale/open_loop CLI는 --help로 flags를 확인하고 output를 checkout 밖으로 지정한다.
- Negative: wrong fixture identity, missed offered requests, history override concealment, nonfinite latency, phase sample gap·wrong count/source after delete refusal.

## 완료 조건

- matching release binary의 각 tier/profile/lifecycle에 independent result와 terminal outcome이 있다.
- successful runtime capacity와 enforced resource refusal, diagnostic override·OS restart를 따로 표시한다.

## 중단·거절·재개 조건

- 실패 tier는 성공 timing으로 기록하지 않는다. sampled max를 true peak·logical delta를 physical write I/O로 표시하지 않는다.
- 필요한 입력 부재는 `BLOCKED`, 미실행은 `NOT_RUN`, 실제 실행 실패는 `FAILED`로 기록한다. 조건 미성립 `NOT_APPLICABLE`에는 실제 판단 근거가 필요하다.
- 변경이 source/input/query/unit/result에 영향을 주면 [I0 source gate](O4-I0-02-matching-source-proof.md)와 영향받는 capture/report를 다시 판정한다.
- 일회성 raw/log/capture/receipt는 checkout 밖 새 root에 둔다. 기존 외부 terminal을 덮어쓰지 않는다.

## 인계 결과

- 실제 source/dirty ownership, 변경 파일과 계약, 실행한 명령/selector, 관측 결과 및 제외 범위.
- raw/model/runtime/binary/input identity는 해당 실행 계약이 요구하는 범위에서 기록한다.
- 완료 조건별 `VERIFIED`/`FAILED`/`BLOCKED`/`NOT_RUN`/`NOT_APPLICABLE`과 후속 티켓에 넘길 입력을 발행한다.
