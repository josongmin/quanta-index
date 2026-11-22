# 01 · 기존 실행 복구와 읽기 비용 분리

**담당과 산출물.** [이 채팅 B](README.md#채팅별-실행권과-재사용-범위)가 기존 controller를 확인한다. TEXT·Catalog는 현재 trace로 판별할 수 없는 비용만 계측한다. 산출물은 같은 operation에 대한 단계별 읽기 차액, 실패 위치 또는 초기 게시 결과, 그리고 02·03·04 중 다음 작업이다. [선행 RCA](../../bugbash/nov-20/findings.md#L129)

## 실행을 이어받는 위치

기존 continuation의 종료 상태와 소유권 복구 조건은 [00 실행기 반례](00-final-audit.md)에 따른다. 새 상태를 저장하기 전 실패하거나 PID가 재사용되면 실행 로그와 잠금까지 확인한다. 이미 시작된 본문의 상태가 불명확하면 복구용 미시작 대기로 분류하지 않는다.

아래 경로는 원 실행 호스트의 `~/Library/Caches/quanta-index/` 기준이다. 상태와 PID는 실행 직전에 다시 읽는다.

| 위치 | 확인 또는 복구할 내용 |
| --- | --- |
| `ann4k-materialization-diagnostic-mug1rt81/` | `state.json`, `admission-controller.log`, `PREFIX4K.log`, `trace/`. 확인 당시 PID 26624 종료, 로그에 `PREFIX4K-preflight.txt`·`state.tmp` 쓰기 ENOSPC, 본문 로그와 capture 없음 |
| `ann4k-owner-budget-repair-uls30q9w/ANN4K/` | 기록 PID 26633 종료. `runs[0].status`는 여전히 `waiting_for_existing_diagnostic`. 동일 selector의 신규 PID·로그·terminal이 생겼는지 대조 |
| `ann4k-owner-budget-repair-uls30q9w/` | `resume_queued.py`가 기존 대기 디렉터리와 잠금을 회수한다. `source-bound-local/build-env.json`의 target이 존재하고 상태·로그·빌드를 쓸 공간이 있는지 확인. `product_after_prefix.py`는 확인 당시 실행 중이며 prefix 게시와 ANN terminal을 기다린다 |
| `ann4k-resume-20261117/run_quality.py` | 원 admission·정확한 Nextest 본문·소스 및 resolver 비교의 실행 권한. 복구 스크립트가 이 파일의 SHA를 확인한다 |

1. `controller_pid`와 `child_pid`의 실제 명령, 기존 실행 잠금, 본문 로그·trace와 terminal을 대조한다. 실행 중이면 그 실행을 이어서 관찰한다. 본문이 시작되었거나 종료했다면 로그부터 인수한다.
2. 두 대기가 모두 **본문 시작 전**에 중단됐고 기존 복구 스크립트의 선행검사가 맞으면, 저장소 루트에서 `python3 -B "$HOME/Library/Caches/quanta-index/ann4k-owner-budget-repair-uls30q9w/resume_queued.py"`로 한 번 회수한다. 하나만 중단됐다면 해당 label 인자(`PREFIX4K` 또는 `ANN4K`)만 쓴다. PID 갱신·잠금·본문 진입을 확인한다. `state.json`을 수동 수정하거나 기존 product continuation을 중복 실행하지 않는다. [호스트 admission](../../../AGENT_PLAYBOOK.md#L49)

## 원인 판별

prefix의 실제 본문은 `ann_quality::materialization_diagnostic::diagnostic_native_ann_4096_source_128_completed_prefix`다. 제품과 같은 4,096개 소스를 제출하고 128개 완료 receipt를 관찰한다. fixture 로그의 daemon qtrace 경로를 `trace_path`에 지정한 뒤 아래 명령을 사용한다. [prefix 본문](../../../crates/quanta-index-searchd-runtime/tests/native_product_process/ann_quality/materialization_diagnostic.rs#L288) · [리포트 구현](../../../tools/benchmark/retrieval/causal_cost_native_ingest.py#L164)

```sh
python3 -m tools.benchmark.retrieval.causal_cost_diagnostic native-ingest \
  --input "$trace_path" --max-capture-bytes 268435456
```

- 같은 요청의 `native_source_apply/3`, `build_prepare/3`, `lexical_build/2`, `native_text_postings/49`, `native_coverage_map_apply`에서 완료 여부와 논리 읽기·쓰기·bytes·mutations를 비교한다. 중첩된 부모와 자식 수치는 합산하지 않는다. `PARTIAL_DIAGNOSTIC`이면 닫힌 span만 사용하고 미관측 비용은 미상으로 둔다.
- 이전 제품 로그의 `NativeCoverageMapApply`는 등록된 자식 메타데이터를 읽은 뒤 edge 검증과 회수 인수를 위해 읽기 4회를 청구하다 거절됐다. 당시 소스 캡처의 `managed_install.rs` SHA-256은 현재 파일과 일치하고, 당시 실행의 소스 drift는 0건이다. 4회는 해당 작업 실행과 사용량 반영 전에 거절됐다. 앞서 승인된 999,999회의 단계별 기여도는 prefix trace의 닫힌 span에서 확인한다. [자식 검증 청구](../../../crates/quanta-index-catalog/src/native/authority/managed_install.rs#L176) · [원 로그 위치](04-product-acceptance.md)
- 기본 출력은 선택된 요청 최대 32개다. `COMPLETE_DIAGNOSTIC`이어도 `requests`에 목표 요청이 없을 수 있다. `matched_requests`, `omitted_requests`, `kernel_spans_without_request`와 해당 `request_identity`·닫힌 span을 먼저 확인한다. 누락 시 같은 capture를 더 큰 `--limit`로 다시 읽는다. 목표 요청과 operation(2)의 관계를 확인할 수 없으면 비용 원인을 확정하지 않는다. [요청 선택과 집계](../../../tools/benchmark/retrieval/causal_cost_native_ingest.py#L205)
- 180초 prefix가 source 게시 marker를 남기고 128개 완료 관측 전에 끝날 수 있다. marker는 초기 게시 증거다. 128 receipt는 별도로 판정한다. Record 기한만 만료되고 게시됐는지 판단할 자료가 없으면 제품의 1,200초 Off 테스트가 실패했다고 간주하지 않는다. [게시 marker와 완료 검사](../../../crates/quanta-index-searchd-runtime/tests/native_product_process/ann_quality/materialization_diagnostic.rs#L412)
- lane 49는 `paths.finish`만 감싼다. 파일별 수집이 분리되지 않으면 `PathWrites`에서 성공한 배치·행 수와 scratch 작업 전후의 읽기 차액을 집계한다. 맵 설치에는 기존 `install_census` 및 외부 자식 검증 수치를 먼저 쓴다. 실패 요청에서 census가 비어 있으면 닫힌 설치 구간에서 확인되는 부분만 기록한다. 계측의 on/off에서 root·오류·예산 사용량이 같은지 기존 회귀로 확인한다. [설치 span](../../../crates/quanta-index-lexical/src/native/text.rs#L14129) · [계측 범위](../../../tools/benchmark/retrieval/causal_cost_native_ingest.py#L271)
- **인계 조건.** 현재 패킹으로 게시가 확인되면 04. 파일별 scratch 제출이 주요 비용이면 02. 비용이 경로 맵 설치·자식 검증에 집중되고 객체 수와 함께 증가하면 03. 그 밖의 단계가 지배하면 해당 단계와 증거를 기존 담당에게 넘긴다. trace가 불충분하면 빠진 계측을 특정하고 같은 prefix로 판별한다.

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
