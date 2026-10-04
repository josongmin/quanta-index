# O4-E2-04 — 5제품 실제 캡처와 blind union 반환

| 항목 | 값 |
| --- | --- |
| 에픽 / 담당 | [E2 — 외부 제품 native 범위·응답 경계·실제 캡처](../epics/E2-external-capture-and-timing.md) / E2 담당 |
| 우선순위 / 종류 | P1 / `EXECUTION` |
| 기준 웨이브 | [W4 — 실제 캡처·성능·scale](../waves/W4-native-capture-performance-and-scale.md) |
| 실행 상태 | source0e6 bat409 warmup0/1 각40/40·독립 verdict/full parity 및 당시 native3 실제60요청/독립 raw replay `VERIFIED`. 수정 scorer의 SG scope receipts 재발행 중; fresh native3/5제품 join·required matrix·정식 qualification 미완료 |
| 선행 결과 | [O4-E1-03](O4-E1-03-admission-and-split.md), [O4-E2-02](O4-E2-02-external-index-universe.md), [O4-E2-03](O4-E2-03-required-cells-and-scheduling.md), [O4-I0-02](O4-I0-02-matching-source-proof.md) |

[전체 지도](../README.md) · [티켓 인덱스](INDEX.md)

## 목적

최종 admitted cells를 실제 제품에 실행해 최신 비교용 raw를 만들고 새로운 미검수 candidates를 E1로 전달한다.

## 배경과 현재 상태

Quanta33캡처/11,272응답과 historical5제품21,815행은 별도의 source/시점이다. Gin exact1196, default/explicit typo, NL/literal API, ARB original/adapted를 서로 다른 request 계약으로 유지해야 한다.

## 착수 입력

- E2-03 required/reuse inventory, E1-03 issued admissions, I0-02 matching runner/daemon/proof
- 제품별 native indexed scope와 config/service/binary identity, fresh external roots
- Gin20 및 ARB 원문17/88 부분과 adapted88 historical 범위를 구별한 input manifest

## 2026-10-04 source107 bat 실제 실행과 평가 거절

- `FAILED`: clean source107에서 `uv run --frozen --extra dev python tools/benchmark/retrieval/run.py pair --spec /private/tmp/qi-p0-plan-v1/bat.json`를 실행했다. producer exit2이며 `complete scored file comparison lacks ordered, judged semble-lexical-file rows`로 평가 단계에서 거절됐다. spec·prepared input hashes와 source는 실행 전후 동일했다.
- `/private/tmp/qi-p0-v1/bat.staging`에 Quanta20 lexical rows(`capped`)와 Semble20 lexical-file rows(`success`), 각21 timing observations(cold1+measured20)가 남았다. 판정되지 않은 `(task_id,path)`는 Semble22/14tasks, Quanta34/16tasks, 합집합51쌍이다. 이 거절은 관측된 index 결함이나 0점 판정이 아니다.
- 최종 `run-manifest.json`, protocol-lock, verdict 및 promoted output root가 없다. 따라서 PAIR_VALID·warmup parity·최종 제품 비교 PASS로 재사용하지 않는다. route별 projected suite와 기존 `holdout_review.capture_review_pool`로 raw를 재검증해 미판정 리뷰 입력을 발행하는 단계만 가능하다.
- 실제 supplemental 판단 후 새 merged suite/pack/receipt/admission을 발행하고 fresh final pair를 실행해야 한다. suite/pack identity가 달라지는 새 epoch에 기존 staging record를 최종 증거로 이식하지 않는다.

- `VERIFIED`: root가 source107의 canonical native collector `--spec /private/tmp/qnp1/bat.capture-spec.json`→`--verify /private/tmp/qn/bat`를 실제 실행해 각각 exit0이었다. prepared E1 admission과 native template를 검증했으며 source/input hashes 전후 동일하다. `/private/tmp/qna1/terminal.json`은 producer/raw_replay VERIFIED, formal lexical comparison BLOCKED, qualified false다.
- SG/OG/cs 각각20rows, HTTP200/200·cs exit0이다. 이번 NL query의 returned-file 수는 셋 모두0이다. Sourcegraph79-file owned native path/stored-document scope는 결속됐고 OpenGrok 전체 indexed-universe attestation은false로 유지됐다. 나머지 required7셀은 이 bat 결과로 실행 완료 처리하지 않는다.

## 2026-10-05 source0e6 fresh pair 연결 점검

- `/tmp/qi-bat-pair-0e6-w1-input-v1.json`은 actual source0e6 Contract/fresh SDK proof·binary와 bat409 v3 admission121paths를 결속했다. 다른11개 admission은 null로 두어 과거 source107 결과를 current readiness로 소비하지 않는다.
- pair v7의 `/private/tmp/qpbw1p` preflight는 exit2, canonical E1 result identity 거절로 bat `BLOCKED`였다. 다른 required7개는 미선택 `NOT_RUN`이다. actual pair·warmup parity·scoring은 이 실행에서 `NOT_RUN`이다.
- E1-03의 external producer v4가 canonical16paths/strict keys를 재발급한 뒤 새 packet/plan root에서 canonical preflight를 다시 수행한다. rejected v3 결과를 고쳐 쓰거나 consumer validator를 완화하지 않는다.
- 후속 `/tmp/qi-bat-pair-0e6-w1-input-v2.json` SHA `83d6eca47328fe7f9d230beacb54fb94295e5ba795f2f44f8461ddbd20111dc2`로 `/private/tmp/qpbw1p2` preflight를 완료했다. bat PREPARED, 다른 required7개 NOT_RUN이다. canonical consumer 전체 replay 통과 뒤 root가 동일 prepared spec으로 `run.py pair --spec /private/tmp/qpbw1p2/bat.json`을 실행 중이다. output은 새 `/private/tmp/qbw1/bat`, source0e6·409판단·warmup1·quality-only/no-speed 조건이다.
- 후속 warmup1 producer `VERIFIED`: 실제 exit0, input binding 전후 동일, `/private/tmp/qbw1/bat`으로 atomic promotion됐다. verdict는 selected/executed/passed40/40/40, failed0, PAIR_VALID/CONTRACT_GREEN/SDK_PATH_GREEN pass, QUALITY_DELTA not_applicable(`attested_only`), PERF_QUALIFIED not_applicable(`no_speed_claim`)다. Quanta20 capped/Semble20 success를 정상 status 의미로 유지했다. 독립 `run.py verdict`를 별도 외부 replay path에서 실행 중이며 아직 그 통과를 주장하지 않는다.
- 동일 source/suite/pack/binaries의 warmup0 control을 `/private/tmp/qpbw0/bat.json`→`/private/tmp/qbw0/bat`에서 실행 중이다. 차이는 warmup 횟수/output root/run ID3개뿐이고 warmup0 정책 채택이나 속도 qualification을 발행하지 않는다.
- warmup1 독립 `run.py verdict` 후속 `VERIFIED`: exit0, producer verdict와 JSON equality를 확인했다. SHA `ae4e4ee7ab23d359bf76e46e6889f481e589929dfcb475233da0e1bef86d58a4`,40/40passed·failed0·동일5states다. source0e6 새 native3 spec은 `/private/tmp/qi-native-bat-0e6-spec-v1.json` SHA `f60ce4c1bcd0bdcb07344e740f3078dfbf638e9a09968a11381273e592d39e5e`로 suite/pack409를 참조하며 actual capture는 아직 NOT_RUN이다.
- 후속 warmup0 actual pair 및 독립 verdict `VERIFIED`: producer/replay 각각 exit0,40/40passed·failed0, `/private/tmp/qbw0/bat/verdict.json`과 replay JSON이 일치했다. replay SHA `20a65be9b453f0e97cec710aff6ed0589de7cfd99ccc2761062ffd0e2bee5b7a`다. full protocol/phase/normalized parity는 별도 실행 중이며 두 producer 성공만으로 정책을 채택하지 않는다.
- 후속 current native3 `VERIFIED`: 위 spec으로 canonical collector 실제 실행 및 `--verify /private/tmp/qnb0e6/bat`가 각각 exit0이고 verifier stdout JSON과 saved capture가 일치했다. SG/OG 각각 HTTP200 20/20, cs exit0 20/20, completed response60/60, error0이며 세 제품의 returned-file 합계는 각각0이다. SG79-file owned native stored-document scope와 OG79-file served indexed view가 결속됐고 whole indexed-universe attestation은false다. 이 empty NL 결과는 실행 실패나 whole-universe qualification으로 바꾸지 않는다.

## 어떤 파일을 어떻게 수정할지

- 후속 scorer consumer 수리는 [E1-06](O4-E1-06-final-pool-and-scoreboards.md)의 실제 RED/focused67로 검증됐다. 기존 SG scope가 mutable main의 old scorer control hash에 결속돼 fresh native v2는 exit2로 거절됐다. old capture를 현재 scorer에 재사용하거나 control hashes를 덮어쓰지 않는다. clean source `ae8f96b`에서 새12repo SG scope receipt를 발행한 뒤 same Python3.13.9/source의 native3 capture와 independent replay를 새 root에서 수행한다.
- 후속 C3 PREPARE: `/private/tmp/qi-current-nine-pair-exploratory-0e6-input-v1.json` SHA `25f9aad15b4a2e9eab4ed951931a68db8aa408bae7fff12a86cd79355d667035`는 bat409 실제 current admission과 나머지 원본8개의 예정 current0e6 발행 경로를 담는다. 나머지8의 actual issuance 전에는9개 readiness를 뜻하지 않는다. SQLAlchemy/Zellij/Tailscale admission은 null로 유지했다.
- 나머지 actual labels가 주간 모델 한도로 막혔으므로 새 미검수 candidates를 위한 다음 캡처는 기존 pair v7 `--capture-mode exploratory --query-warmup-passes 1 --selection all-twelve`로 준비한다. v7은 canonical admitted bundle을 먼저 재생한 뒤 admission 권위/qualification claim을 제거한 별도 spec와 admitted-lineage를 발행한다. qualified consumer의 unknown-judgment 거절은 유지하고 final labels를 합성하지 않는다. 실제 제품 실행/독립 replay는 아직 NOT_RUN이다.

`OWNED`는 에픽 담당 통합, `SHARED`는 I0 반영, `READ`는 기존 구현 소비다. 재현된 결함이나 채택된 계약 변경이 있을 때만 product source를 수정한다. 구현 파일과 독립 검증 파일을 함께 지정한다.

| 파일 | 함수 / 경계 | 구체적인 변경 또는 검증 | 모드 |
| --- | --- | --- | --- |
| [tools/benchmark/retrieval/live_lexical_external.py](../../../../tools/benchmark/retrieval/live_lexical_external.py) | capture / verify / BoundRelease | current producer로 fresh raw와 transport/status/complete clocks를 발행한다. product execution 성공을 scoring 성공과 구분한다. | OWNED |
| [tools/benchmark/retrieval/semble.py](../../../../tools/benchmark/retrieval/semble.py) | run_adapter / run_completed_worker | 현 resident worker·source/model lock·normalized output producer를 실행한다. | OWNED |
| [tools/benchmark/retrieval/arb_adapter.py](../../../../tools/benchmark/retrieval/arb_adapter.py) | 현재 input adapter | 원문 query/snapshot과 adapted query의 admission을 분리한다. 실제 요청 결함이 재현될 때만 변경한다. | OWNED |
| [tools/benchmark/retrieval/run.py](../../../../tools/benchmark/retrieval/run.py) | cmd_quanta / cmd_pair / run_quality_matrix / cmd_verdict | I0 소유 driver로 Quanta/Semble current-source capture와 replay를 실행한다. | READ |
| [tools/benchmark/retrieval/holdout_review.py](../../../../tools/benchmark/retrieval/holdout_review.py) | capture_review_pool | E1 helper로 product-blind fresh union을 생성해 E1-06에 전달한다. | READ |

## 실행 단계

1. runtime/source/model/license/request/source-scope preflight를 셀마다 확인한다. ready repository부터 serial products를 실행한다.
2. exact1196와 prefix/infix/components, typo4 default/explicit, no-answer, C3 NL240, Gin20, ARB, B09 OSA/CLARC/CSN의 native-mode case series를 개별 실행한다.
3. success/empty/unsupported/cap/partial/error/timeout/missing raw와 process exit를 모두 남기고 native response를 replay한다.
4. canonical source/clock/window/phase validator로 같은 record에 묶인 입력·상태·unit을 검증한다.
5. fresh top-k/source alternative union을 E1-06에 반환한다. 새 qrel revision이 request/pack binding에 영향을 주면 affected cells를 새 root에서 재실행한다.
6. historical reuse와 actual new calls를 final cell inventory에서 구분한다.

## 검증 계획 — NOT_RUN

아래는 실행할 명령/시나리오다. 본 문서에서 통과를 주장하지 않는다. `<...>`와 외부 root는 실행 전에 실제 값으로 확정한다. test filter는 실제 수집 ID를 확인하고 0 tests를 성공으로 표시하지 않는다.

- `uv run --frozen --extra dev python -m pytest tools/ci/tests/test_live_lexical_external.py tools/ci/tests/test_retrieval_capture.py tools/ci/tests/test_lexical_five_product_oracle.py -q`
- `uv run --frozen --extra dev python tools/benchmark/retrieval/run.py quality-matrix --spec <issued-quality-matrix.json>`
- `uv run --frozen --extra dev python tools/benchmark/retrieval/run.py quality-matrix-verify --spec <same-quality-matrix.json>`
- `uv run --frozen --extra dev python tools/benchmark/retrieval/live_lexical_external.py --spec <issued-external-spec.json>`
- `uv run --frozen --extra dev python tools/benchmark/retrieval/live_lexical_external.py --verify <fresh-native-capture-root>`
- 실제 loaded spec의 outside-checkout output·제품·scope를 먼저 검증한다. quality matrix의 Quanta/Semble 실행과 별도 native external collector를 같은 required-cell inventory에서 join한다. 위 명령은 NOT_RUN이다.
- Raw independent replay: wrong request/source/indexed file set, duplicate file ranks, partial underfill, mismatched pack, unsupported normalization을 거절한다.

## 셀별 선택 조건

- 위 선행 결과는 해당 repository/product/capture epoch 범위에 적용한다. 실패한 sibling과 신규 unseen holdout은 ready 셀을 막지 않는다.
- E2-01 completed timer가 미완료이면 품질 raw는 historical transport boundary를 정확히 유지한 diagnostic으로만 발행하고 completed-response speed qualification은 NOT_RUN이다.
- E2-06 실제 parity가 미완료이면 quality spec은 기존 warmup1을 유지한다. warmup0을 선택한 셀은 E2-06 parity·protocol ledger가 선행 결과다.
- name metric·single-RPC·durable batching·token authority를 해당 epoch에 도입했으면 I0에서 그 producer/consumer 및 narrow rails를 먼저 통합한다.

## 완료 조건

- required cells가 모두 terminal outcome으로 설명되고 실제 호출 raw/exit/request/source/unit/clock identity가 replay된다.
- 모든 새 미검수 pair를 E1에 전달하고 scoring 가능한 input의 최종 iteration을 종료한다.

## 중단·거절·재개 조건

- full upstream CoIR/CORE/CSN import나 unseen/human/perf qualification은 이 캡처 자체로 달성하지 않는다.
- 필요한 입력 부재는 `BLOCKED`, 미실행은 `NOT_RUN`, 실제 실행 실패는 `FAILED`로 기록한다. 조건 미성립 `NOT_APPLICABLE`에는 실제 판단 근거가 필요하다.
- 변경이 source/input/query/unit/result에 영향을 주면 [I0 source gate](O4-I0-02-matching-source-proof.md)와 영향받는 capture/report를 다시 판정한다.
- 일회성 raw/log/capture/receipt는 checkout 밖 새 root에 둔다. 기존 외부 terminal을 덮어쓰지 않는다.

## 인계 결과

- 실제 source/dirty ownership, 변경 파일과 계약, 실행한 명령/selector, 관측 결과 및 제외 범위.
- raw/model/runtime/binary/input identity는 해당 실행 계약이 요구하는 범위에서 기록한다.
- 완료 조건별 `VERIFIED`/`FAILED`/`BLOCKED`/`NOT_RUN`/`NOT_APPLICABLE`과 후속 티켓에 넘길 입력을 발행한다.
