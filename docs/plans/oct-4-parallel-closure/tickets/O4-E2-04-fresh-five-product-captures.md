# O4-E2-04 — 5제품 실제 캡처와 blind union 반환

| 항목 | 값 |
| --- | --- |
| 에픽 / 담당 | [E2 — 외부 제품 native 범위·응답 경계·실제 캡처](../epics/E2-external-capture-and-timing.md) / E2 담당 |
| 우선순위 / 종류 | P1 / `EXECUTION` |
| 기준 웨이브 | [W4 — 실제 캡처·성능·scale](../waves/W4-native-capture-performance-and-scale.md) |
| 실행 상태 | source97 focused203·SG12/13,347files·bat native3 actual/replay·retained full5-product join `VERIFIED`. Q/S8 producer/독립 replay 완료. required8 native v4 guard 실패 보존; v5 actual capture8/8 성공·독립 replay8/8·full joins8/8 VERIFIED. 새 union151tasks/742unjudged pairs PREPARED. final quality/speed 및 나머지 lanes 미완료 |
| 선행 결과 | [O4-E1-03](O4-E1-03-admission-and-split.md), [O4-E2-02](O4-E2-02-external-index-universe.md), [O4-E2-03](O4-E2-03-required-cells-and-scheduling.md), [O4-I0-02](O4-I0-02-matching-source-proof.md) |

[전체 지도](../README.md) · [티켓 인덱스](INDEX.md)

## 2026-10-05 source97 canonical 실행 결과와 required8 복구

- clean `97eedd11b70e76c66985b15a968211a2faf92c6d`의 JSON 타입 결속 수리 후 focused 회귀는 exit0/203passed·0failed/skipped다. JUnit의 selected203과 relocated Contract/SDK2cases를 별도로 확인했다. SG12 producer는 exit0/2726.705s, 독립 replay는 exit0/1227.758s이며12repo/13,347files가 `VERIFIED`다. replay SHA는 `02d8272bc73ef70bc9fbf3dfbd81a9586afe61a0ec8c346a951922e733ea3a45`다.
- bat native actual `SUCCEEDED`: `python -m tools.benchmark.retrieval.live_lexical_external --spec /private/tmp/qi-native-bat-canonical-json-97eedd-v5-spec.json` — exit0/351.098s. 별도 `python -m tools.benchmark.retrieval.live_lexical_external --verify /private/tmp/qnbj97/bat` — exit0/170.034s/`VERIFIED`. producer·saved capture·독립 replay의 canonical bytes가 동일하다. replay SHA는 `7dd69bb9d093cac0c601836a070f06989b885b9f546943199fc4780f8a16fd97`다.
- raw3×20의 exact task inventory·completed-response5-field object(boundary/clock/duration/output size/hash)를 확인했다. SG/OG HTTP200·error=null,cs exit0이며 각 제품 returned files는0이다. 초기 root readback의 bool 가정은 실제 object schema로 교정했다. SG79 native files·OG79 served-view files가 결속됐지만 OG whole-index qualification은false다. complete-empty를 missing/unsupported/failure로 바꾸지 않는다.
- bat full5-product join `VERIFIED`: `python -m tools.benchmark.retrieval.lexical_file_comparison --external-spec /private/tmp/qi-bat-five-product-canonical-json-97eedd-v6-join-spec.json --out /private/tmp/qi-canonical-json-97eedd-serial-v3/bat-five-product.json` — exit0/1095.681s. 결과 SHA `eac4755c54da23e6fbe64c5b2119f20ec1fbfa673a1a91aee92fe490a920989f`. source bundle·canonical raw/verdict/proofs 및 새 native raw를 재생했다. 공통 eligible20, five-product 각20관측, distinct-file 단위, cross-unit 비교 금지다. Quanta/Semble file NDCG@10은 각각0.6068563832657718/0.6143293951848605, SG/OG/cs는0이다. 기존 AI 판단 기반 `diagnostic_unqualified`; independent gold·native-rank equivalence·whole indexed-universe·qualified latency는 제외한다. 기존 source0e6 Q/S raw의 retained replay이며 Q/S를 새97 binary 실행으로 재명명하지 않는다.
- 중앙 `/private/tmp/qi-canonical-json-97eedd-serial-v3.py`(SHA `2f9a033416e7072058fa7bffee85c89d8c840afb102dc916f2a383877b803cd3`)는6phase 모두 요구 terminal로 종료했고 summary `VERIFIED`/qualified=false다. 과거 join v4 실패와 old source namespaces는 보존했다.
- pure all12 PREPARE는 exit2로 종료했다. `/private/tmp/qpc9p/prepare.json` SHA `cff64978878987d95468e2209ab048bc31a50e9fbc72b59f060a12210df0d946`, `aggregate.json` SHA `cd08861444e8a7965049dbfd7b5b17fffd71f916135679e1140cfd8ad156924d`다. source0e6·exploratory·warmup1에서 bat/cli/lo/mocha/uvicorn/zustand/django/typeorm/nushell9개 `PREPARED`; SQLAlchemy/Tailscale/Zellij3개 admission 부재 `BLOCKED`. 이 준비 terminal을 actual capture나12개 완료로 합산하지 않는다.
- required8 중앙 queue `/private/tmp/qi-current-eight-serial-97eedd-v1.py`(SHA `7bd9d46438c1a6be03ee9e01124da44750e116785bbadfd57fd50a842bd5686e`)는 시작했다. native bind는 exit0이지만 v4 native controller가 original review validator revision을 current product0e6과 동일하다고 잘못 요구해8개 모두 호출 전 `BLOCKED`로 종료했다. capture/replay phase는 exit2/`FAILED`, 원본8행 ledger를 보존한다. 독립 Q/S8 producer/독립 replay는 각각 exit0으로 완료했다. 각40 selected/executed/passed·failed0, 합계320 관측이다.8개 모두 CONTRACT_GREEN/PAIR_VALID/SDK_PATH_GREEN=pass, PERF_QUALIFIED/QUALITY_DELTA=not_applicable다. producer ledger SHA `0ad309bca6ad9538a67153316d7ab23e4012848da2741c54a88621acf73baabe`, 독립 replay ledger SHA `d1237af18a608877cdf4c2e5b66478035748ecd3019776a8c26d998e91d54cca`를 root가 별도로 읽었다. canonical `producer_execution.execute`의 바깥 phase deadline/process-group custody는 유지한다.
- 복구 PREPARE v5는 `/private/tmp/qi-current-native-eight-97eedd-prepare-v5`다. controller SHA `90941aa0783d2953cde19ccc523b92df278428e0f8bc158684d09ee897ac2ef1`, sealed packet SHA `361270f56159e0539f560969d69f8dd14248e360beef628afa7fa1561d172a0f`. original review validator의369/499/904 source identity를 그대로 보존하고, current product authority는 canonical E1 issuance의12keys/16input hashes/source0e6으로 결속한다. suite/pack/admission digest와16개 inputs를 전후 확인한다. 읽기 전용 입력 preflight8/8은 완료했다. 후속 v5 actual capture는8/8 모두 `SUCCEEDED`, phase exit0/1082.195s다. `/private/tmp/qi-native-eight-97eedd-capture-v2/ledger.json` SHA `11c0ae21f0d40fbe2680fcd8e0b17e0a935f3357239bd70868fb1b952b9b995f`, final guard error=null·full_eight_complete=true·qualified_comparison=false다. 독립 replay도 phase exit0/972.520s,8/8 모두 `VERIFIED`로 종료했다. replay ledger SHA `da189aafac32d637f464bb93e85c741507ae54906310d7e3e8502ae9ed683f4b`, final guard error=null·full_eight_complete=true·qualified_comparison=false를 root가 별도로 확인했다. fresh output은 `/private/tmp/qnc97v5/<repo>`다. 복구 중앙 queue `/private/tmp/qi-current-eight-serial-97eedd-v2.py`(SHA `79b6ba040cd9e466f7a664b99b5a408741ccff8f4de5a0afc733dc0435da288b`)를 시작했으며 기존 v1의 pair producer/독립 replay 종료 후 실제 native v5 capture를 시작했다. independent native replay8/8과 full5 join8/8은 모두 완료됐다. join phase exit0/3764.832s, `/private/tmp/qi-current-eight-five-product-join-97eedd-v2/ledger.json` SHA `7942ef4ea846f28161ba2e6a94cc84cba3fb08e9c867c44a1deff3535f9c3d1b`, final guard error=null·full_eight_joins_verified=true·qualified=false다. 중앙3phase summary `VERIFIED` SHA `189e0aaafb962dfc0cce64a16a24be65ba57d4f90d0534552aae5f770c6f843a`를 root가 별도 확인했다. native capture→독립 replay→full5 join을 직렬 실행하고 v1 실패 ledger는 보존한다.
- remaining8은 cli/lo/mocha/uvicorn/zustand/django/typeorm/nushell이다. v5 출력에 결속한 full-join PREPARE는 `/private/tmp/qi-current-eight-five-product-join-97eedd-prepare-v2`로 packet SHA `6d96d95398d0d0a7cae4ec1f5ce340176171fb76a1c5a436792e3a97aa44fd62`, controller SHA `1681a58301f08b0ca9295e74a100a3c212341decc6ba63855ed55aae8397b075`다. full join은 각각의 current pair/native 독립 replay 이후에 실행한다. source-bundle metadata/stat상8개 모두256MiB 미만이며 Django236,027,995bytes다. shallow/missing-parent source나 cap 완화는 사용하지 않는다. final blind union·새 후보 실제 검수·human/final quality/speed/Linux qualification은 별도 잔여다.

## 2026-10-05 canonical consumer628541 PREPARE

- 추가 derived provenance·frozen-context 경로 수리를 clean `628541e150192b4aaf0ff4ba54566ae28c6ca25f`/`/Users/songmin/.codex/worktrees/oct5-canonical-consumer/quanta-index`에 고정했다. source0e6 Rust product bytes는 그대로이며 새 Python canonical owner의 focused 회귀는 직렬 admission 종료 뒤 실행한다. 지금 `NOT_RUN`이다.
- 새 SG batch `/private/tmp/qi-sg-canonical-consumer-628541-20261005-v1-batch.json` SHA `b94aad19ac82a37c257b496095cedcad016ba44105aa08aef3e23ad9e108bdde`, bat native3 spec `/private/tmp/qi-native-bat-consumer-628541-v4-spec.json` SHA `b9aeabf0cf88e509d056299fc9d754e7621d518f43484ef47eb2ca47722293c0`, bat join v5 spec SHA `2b3c95e41e043c276e7b30ed4c25a0f7c58794cc1f30fcc84cb0dc21839fbca3`를 fresh namespaces로 준비했다. 실제 SG/native/join은 `NOT_RUN`이며 ae8f의 완료 raw/receipt를 수정하지 않았다.
- 원본8 external-native PREPARE `/private/tmp/qi-current-native-eight-628541-prepare-v2`는 controller SHA `90151a760f525a17bdbcfa00b897e695d6b233ddf40c1dc5c8173fc494191148`, packet SHA `d6371897638f2eb089c30fef3cb93cdd29acb1f51271a009cb4cc6d6249ac262`다. AST·exact source/spec/source-helper binding을 확인했고, 마지막 source guard 실패도 aggregate ledger에 남기며 전체 성공을 거절하도록 보완했다. packet unsealed·SG receipt hashes null·issuance terminal 미완료이므로 READY/capture/replay 결과가 아니다. 실제 issuance 결과 및 새 SG 독립 replay 후 새 bound packet을 발행한다.

## required8 full joins 실제 분모 — 2026-10-05

| 저장소 | full join wall s | 공통 eligible / 요청20 | Quanta eligible | Semble eligible |
| --- | ---: | ---: | ---: | ---: |
| cli | 559.495 | 0 | 1 | 0 |
| lo | 469.684 | 5 | 5 | 20 |
| mocha | 454.317 | 0 | 3 | 1 |
| uvicorn | 436.734 | 1 | 3 | 3 |
| zustand | 439.149 | 2 | 6 | 2 |
| django | 485.893 | 0 | 0 | 0 |
| typeorm | 488.323 | 1 | 2 | 2 |
| nushell | 431.007 | 0 | 0 | 1 |

- 모든 report는 각5제품의20관측을 보존했고 결과 SHA를 ledger와 대조했다. SG/OG/cs는각20 eligible이며480 native rows의 exact task IDs가 suite와 일치했다. OpenGrok의 nushell.nl.19만1파일을 반환했고 나머지는 complete-empty다. native raw completion은 query 결과의 relevance를 뜻하지 않는다.
- Q/S 반환 후보의 미판정 pair 때문에 eligible 분모가 줄었다. cap 상태를 failure로 바꾸거나 미판정 파일을 grade0으로 채우지 않는다. common0 report는 채점 가능한 공통 질의가 없는 diagnostic artifact이며0점 비교/성능·품질 우열을 발행하지 않는다.
- current9 blind supplemental adapter actual exit0,9/9 `PREPARED`: `/private/tmp/qi-current-nine-supplemental-pool-97eedd-actual-v1/ledger.json` SHA `78013ed33e5647bfa5e109fc5edabb422e41dc48d0cbf4f63f713785642818ec`. bat의 신규 미판정0, 나머지8의151tasks/742pairs가 남았다. 실제 판단·새 admission·final scoreboard는 [E1-02](O4-E1-02-supplemental-labels.md)의 별도 범위다.
- 이 결과는 frozen source97의 실제 epoch다. 후속 native metadata 타입 수리의 focused RED/GREEN은 별도 source/owned overlay이며 과거 capture/control hashes를 새 hash로 덮어쓰지 않는다. 해당 producer 변경 후의 fresh native evidence는 영향 gate에서 따로 발행한다.

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
