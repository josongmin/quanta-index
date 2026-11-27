# 01 · 기존 실행 복구와 읽기 비용 분리

**담당과 산출물.** [이 채팅 B](README.md#채팅별-실행권과-재사용-범위)가 기존 controller를 확인한다. TEXT·Catalog는 현재 trace로 판별할 수 없는 비용만 계측한다. 산출물은 같은 operation에 대한 단계별 읽기 차액, 실패 위치 또는 초기 게시 결과, 그리고 02·03·04 중 다음 작업이다. [현재 실패 분기](04-product-acceptance.md#가장-먼저-실패한-경계로-분기) · [선행 감사 원문](../../ARCHIVE-INDEX.md#nov-20-ann-audit-preimage)

## 실행을 이어받는 위치

기존 continuation의 종료 상태와 소유권 복구 조건은 [00 실행기 반례](00-final-audit.md)에 따른다. 새 상태를 저장하기 전 실패하거나 PID가 재사용되면 실행 로그와 잠금까지 확인한다. 이미 시작된 본문의 상태가 불명확하면 복구용 미시작 대기로 분류하지 않는다.

아래 경로는 원 실행 호스트의 `~/Library/Caches/quanta-index/` 기준이다. 상태와 PID는 실행 직전에 다시 읽는다.

| 위치 | 확인 또는 복구할 내용 |
| --- | --- |
| `ann4k-materialization-diagnostic-mug1rt81/` | 초기 ENOSPC 뒤 회수한 prefix 본문이 1 PASS·exit 0으로 끝났다. `PREFIX4K.log`와 `trace/ann-materialization-4096-prefix128.qtrace`를 판독한다. |
| `ann4k-owner-budget-repair-uls30q9w/` | `ANN4K/`는 1 PASS·exit 0, `PRODUCT4K/`는 1 FAIL·exit 100 terminal이다. `quality-pair.json`은 `quality_incomplete`다. [두 결과](04-product-acceptance.md#기존-실행기-결과) |
| `ann4k-late-record-6cO3Bn2x/` | 미시작 예약 이력. PID 5314가 사라졌고 child·본문 로그·trace·terminal이 없다. 종료 원인은 기록되지 않았다. |
| `ann4k-late-record-h4a2lcwt/` | 같은 controller로 재접수했으나 admission 대기 중 preflight·상태 쓰기에서 `ENOSPC`가 났다. PID는 종료, child·본문·trace는 부재다. [실행 기록](04-product-acceptance.md#후기-worker-계측-준비) |
| `ann4k-late-record-5w6y78vu/` | 직전 후기 Record. PID 89072가 종료됐으며 상태의 대기 표시는 잔류 값이다. 본문·trace·terminal이 없어 **NOT_RUN**이다. 다음 접수는 이 슬롯을 보존하고 기존 admission과 공용 잠금으로 한 건만 수행한다. [최신 판정](04-product-acceptance.md#현재-코드와-실행-판정) |
| `ann4k-resume-20261117/run_quality.py` | 원 admission·정확한 Nextest 본문·소스 및 resolver 비교의 실행 권한. 복구 스크립트가 이 파일의 SHA를 확인한다 |

1. `controller_pid`와 `child_pid`의 실제 명령, 기존 실행 잠금, 본문 로그·trace와 terminal을 대조한다. 실행 중이면 그 실행을 이어서 관찰한다. 본문이 시작되었거나 종료했다면 로그부터 인수한다.
2. Prefix와 ANN 예약의 초기 ENOSPC 회수는 기존 `resume_queued.py`가 처리했고 두 본문의 terminal이 남아 있다. 후기 Record는 별도 진단 슬롯이다. 다음 실행 전 원 controller와 공용 잠금을 확인한다. 앞선 슬롯을 미시작 예약으로 되돌리거나 상태 파일을 수동 수정하지 않는다. [호스트 admission](../../../AGENT_PLAYBOOK.md#tests)

## 원인 판별

prefix의 실제 본문은 `ann_quality::materialization_diagnostic::diagnostic_native_ann_4096_source_128_completed_prefix`다. 제품과 같은 4,096개 소스를 제출하고 128개 완료 receipt를 관찰한다. fixture 로그의 daemon qtrace 경로를 `trace_path`에 지정한 뒤 아래 명령을 사용한다. [prefix 본문](../../../crates/quanta-index-searchd-runtime/tests/native_product_process/ann_quality/materialization_diagnostic.rs#L288) · [리포트 구현](../../../tools/benchmark/retrieval/causal_cost_native_ingest.py#L164)

```sh
python3 -m tools.benchmark.retrieval.causal_cost_diagnostic native-ingest \
  --input "$trace_path" --max-capture-bytes 268435456
```

- 같은 요청의 `native_source_apply/3`, `build_prepare/3`, `lexical_build/2`, `native_text_postings/49`, `native_coverage_map_apply`에서 완료 여부와 논리 읽기·쓰기·bytes·mutations를 비교한다. 중첩된 부모와 자식 수치는 합산하지 않는다. `PARTIAL_DIAGNOSTIC`이면 닫힌 span만 사용하고 미관측 비용은 미상으로 둔다.
- 이전 제품 로그의 `NativeCoverageMapApply`는 등록된 자식 메타데이터를 읽은 뒤 edge 검증과 회수 인수를 위해 읽기 4회를 청구하다 거절됐다. 당시 소스 캡처의 `managed_install.rs` SHA-256은 현재 파일과 일치하고, 당시 실행의 소스 drift는 0건이다. 4회는 해당 작업 실행과 사용량 반영 전에 거절됐다. 성공한 별도 prefix의 닫힌 span으로 같은 종류의 단계 비용을 비교할 수 있다. 실패 실행에서 이미 승인된 999,999회의 단계별 기여도는 미관측이다. [자식 검증 청구](../../../crates/quanta-index-catalog/src/native/authority/managed_install.rs#L176) · [원 로그 위치](04-product-acceptance.md)
- 기본 출력은 선택된 요청 최대 32개다. `COMPLETE_DIAGNOSTIC`이어도 `requests`에 목표 요청이 없을 수 있다. `matched_requests`, `omitted_requests`, `kernel_spans_without_request`와 해당 `request_identity`·닫힌 span을 먼저 확인한다. 누락 시 같은 capture를 더 큰 `--limit`로 다시 읽는다. 목표 요청과 operation(2)의 관계를 확인할 수 없으면 비용 원인을 확정하지 않는다. [요청 선택과 집계](../../../tools/benchmark/retrieval/causal_cost_native_ingest.py#L205)
- 180초 prefix가 source 게시 marker를 남기고 128개 완료 관측 전에 끝날 수 있다. marker는 초기 게시 증거다. 128 receipt는 별도로 판정한다. Record 기한만 만료되고 게시됐는지 판단할 자료가 없으면 제품의 1,200초 Off 테스트가 실패했다고 간주하지 않는다. [게시 marker와 완료 검사](../../../crates/quanta-index-searchd-runtime/tests/native_product_process/ann_quality/materialization_diagnostic.rs#L412)
- lane 49는 `paths.finish`만 감싼다. 파일별 수집이 분리되지 않으면 `PathWrites`에서 성공한 배치·행 수와 scratch 작업 전후의 읽기 차액을 집계한다. 맵 설치에는 기존 `install_census` 및 외부 자식 검증 수치를 먼저 쓴다. 실패 요청에서 census가 비어 있으면 닫힌 설치 구간에서 확인되는 부분만 기록한다. 계측의 on/off에서 root·오류·예산 사용량이 같은지 기존 회귀로 확인한다. [설치 span](../../../crates/quanta-index-lexical/src/native/text.rs#L14129) · [계측 범위](../../../tools/benchmark/retrieval/causal_cost_native_ingest.py#L271)
- **인계 조건.** 현재 패킹으로 게시가 확인되면 04. 파일별 scratch 제출이 주요 비용이면 02. 비용이 경로 맵 설치·자식 검증에 집중되고 객체 수와 함께 증가하면 03. 그 밖의 단계가 지배하면 해당 단계와 증거를 기존 담당에게 넘긴다. trace가 불충분하면 빠진 계측을 특정하고 같은 prefix로 판별한다.

## 후기 실행의 판독 기준

Prefix는 초기 게시와 첫 128개 완료의 근거다. 후기 원인 비교에는 원 Product4K 본문의 첫 daemon이 작성한 `ann-initial.qtrace`를 사용한다. 해당 실행기의 PID·실명·본문·종료값과 소스·resolver 입력을 먼저 확인한다. Record의 결과는 진단 범위에 귀속한다.

1. `native-ingest`에서 원 SourceAdvance 요청과 `start_ns`를 찾는다. `closed-workers --occupancy-only --window-ns START END --max-capture-bytes 1073741824`에는 같은 qtrace의 절대 시각을 전달한다. 첫 worker가 SourceAdvance 응답보다 먼저 시작할 수 있다. 단계별 비용이 필요하면 `--occupancy-only` 대신 `--minimum-closed-workers N`을 지정한다. [CLI](../../../tools/benchmark/retrieval/causal_cost_diagnostic.py#L1114)
2. 닫힌 worker 점유가 작으면 worker 밖에서 시간이 빠지는 경계를 조사한다. 점유가 크면 동일 구간의 Catalog 락 대기·WAL/main sync·backfill·객체 설치·ANN build/seal을 대조한다. 열린 worker, 누락 이벤트, 캡처 밖 시간은 별도 기록한다. wall time이 겹치는 `WorkStatus` 관측과 worker 실행은 합산하지 않는다.
3. 원 watchdog 만료를 처리하며 daemon을 강제 종료하면 footer가 없을 수 있다. 건강한 canonical partial의 관측 구간만 사용할 수 있다. 후기 표본 또는 시각 경계가 부족하면 원인을 미확정으로 유지한다. [실패 정리](../../../crates/quanta-index-searchd-runtime/tests/native_product_process/ann_quality.rs#L955) · [코드 잔여](04-product-acceptance.md#현재-코드와-실행-판정)

## Prefix 실측 결과

기존 실행기에서 **VERIFIED**, 1 PASS·exit 0(`63cc2a94-2ea8-46f8-ac35-555ef3ebcafb`). 4,096개 source가 게시됐고 완료 영수증 128개를 관찰했다. 이 실행의 소스·resolver drift는 각각 0건이다. [04 제품 수용](04-product-acceptance.md)으로 진행한다.

`trace/ann-materialization-4096-prefix128.qtrace`를 `native-ingest`로 읽었다. 닫힌 control 요청 `(connection_id=73, request_id=72)`은 `ok`로 끝났고 19.320초로 fixture의 `source-submit`과 일치한다. 표에 적은 단계는 정상 종료했다. 리포트에 직접적인 operation key 연결은 없다.

| 관측 단계 | 승인된 읽기 |
| --- | ---: |
| `native_source_apply/3` | 90,857 |
| `build_prepare/3` | 760,090 |
| `lexical_build/2` | 678,560 |
| `native_text_postings/2` | 289,962 |
| `paths.finish` lane 49 | 145,075 |
| `native_coverage_scan/3` | 1,536 |
| `native_coverage_map_apply/3` | 79,553 |
| `native_searchable_prepare/3` | 1 |
| `native_coverage_save/3` | 170 |

부모·자식 수치는 합산하지 않는다. 현재 `build_prepare`에서 `lexical_build`의 관측 읽기가 가장 크다. qtrace는 `PARTIAL_DIAGNOSTIC`이며 17,858개 입력 이벤트가 누락되고 5개 span이 열린 채 끝났다. 474,637개 수락 이벤트는 모두 기록됐고 61,877,651바이트에서 정상 종료했다. 첫 미종결 span은 `native_source_map_apply` 아래의 `managed_object_confirm/3`이다. 표는 카운터가 있는 닫힌 span만 사용한다. 과거 실패에는 단계별 승인 읽기 내역이 없어 두 실행의 비용 차이를 개별 단계에 귀속할 수 없다.

같은 캡처를 `closed-workers`로 읽으면 닫힌 worker 범위 134개가 나온다. 133개는 정상 종료했고 1개는 `aborted`다. 전체 worker wall time 중앙값은 176.341ms였다. 관측 단계에서 `native_coverage_save/3` 중앙값이 63.756ms로 가장 컸고, 그 내부 Catalog 잠금 보유 중앙값은 40.499ms였다. 이 시간은 서로 겹친다. 정상 종료 133개에서 각각 CoverageSave 아래 reserve·install·confirm이 한 번씩, Catalog 잠금 보유가 두 번 관측됐다. 초기 설치 그룹을 더 합칠 근거는 확인되지 않았다. 4K 후반 materialization의 건당 지연은 별도로 계측해야 한다.
