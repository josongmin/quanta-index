# 근거와 적용 한계

확인일: 2026-10-08. 외부 자료는 해당 프로젝트·제작자의 1차 자료다.
자료의 패턴을 설계 근거로 사용하며, 우리 구현의 정확성·실행 결과를 대신하지 않는다.
최신 문서의 기능과 Cargo.lock에 고정된 릴리스의 지원 여부는 별도로 확인한다.

## 현재 소스

기준과 재대조 범위는 [README](README.md#기준-소스와-중복-방지)에 명시했다.
아래 경로는 현재 checkout을 가리키며 이후 변경 시 구현 전에 다시 확인한다.

| 근거 | 확인할 함수·타입과 의미 |
| --- | --- |
| [공개 모델 계약](../../../crates/quanta-index-contract/src/ipc/ingest/semantic_wire.rs) / [generation 계약](../../../crates/quanta-index-semantic/src/generation_contract.rs) | `EmbeddingModelContract`의 policy/view policy 필드가 `GenerationContract`에 보존되지 않음 |
| [Semantic build](../../../crates/quanta-index-semantic/src/build.rs) | `ensure_generation_contract`, `prepare_staging_dataset`, 첫 `next_window()`의 순서; base 호환성 사전 검사 위치 |
| [같은 build owner](../../../crates/quanta-index-semantic/src/build.rs) | `inherit_dataset_tree()`의 전체 directory 순회/hardlink; `seal_manifest_bytes()`의 row·membership 집계 |
| [Query 모델 검사](../../../crates/quanta-index-search-plane/src/query_dispatcher/semantic_query.rs) | `ensure_query_model_matches_index_v1`; 모델 ID/revision 검사는 이미 있음 |
| [Semantic row 무결성](../../../crates/quanta-index-semantic/src/semantic_row_integrity_v1.rs) | 전체 row 수집·고유성·정렬·commitment와 native row fingerprint 관계 |
| [Publication proof](../../../crates/quanta-index-lexical/src/file_authority/publication_proof.rs) | `verify_for_publication`은 identity/root/policy 일치 시 현재 객체 hash를 확인하며 기존 증거 재사용 |
| [Canonical verifier](../../../crates/quanta-index-lexical/src/file_authority/verify.rs) / [output](../../../crates/quanta-index-lexical/src/file_authority/verify/output.rs) | source bucket별 independent membership proof; serving/publication output 분리 |
| [SearchCorpus](../../../crates/quanta-index-search-plane/src/ingest_dispatcher/search_corpus.rs) | operation lock, base preflight, stream 생성과 event reserve의 순서 |
| [Activation catalog](../../../crates/quanta-index-search-plane/src/readiness/activation_catalog.rs) | head CAS와 source event Active 전환의 repository persist 경계 |
| [Repository envelope](../../../crates/quanta-index-search-plane/src/readiness/activation_catalog/repository_envelope.rs) | events/streams/roots/encoded envelope의 서로 다른 한도 |
| [RepoMap store](../../../crates/quanta-index-repomap/src/store.rs) / [pin](../../../crates/quanta-index-repomap/src/pinned.rs) | `acquire_pinned`의 보호 구간 종료 후 pin 증가; `gc_retired_objects`의 pin 확인과 unlink 분리 |
| [RepoMap 테스트](../../../crates/quanta-index-repomap/tests/read_view_lifetime_owner_v1.rs) | `panic_and_early_release_return_the_pin_and_gc_proceeds`; 이미 교체된 generation의 acquire panic도 잡을 수 있음 |
| [Request budget](../../../crates/quanta-index-core/src/request_budget.rs) / [dispatch](../../../crates/quanta-index-search-plane/src/ingest_dispatcher/dispatcher.rs) / [provider](../../../crates/quanta-index-embed/src/openai.rs) | cooperative budget, ingest 입장 이후 durable 종료 정책, bounded/unbounded embedding 경로 |
| [배치 admission](../../../crates/quanta-index-core/src/ingest_resource.rs) / [window](../../../crates/quanta-index-core/src/domains/semantic/stream.rs) / [F15](../../../crates/quanta-index-lexical/src/file_authority.rs) / [process config](../../../crates/quanta-index-searchd/src/app/config.rs) | 범위별 한도와 process 선언 회계/RSS gate의 차이 |
| [Cargo.lock](../../../Cargo.lock) | 재대조 기준의 Lance 7.0.0. 최신 문서만으로 선택된 API·feature 사용을 단정하지 않음 |

## 기존 완료 증거와 다른 작업

- [Plan custody](../../adr/OCT-05-004-cost-capacity-and-qualification-boundaries.md#publication-plan-custody-and-remaining-structural-cost): 이미 완료된 반복 plan 파생 수리.
- [Publication-only output](../../adr/OCT-05-004-cost-capacity-and-qualification-boundaries.md#publication-only-verification-output): 조회 객체를 보관하지 않는 canonical 검증 경로.
- [Matching XL](../../adr/OCT-05-004-cost-capacity-and-qualification-boundaries.md#post-repair-matching-xl-diagnostic): frozen source의 비용·회계·sampled RSS와 qualification 한계.
- [기존 bounded publication](../../adr/OCT-05-004-cost-capacity-and-qualification-boundaries.md#bounded-publication-and-interruption): staged upload/EOF/checked admission을 새 작업으로 다시 만들지 않음.
- [Replay CAS 수리](../../adr/OCT-05-003-active-query-and-runtime-lifecycle.md#replay-cas-preflight-authority-repair): 원 publication/receipt를 기준으로 하는 복구.
- [Explicit publication 후보](../../adr/OCT-05-003-active-query-and-runtime-lifecycle.md#explicit-publication-and-activation-candidate): SDK/producer 결합 통합 경계.
- [기존 회귀·coverage](../../adr/SEP-27-005-catalog-recovery-supervision-and-proof-custody.md#implemented-lifecycle-tests-and-remaining-coverage): 과거 실행 결과와 새 acceptance를 구분.
- [잔여 작업 인덱스](../oct-4-parallel-closure/tickets/INDEX.md): 프로젝트 전체 진행·main/candidate 상태의 기존 위치.

## 외부 1차 레퍼런스

| 자료 | 적용할 원리 | 적용 한계 |
| --- | --- | --- |
| [Tantivy IndexWriter](https://docs.rs/tantivy/latest/tantivy/indexer/struct.IndexWriter.html) | commit의 지속성/가시성, 살아 있는 segment metadata와 파일 보존 | 엔진 commit이 Index의 lexical+semantic pair activation 원자성을 대신하지 않음 |
| [RocksDB Full File Checksum](https://github.com/facebook/rocksdb/wiki/Full-File-Checksum-and-Checksum-Handoff) | 파일 identity와 checksum/알고리즘을 manifest에 결속, ingestion/전송 검증 | CRC 기반 corruption 검출을 우리 SHA-256 무결성·semantic proof와 동일시하지 않음 |
| [Lance Index Format](https://lance.org/format/index/) | immutable index files, lazy loading, index coverage, compaction 시 주소 remap | stable-row-ID index는 문서상 experimental; 우리 버전/옵션의 지원 증거가 필요 |
| [Lance Performance Guide](https://lance.org/guide/performance/) | fragment 수와 manifest 비용, fragment 크기와 갱신 비용의 tradeoff; scan buffer soft limit | 최신 기능·기본값은 우리 profile에 자동 적용하지 않음; 자체 RSS/fragment 비용 측정 필요 |
| [Lance Fragment Metadata Tree](https://lance.org/format/table/fragment_metadata/) | flat metadata의 페이지화 설계 참고 | 확인 시점에 unstable proposal이며 릴리스된 Lance의 생성/읽기 지원 없음. 즉시 도입 의존성으로 사용 금지 |
| [Lance Row ID and Lineage](https://lance.org/format/table/row_id_lineage/) | logical ID와 physical row address, lineage 구분 | 주소 불변을 가정하지 않음; 삭제·compaction 이후 coverage/destination을 확인 |
| [AWS Idempotent APIs](https://aws.amazon.com/builders-library/making-retries-safe-with-idempotent-APIs/) | 동일 token의 입력 일치, 원 결과의 의미 보존, 늦은 재시도와 보존 기간 | 정확한 epoch·expiry·pending 정책은 우리 owner 계약으로 정의; 일반 문서로 exactly-once를 주장하지 않음 |
| [RocksDB Write Buffer Manager](https://github.com/facebook/rocksdb/wiki/write-buffer-manager) | 여러 writer/DB의 공유 memory charge, flush/stall로 backpressure | soft memtable budget은 전체 프로세스 hard RSS 한도가 아님 |
| [Crossbeam Epoch](https://docs.rs/crossbeam-epoch/latest/crossbeam_epoch/) | 접근 전에 보호를 확보하고 기존 reader가 끝난 뒤 회수 | 메모리 회수 원리의 참고. 라이브러리 도입 자체가 파일·다중 프로세스 GC를 해결하지 않음 |
| [E5 제작자 모델 카드](https://huggingface.co/intfloat/multilingual-e5-large) | query/document의 다른 prefix와 명시적 normalization 계약 | 모델 교체 추천이 아님. 정책을 무조건 동일하게 비교하면 안 되는 구체적 반례 |

이 자료들이 지지하는 것은 설계 원리다. SOTA 성능, 특정 지연, corpus 무제한 처리,
전체 RSS 절대 상한, 모든 장애에서의 완료는 [실제 합격 조건](VALIDATION.md) 없이 선언하지 않는다.
