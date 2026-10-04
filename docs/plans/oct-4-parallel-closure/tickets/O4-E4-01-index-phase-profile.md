# O4-E4-01 — 인덱싱 lifecycle 비용·resource 원인 분해

| 항목 | 값 |
| --- | --- |
| 에픽 / 담당 | [E4 — 인덱싱·typo 실행 비용·release 성능·scale](../epics/E4-storage-query-and-scale.md) / E4 담당 |
| 우선순위 / 종류 | P1 / `EXECUTION_AND_PROOF` |
| 기준 웨이브 | [W1 — 근거·정답·producer 병렬 준비](../waves/W1-evidence-and-producers.md) |
| 실행 상태 | exact live-BM25 producer·artifact migration seam 수리 및 workspace lifecycle oracle `VERIFIED`; release phase profile·scale·seal streaming-pass 비용은 `NOT_RUN` |
| 선행 결과 | 없음. 현재 source 확인과 fixture 준비부터 시작 가능 |

[전체 지도](../README.md) · [티켓 인덱스](INDEX.md)

## 목적

full/delta/delete/no-op/reopen의 저장·검증·shard 비용과 transient resource를 분해해 필요한 최적화와 capacity 거절을 결정한다.

## 2026-10-04 계측·fixture 통합

- 기존 scale harness의 phase resource에 unique-inode `st_blocks*512` allocation과 `statvfs` free/available, 500ms interior samples를 추가했다. logical directory bytes 및 physical write I/O와 구분한다.
- high water는 관측한 표본의 최댓값이며 true peak가 아니다. interior allocation 표본이 없으면 high water를 null로 남긴다. probe 실패는 수치 없이 unavailable로 기록하고 observer wall/gap/setup/teardown을 보존한다.
- disk worker는 cooperative cancel 후 join하고 end boundary를 읽어 동시 recursive walker를 피한다. 막힌 filesystem syscall 자체는 중단하지 못한다.
- `small_source_lifecycle_matches_independent_bytes_and_fresh_rebuild`가 full→delta→no-op→delete→same-process reopen의 source hash, deleted-token negative, final path/rank/score parity를 검사한다. fresh rebuild parity는 독립 ranking gold나 OS restart 증거가 아니다.
- Rust compile에서 unused `usize` 반환값 refusal을 수리했다. 중앙 workspace lib/bin은146 harness tests 통과 후 새 lifecycle fresh-rebuild score 비교1건이 실패했다. focused 재현에서 top10 paths/order는 같고 모든 score bits가 달랐다.
- 일반 lexical seal은 tombstoned Tantivy 문서를 commit/wait만 하고, history index는 같은 BM25 N/df/length 잔여를 stale-segment compaction으로 제거한다. 일반 seal에도 stale segment만 purge하고 잔여0을 검증하는 구조 수리를 통합했다. 직접 L2 regression은 full→replace→invalid delete refusal→delete→fresh 경로와 고정 unequal-token source를 사용한다. 수리 전 retained source/path/candidate/hash는 같고 score bits가 달라 FAIL했다. source hash/no-op/reopen 검사를 완화하지 않는다.
- 첫 stale-only purge 뒤 직접 short L2는30 passed였지만, 중앙 workspace harness의 fresh parity는 다시146 passed/1 failed였다. retained long-source L2로 좁혀 재현하니 deleted0, num_docs=max_doc4, needle df2는 같고 `chunk_text` total tokens만 live178 / fresh188이었다. pinned Tantivy0.22.1 merger는 delete 시 quantized fieldnorm로 token 총수를 근사해 저장한다. 같은 source에 대한 score difference가 지속돼 기존 fixture를 완화하지 않는다.
- producing authority 수리로 pinned Tantivy merger의 문자열 freq fields에 한해 live posting term-frequency 합을 사용하도록 준비 중이다. Basic/JSON은 기존 엔진 의미를 보존한다. 추가 postings scan의 seal I/O/CPU와 dependency pin/upgrade 책임이 생긴다. 구형 근사 header를 새 exact 계약으로 읽지 않도록 lexical manifest13 및 history epoch2가 필요하다. 이 수리·old-format refusal·matching source 검증 전 formal proof는 `NOT_RUN`이다.
- release phase capture·실제 physical I/O·tier scale은 `NOT_RUN`이다.

## 배경과 현재 상태

현재 diagnostic9/protocol7/phase4, ingest preparation/coverage/source children, SDK request-local attribution과 sampled CPU/RSS가 있다. 과거 g9의 coverage+source2.346초/SDKpublish3.581초는 한 busy-host release 진단이다. compiler/model preparation와 index/publish/seal/activate 시간은 다른 경계다.

## 2026-10-04 Tantivy 원인 수리와 중앙 검증

- 기존 삭제 segment의 토큰 수가 quantized fieldnorm 역변환으로 추정되어, 같은 live source의 fresh rebuild와 `chunk_text` 총 토큰 수(178 대 188) 및 BM25 score bits가 달랐다. fixture의 기대값은 완화하지 않았다.
- pinned Tantivy 0.22.1을 `vendor/tantivy-0.22.1`에서 고정하고 삭제가 있는 pure string frequency fields의 살아 있는 posting TF를 checked-u64로 합산하도록 producer를 수리했다. Basic/JSON 추정 및 no-delete shortcut은 유지한다. 정확한 resolved metadata에는 local Tantivy가 하나이며 registry twin이 없다.
- lexical sealed format13/history epoch format2를 도입해 이전 approximate artifacts를 typed rebuild refusal로 처리한다. 새 index의 구버전 미봉인 재개 seam은 별도 감사에서 확인되어 수리 중이다.
- `VERIFIED`: `./scripts/cargow --lane test-fast-lane test --workspace --test l2_file_mutation --all-features --locked tombstone_scoring_uses_only_live_source_docs -- --nocapture` — 1 passed, 29 filtered, 1.81s. full→delta→delete의 source/hash/order와 score bits가 fresh rebuild와 일치한다.
- `VERIFIED`: MSRV-compatible equivalent helper 반영 뒤 `./scripts/cargow --lane test-fast-lane test --workspace --test l2_file_mutation --test sealed_manifest --all-features --locked` — L2 30, lexical sealed37, semantic sealed23 모두 passed. 실제 old-format12 base inheritance와 open refusal를 포함한다.
- vendor 자체 regression은 offline dev-dependency `fail` 부재로 실행되지 않았고 online locked rail을 실행 중이다. full workspace/scale oracle/최종 daemon/format-migration seam 및 seal-time streaming-pass 비용은 `NOT_RUN`이다.

## 2026-10-04 exact BM25 및 미봉인 재개 수리 검증

- 일반 seal은 tombstone이 있는 segment만 purge하고 잔여 deleted docs 0을 확인한다. vendor pure-string frequency fields는 quantized fieldnorm 역변환 대신 살아 있는 posting TF를 checked-u64로 합산한다. no-delete shortcut과 Basic/JSON 의미를 유지한다. 추가 seal-time CPU/I/O 비용은 아직 측정하지 않았다.
- lexical format13/history epoch2와 함께 미봉인 index marker `search-corpus-index-format.cbor` version1을 first meta 생성 전에 durable publish한다. 구버전 materialized index에 marker가 없거나 잘못된 경우 resume/base inheritance/seal 전에 typed rebuild refusal한다. verified sealed13 authority는 유지한다.
- `VERIFIED`: vendor `test_deleted_long_text_merge_keeps_exact_live_bm25_token_total` 1 passed/866 filtered. 현재 root resolved graph에 Tantivy 0.22.1 local path dependency 하나가 있으며 registry twin은 없다.
- `VERIFIED`: source `90404330`의 중앙 workspace lib/bin + L2/sealed 실행 exit 0; lexical296, L2 31, lexical sealed37, semantic sealed23, harness147을 포함한다. 최초 harness146/1 및 long-source178/188 failure는 수리 전 반례다.
- 새 L2 compactor admission failure fixture는 두 번째 writer admission을 거절해 manifest/identity 부재, inventory 제외, discard 후 재구축과 독립 fresh source/hash/candidate/order/score bits parity를 확인했다. 이 fixture는 pre-merge admission failure 및 같은 프로세스 adapter 재조립 범위이며 실제 mid-merge OS kill/power loss 증거는 아니다.
- daemon profile은 실행 중이다. 실제 release tier·OS restart·physical I/O/streaming-pass overhead 및 qualified performance는 `NOT_RUN`이다.

## 착수 입력

- 현 source/driver schema와 matching release binaries; fresh same corpus roots
- full/delta/delete/no-op/reopen fixture와 independent fresh rebuild oracle
- disk free/allocated/logical/transient와 sampled RSS/CPU probe scope

## 어떤 파일을 어떻게 수정할지

`OWNED`는 에픽 담당 통합, `SHARED`는 I0 반영, `READ`는 기존 구현 소비다. 재현된 결함이나 채택된 계약 변경이 있을 때만 product source를 수정한다. 구현 파일과 독립 검증 파일을 함께 지정한다.

| 파일 | 함수 / 경계 | 구체적인 변경 또는 검증 | 모드 |
| --- | --- | --- | --- |
| [crates/quanta-index-contract/src/ipc/ingest_observation.rs](../../../../crates/quanta-index-contract/src/ipc/ingest_observation.rs) | ingest stage observation | READ: 현재 child clocks를 재사용한다. 실제 빠진 phase만 I0가 contract에 통합한다. | READ |
| [crates/quanta-index-lexical/src/adapter_ingest.rs](../../../../crates/quanta-index-lexical/src/adapter_ingest.rs) | publish preparation/build observations | 기존 measured stage와 parent bounds를 사용하고 observer overhead/on-off를 보존한다. | OWNED |
| [crates/quanta-index-lexical/src/adapter_open.rs](../../../../crates/quanta-index-lexical/src/adapter_open.rs) | cold-open phase | same-process adapter reopen와 actual process restart를 다른 observation으로 기록한다. | OWNED |
| [crates/quanta-index-lexical/src/file_authority.rs](../../../../crates/quanta-index-lexical/src/file_authority.rs) | plan_ops / apply_plan / from_verified_files | source persistence·digest/admission·postings build의 실제 work를 구별한다. | OWNED |
| [benchmarks/retrieval/src/diagnostics.rs](../../../../benchmarks/retrieval/src/diagnostics.rs) | current ingest/query diagnostics | 현 schema field를 소비하며 parent bounds와 absent/invalid state를 보존한다. | SHARED |
| [tools/benchmark/retrieval/query_timing_overhead.py](../../../../tools/benchmark/retrieval/query_timing_overhead.py) | observation profile | observation on/off output equality와 request work counter를 현 paths로 대조한다. | OWNED |
| [crates/quanta-index-lexical/tests/l2_file_mutation.rs](../../../../crates/quanta-index-lexical/tests/l2_file_mutation.rs) | full/delta/delete/no-op fixture | 독립 fresh rebuild와 expected sources/windows/coverage로 parity를 증명한다. | OWNED |
| [crates/quanta-index-ipc/src/server.rs](../../../../crates/quanta-index-ipc/src/server.rs) | RequestEventScope / DispatchContextV1.record_event_v1 | request/connection-bound 기존 events를 먼저 소비한다. prevalidation/ingress residual이 측정상 계속 클 때만 E3와 함께 최소 stage를 제안하고 I0가 반영한다. | SHARED |
| [crates/quanta-index-ipc/src/server/tests.rs](../../../../crates/quanta-index-ipc/src/server/tests.rs) | request event / ingress/deadline controls | 계측이 바뀌면 request/connection 정확한 join·terminal1회·zero drops와 credentials/deadline/cancel/partial-response를 독립 fixture로 검사한다. | SHARED |

## 실행 단계

1. 현재 child/parent 시간과 work counters의 포함 관계를 static source graph에서 고정한다.
2. 작은 full/delta/delete/no-op/reopen actual run에서 rows/cursor/source authority를 fresh rebuild와 비교한다.
3. matching release에서 source/coverage/shard/digest/sync 또는 명시적 unattributed 구간을 분해한다.
4. prevalidation/ingress residual이 반복해서 큰 경우만 기존 request-local events에서 정확한 request/connection join과 zero drops를 확인한다. 필요한 최소 계측은 E3 경계 검토 후 I0가 통합하며 RPC roundtrip 차이를 IPC로 단정하지 않는다.
5. 관측 overhead·sample gap·parent probes/CPU domain을 표기하고 disk logical/physical/transient를 따로 측정한다.
6. 내부 성능 원인과 resource admission refusal을 E4-02/03/04/05의 조건으로 전달한다.

## 검증 계획 — NOT_RUN

아래는 실행할 명령/시나리오다. 본 문서에서 통과를 주장하지 않는다. `<...>`와 외부 root는 실행 전에 실제 값으로 확정한다. test filter는 실제 수집 ID를 확인하고 0 tests를 성공으로 표시하지 않는다.

- `./scripts/cargow test -p quanta-index-lexical --test l2_file_mutation --locked`
- `uv run --frozen --extra dev python -m pytest tools/ci/tests/test_retrieval_benchmark.py tools/ci/tests/test_completed_response_timing.py -q -k 'ingest or detailed_file_authority or observation'`
- Negative: child>parent, missing observer phase, process CPU scope 혼동, wrong source/delete/no-op result, sample gap을 peak으로 정상화 거절.
- IPC 계측 변경 시 ./scripts/cargow test -p quanta-index-ipc --lib --all-features --locked 및 실제 SDK request/connection join fixture; decode/wire 변경은 just rust-fuzz-smoke.

## 완료 조건

- 모든 lifecycle phase는 explicit timer/resource domain과 source-bound result parity를 갖는다.
- 어떤 비용이 어느 work에 속하는지 설명하고 실제 큰 residual이 아니면 추가 instrumentation을 중단한다.

## 중단·거절·재개 조건

- file별 fsync를 봤다는 사실만으로 주원인을 단정하지 않는다. syscall 특권이 없으면 physical I/O는 NOT_RUN이며 임의 privilege escalation으로 대체하지 않는다.
- 필요한 입력 부재는 `BLOCKED`, 미실행은 `NOT_RUN`, 실제 실행 실패는 `FAILED`로 기록한다. 조건 미성립 `NOT_APPLICABLE`에는 실제 판단 근거가 필요하다.
- 변경이 source/input/query/unit/result에 영향을 주면 [I0 source gate](O4-I0-02-matching-source-proof.md)와 영향받는 capture/report를 다시 판정한다.
- 일회성 raw/log/capture/receipt는 checkout 밖 새 root에 둔다. 기존 외부 terminal을 덮어쓰지 않는다.

## 인계 결과

- 실제 source/dirty ownership, 변경 파일과 계약, 실행한 명령/selector, 관측 결과 및 제외 범위.
- raw/model/runtime/binary/input identity는 해당 실행 계약이 요구하는 범위에서 기록한다.
- 완료 조건별 `VERIFIED`/`FAILED`/`BLOCKED`/`NOT_RUN`/`NOT_APPLICABLE`과 후속 티켓에 넘길 입력을 발행한다.
