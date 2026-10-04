# O4-E4-04 — source-bound distinct token authority

| 항목 | 값 |
| --- | --- |
| 에픽 / 담당 | [E4 — 인덱싱·typo 실행 비용·release 성능·scale](../epics/E4-storage-query-and-scale.md) / E4 담당 |
| 우선순위 / 종류 | P2 / `CONDITIONAL_CODE` |
| 실행 상태 | `PLANNED` — 본 티켓의 구현·실행·검증은 `NOT_RUN` |
| 선행 결과 | [O4-E4-01](O4-E4-01-index-phase-profile.md), [O4-E4-03](O4-E4-03-ascii-scanner-decision.md) |

[전체 지도](../README.md) · [티켓 인덱스](INDEX.md)

## 목적

반복 tokenization이 계속 지배할 때 generation/source-bound token postings와 정확한 witness를 한 권위로 재사용한다.

## 배경과 현재 상태

현재 trigram shortlist, source file authority, query-local OSA distance cache, declaration evidence가 있다. proposed distinct identifier tokens는 기존 gram dictionary와 다른 단위지만 ingestion/residency/storage 비용을 늘린다. complete fallback을 heuristic shortlist만 보고 제거할 수 없다.

## 착수 입력

- E4-01/03 after-scanner profile에서 persistent token-scan bottleneck
- 독립 exhaustive tokenizer+full-DP OSA1 oracle, source digest/name spans/declaration attestation
- build/cold-open/residency/delta/delete budget와 accepted typo policy

## 어떤 파일을 어떻게 수정할지

`OWNED`는 에픽 담당 통합, `SHARED`는 I0 반영, `READ`는 기존 구현 소비다. 재현된 결함이나 채택된 계약 변경이 있을 때만 product source를 수정한다. 구현 파일과 독립 검증 파일을 함께 지정한다.

| 파일 | 함수 / 경계 | 구체적인 변경 또는 검증 | 모드 |
| --- | --- | --- | --- |
| [crates/quanta-index-lexical/src/file_authority.rs](../../../../crates/quanta-index-lexical/src/file_authority.rs) | from_verified_files / source_posting_memberships | distinct raw spelling→source files/verified byte witnesses를 current generation authority에 결속하고 admission bounds를 계산한다. | OWNED |
| [crates/quanta-index-lexical/src/searcher/code_search.rs](../../../../crates/quanta-index-lexical/src/searcher/code_search.rs) | typo shortlist/witness/distance cache path | generation-bound token authority를 소비해 반복 source scans를 줄이되 complete fallback·budget/cancel·unknown coverage를 유지한다. | OWNED |
| [crates/quanta-index-lexical/src/searcher/code_search/ranking.rs](../../../../crates/quanta-index-lexical/src/searcher/code_search/ranking.rs) | distance/declaration/occurrence ordering | 현 distance>declaration>bounded occurrence와 cursor signature를 보존하거나 명시적 policy revision으로 검사한다. | OWNED |
| [crates/quanta-index-lexical/src/adapter_open.rs](../../../../crates/quanta-index-lexical/src/adapter_open.rs) | verified cold-open authority | stale digest/format/posting bounds를 cold-open에서 검증한다. parallel legacy readers를 만들지 않는다. | OWNED |
| [crates/quanta-index-lexical/tests/l3_exact_source.rs](../../../../crates/quanta-index-lexical/tests/l3_exact_source.rs) | typo exhaustive fixtures | all matching files/name witnesses, short/case/Unicode/collision/ambiguous names, pages/cursor/cancel controls를 추가한다. | OWNED |
| [crates/quanta-index-lexical/tests/l2_file_mutation.rs](../../../../crates/quanta-index-lexical/tests/l2_file_mutation.rs) | token authority delta/delete | changed source/delta/delete와 fresh rebuild 동등성을 검증한다. | OWNED |

## 실행 단계

1. bottleneck·expected benefit·memory/build budget가 충족될 때만 설계한다. 성립하지 않으면 근거로 NOT_APPLICABLE 종료한다.
2. raw spelling/canonical mapping/source digest와 exact file membership/witness를 typed authority로 정의한다.
3. 하나의 canonical producer/open/consumer를 함께 업데이트하고 grammar/declaration unknown은 보존한다.
4. 모든 OSA1 edit·짧은 이름·case·mixed Unicode에 대해 exhaustive oracle와 후보 completeness를 검증한다.
5. delta/delete/reopen/source mutation 및 budget/cancel/cursor를 검사한다.
6. query 비용뿐 아니라 ingest/cold-open/resident/disk 비용을 E4-06에서 비교하고 completeness 전 fallback 제거를 금지한다.

## 검증 계획 — NOT_RUN

아래는 실행할 명령/시나리오다. 본 문서에서 통과를 주장하지 않는다. `<...>`와 외부 root는 실행 전에 실제 값으로 확정한다. test filter는 실제 수집 ID를 확인하고 0 tests를 성공으로 표시하지 않는다.

- `./scripts/cargow test -p quanta-index-lexical --lib --test l3_exact_source --test l2_file_mutation --locked`
- module/public/wire surface에 실제 변경이 있으면 해당 canonical escalation gates를 I0가 실행한다.
- Negative: stale source/grammar, wrong folded byte witness, missing near tokens, false exact-name collision, budget/cancel bypass, postings cap overflow 거절.

## 완료 조건

- exhaustive candidate/name witness oracle와 lifecycle parity가 통과하고 declared memory/build/latency tradeoff가 acceptance를 충족한다.
- conditional threshold가 미충족이면 새 token index를 추가하지 않고 disposition을 남긴다.

## 중단·거절·재개 조건

- 삼각 gram heuristic을 OSA1 completeness 증명으로 취급하지 않는다. immutable source/coverage pack 변경은 이 티켓에 묶지 않는다.
- 필요한 입력 부재는 `BLOCKED`, 미실행은 `NOT_RUN`, 실제 실행 실패는 `FAILED`로 기록한다. 조건 미성립 `NOT_APPLICABLE`에는 실제 판단 근거가 필요하다.
- 변경이 source/input/query/unit/result에 영향을 주면 [I0 source gate](O4-I0-02-matching-source-proof.md)와 영향받는 capture/report를 다시 판정한다.
- 일회성 raw/log/capture/receipt는 checkout 밖 새 root에 둔다. 기존 외부 terminal을 덮어쓰지 않는다.

## 인계 결과

- 실제 source/dirty ownership, 변경 파일과 계약, 실행한 명령/selector, 관측 결과 및 제외 범위.
- raw/model/runtime/binary/input identity는 해당 실행 계약이 요구하는 범위에서 기록한다.
- 완료 조건별 `VERIFIED`/`FAILED`/`BLOCKED`/`NOT_RUN`/`NOT_APPLICABLE`과 후속 티켓에 넘길 입력을 발행한다.
