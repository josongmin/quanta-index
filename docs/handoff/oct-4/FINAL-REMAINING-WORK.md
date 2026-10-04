# 2026-10-04 통합 잔여 작업

입력: 같은 디렉터리의 `agent-1.md`부터 `agent-5.md`까지 5개 핸드오프. 이 문서는 중복 작업과 과거의 완료된 수정을 제거한 실행 목록이다. 제품 품질·성능·배포 적격성 영수증은 아니다. 각 작업 착수 시 [B07](../../plans/sep-30-code-search-benchmark-trust/tickets/S30-B07-performance-and-indexing.md), [B08](../../plans/sep-30-code-search-benchmark-trust/tickets/S30-B08-fresh-multirepo-holdout.md), [B09](../../plans/sep-30-code-search-benchmark-trust/tickets/S30-B09-external-robustness-adoption.md)의 현재 상태와 원본 실행 terminal을 다시 읽는다.

## 0. 먼저 고정할 경계

- 현재 확인한 `main` HEAD는 `2062fed3ff86287a0914ece99dc2fe0af829c29b`다. 81개 경로가 staged다. `origin/main...HEAD`의 로컬 ref 비교는 `0 2`이며 원격 fetch 결과가 아니다. HEAD만 실행 소스로 표시하거나 공유 index 전체를 한 작업자의 변경으로 취급하지 않는다.
- staged `git diff --cached --check`는 `vendor/tree-sitter-javascript/quanta-compatibility.patch:5`의 trailing whitespace에서 실패한다. patch context와 provenance를 보존하면서 원본 소유자가 수정하고 diff를 재검증해야 한다.
- 각 hunk의 소유권, source/lockfile/proof registry의 단일 통합 담당, 실제 실행 중인 driver를 확인한다. 핸드오프에 기록된 pair controller PID 32335와 collector PID 88605는 이번 재조회에서 존재하지 않았다. 원본 failed root를 재사용·덮어쓰기 전에 terminal과 새 namespace의 중복 여부를 확인한다.
- 공유 변경을 소유 범위별로 통합하고 영향받는 좁은 rail을 실행한 뒤 실행 source/binary/input/query/qrel/unit/profile을 고정한다. 기존 clean export를 `main`에 복사하거나 과거 receipt를 새 source의 증거로 승격하지 않는다. 일회성 실행 산출물은 checkout 밖 새 root에 둔다.

## 1. 벤치 결과를 닫는 필수 경로

| 순서 | 실제 남은 일과 책임 경계 | 완료 조건 |
| --- | --- | --- |
| 1. 검수 실행 | [B08]의 실패한 원본 5저장소를 로그·유효 cache·source별로 복구한다. TypeORM/Tailscale의 unresolved 판단을 재검수하고 Django/SQLAlchemy/Zellij 실패 원인을 각각 확인한다. 첫 6저장소의 supplemental union과 이후 저장소별 union을 실제 독립 AI reviewer 2개 및 조정자로 판단한다. 현재 source의 `bind_supplemental_review_tasks`와 `iter_repository_admissions`를 사용하고, 외부 driver가 그 API를 실제 `--run` 경로에서 쓰는지 확인한다. | 원본 terminal의 7저장소×20 = 140/240 발행과 첫 6저장소 미검수 375쌍은 **잔여 산정의 출발점**이다. 214쌍/14 payload preflight는 모델 호출 0회였으므로 완료로 세지 않는다. raw 응답, 모델 identity, source/query/rubric/threshold와 unresolved·제외 집합이 재생 가능해야 한다. AI provenance를 human으로 표시하지 않는다. |
| 2. 하나의 라벨·admission 권위 | 기존 evaluator, `holdout_review`, suite/pack/manifest/receipt 경로로 검수 결과와 objective/no-answer strata를 합친다. 저장소별 ready/failed/pending을 terminal과 대조하고 ready 저장소부터 실행한다. 과거 C5의 stale mechanical suite/exclusion 4개가 최종 B08 cohort에 필요하면 해당 소스에서 새 입력으로 발행한다. 후속 B09 global12 실행과 혼동하지 않는다. | 최종 qrels, source-bound suite, blind pack, family split 및 admission이 서로 일치한다. 누락 threshold, source 변화, duplicate pair, 미검수 grade 주입, 옛 record의 새 qrel 재결속을 거절한다. NL-only 진단을 mixed-track 제품 결정으로 표시하지 않는다. |
| 3. 독립 정답과 평가 단위 | exact same-name의 모든 선언 파일, 대표 파일, ambiguity, no-answer, test/generated/사용처를 독립적으로 판정한다. `unit_id`와 실제 declaration-name span을 source에 결속하는 별도 평가를 기존 scorer에 추가한다. 튜닝에 노출되지 않은 repository/query-family holdout을 사전에 고정한다. | file Hit/MRR/NDCG와 정확한 name-span/ID 회수를 별도로 출력한다. context 확대·같은 줄의 다른 선언·사용처만 맞은 경우를 negative로 거절한다. Gin/기존 global12를 새 unseen holdout으로 재명명하지 않는다. |
| 4. 외부 색인 범위 | Sourcegraph/OpenGrok의 해당 **실행 universe** 전체 indexed path와 native stored content/posting 또는 source reference를 입증한다. 새 캡처에는 가능한 전후 receipt를 결속한다. | B08 C3의 13,347파일 증거를 B09 11,695파일에 재사용하지 않는다. 사후 조사만 있으면 `after_only`로 기록한다. 필요한 native inventory가 없으면 해당 비교는 `BLOCKED` 또는 diagnostic으로 남긴다. |
| 5. 다섯 제품 실제 실행 | final frozen source/input에서 cohort×repository×product×profile×unit의 required-cell inventory를 먼저 발행한다. Quanta, Semble, Sourcegraph, cs, OpenGrok을 동일 준비 입력에서 실행하고 기존 정상 raw는 영향 분석 후에만 재사용한다. 외부 producer의 요청 생성→정규화된 응답 완료 timer를 기존 transport timer와 구별해 구현·검증한다. | exact Gin **1,196**, prefix, infix, components, 네 typo edit, no-answer, C3 NL 240, Gin 공식 20, ARB original/adapted, B09 OSA/CLARC/CSN을 별도 분모·정책으로 보고한다. 원본 HTTP/process bytes, request, status, rank unit, cap/partial/error/unsupported/missing, indexed universe를 재생한다. 300-query 과거 RCA나 21,815행 historical join을 최신 5제품 실행으로 세지 않는다. |
| 6. 재채점·보고 | 기존 evaluator/reporter로 최종 qrels와 native rows를 replay한다. 공통 eligible task ID에서 조건부 품질과 운영 coverage를 함께 계산하고 repository cluster 불확실성·pool exposure 민감도를 제시한다. | 각 scoreboard의 결과·분모·제외 사유가 raw에서 재계산된다. 미검수/unknown을 0점이나 no-answer로 바꾸지 않는다. 파일 점수, 선언 span, 기본 literal-first, 명시적 OSA1 및 NL/semantic을 합쳐 단일 순위를 만들지 않는다. |

현재 확인한 원본 terminal: `c3-review-resume-quota-qcshswey/terminal.json`, `c3-supplemental-queue-163-w6lwIs1x/terminal.json`, `c3-all-five-pool-audit-gsZWij9u/remaining-terminal.json`은 모두 `FAILED`다. 따라서 위 1–6의 최종 적격 비교는 `NOT_RUN`이다.

## 2. 엔진 정확성·운영 경계

1. **Active 선택과 read-view 획득을 한 권위로 묶기.** 먼저 G1 선택→G2 활성화→G3 retention→G1 view 획득 interleaving을 결정적으로 재현한다. 실제 거절/잘못된 선택이 확인되면 catalog/retention admission pin을 view로 이전하는 짧은 원자적 선택 경계를 설계한다. SDK의 사전 `ResolveActiveGeneration` RPC 제거도 같은 generation/token/ABA/response-binding 계약 안에서 다룬다. 단순히 pre-resolve를 삭제하거나 server-side `Active`로 치환하지 않는다. tokened/explicit pin refusal, cursor, GC, cancel, A→B→A와 실제 daemon 경로를 독립 oracle로 확인한다. 현재 combined race와 단일 RPC 수정은 `NOT_RUN`; 정적 위험과 두 RPC 관측만 있다. [설계 경계](../../adr/OCT-04-001-search-corpus-selection-and-ingest-pressure.md), [P04/P09 실행 계획](../../plans/sep-21-search-plane-sota-hardening/tickets/FINAL-RESIDUAL-EXECUTION-PLAN.md).
2. **Maintenance와 timeout의 실제 실패 경계 검증.** 디스크 full-tree walk를 3 cadence 이상 지연시켜 backend freshness/readiness 영향을 확인하고, 필요한 경우 health와 paced disk metering을 분리한다. admitted slow publish 도중 30초 client timeout 이후 operation identity 확인·정확한 replay를 검증한다. logical bytes, allocated space, physical write I/O, transient merge high water를 구분한다. serial ingest slot을 늘리거나 async ACK를 도입하는 것은 측정된 contention과 durable source custody가 있을 때만 결정한다. 현재 `NOT_RUN`.
3. **정책 결정은 잔여 실패의 원인별로 한다.** B09 default OSA의 23개 `ordinary` 잔여는 literal-first 정책에서 교정이 억제된 사례다. explicit OSA1 4363/4363을 default 성공으로 전용하지 않는다. default 변경이 필요하면 ambiguity/no-answer/critical-stratum 독립 fixture와 미사용 holdout으로 결정한다. Gin exact 잔여 4개는 별도 symbol control에서 선언 존재가 확인됐지만 file@10 해결이나 정확한 span 증거는 아니다. NL relevance와 semantic 후보 생성을 독립 qrel·source·model·ranking unit으로 진단한 후에만 fusion/model/chunking을 바꾼다.

## 3. 성능 작업의 순서와 조건부 수정

1. **먼저 같은 경계를 측정한다.** 외부 HTTP/process completed-response timer, Semble process 내부의 약 6초 귀속 공백, query/full·delta·delete·no-op·reopen 인덱싱 단계, CPU/RSS/disk high water를 기존 계측 경로에 연결한다. quality-only의 `query_warmup_passes=0`은 기존 API로 0/1 parity와 speed-mode refusal을 확인한 뒤 사용한다. 현재 Quanta SDK, Semble worker BM25, 외부 transport timer의 합계를 속도 순위로 비교하지 않는다.
2. **지속 비용이 확인된 뒤 저장 방식을 바꾼다.** 182개 artifact의 격리 `F_FULLFSYNC` 실험은 file sync 182회 유지, directory sync 182→4회에서 평균 1.845→0.919초를 보였다. 엔진 개선이나 crash safety 증거는 아니다. 실제 generation writer에서 content sync/rename→group directory barrier→root/manifest 공개 순서를 하나의 canonical durable-write authority로 구현한다면, 각 실패 지점의 fault injection, old/new root reopen, fresh/delta/delete/no-op 동등성, activate refusal을 선행해야 한다. source/coverage pack은 batch barrier 뒤 file sync가 여전히 지배할 때만 별도 결정한다.
3. **ASCII/typo 최적화는 전체 호출로 판정한다.** 현재 scanner A/B는 결과 동등성을 보였으나 deletion 전체 호출 평균은 +8.75%였고 host가 혼잡했다. 동일 source 차이와 허용된 host에서 재측정해 유지/수정/철회한다. 그 뒤에도 token scan이 병목이면 기존 gram/cache 위에 중복 계층을 올리지 말고 source·generation-bound distinct token/posting authority의 구축·메모리·delta/delete·cold-open 비용과 OSA1 completeness를 함께 증명한다.
4. **evaluator cold bootstrap은 필요할 때 기존 구현을 교체한다.** bounded numeric cache는 이미 있다. 최초 cold 계산이 계속 병목이면 fixed draw-index golden과 독립 scalar/statistics reference를 둔 bounded vectorization을 한다. RNG/CI 계약이 바뀌면 방법 버전을 명시하고 과거 byte parity를 주장하지 않는다.
5. **마지막에 release 반복 측정과 scale.** owner-local correctness 뒤 동일 release binary/input, 동일 completed-response 경계, 직렬 실행, 호스트 지속 admission, fresh roots, 사전 효과·불확실성 기준을 쓴다. 256→4096→32768 규모와 full/delta/delete/reopen 및 OS restart를 구별한다. 이전 4096 timeout, 32768 posting-cap refusal, busy-host A/B를 성공 speed/scale로 세지 않는다. 현재 정식 B07·새 scale은 `NOT_RUN`.

## 4. 별도 release 게이트

제품 결정과 성능 평가가 끝나도 [SEP-21 residual plan](../../plans/sep-21-search-plane-sota-hardening/tickets/FINAL-RESIDUAL-EXECUTION-PLAN.md)의 raw proof-result authority, P03–P08 독립 반례, P09 process truth, P10 restore, P11 paired producer 및 Linux release 실행, 배포/활성화/롤백 영수증은 별개다. 정확한 최종 source pair와 hosted CI를 재발행하기 전 `CODE_QUALIFIED`, `DEPLOYED`, `ACTIVATED`, `ROLLBACK_PROVEN`으로 표시하지 않는다. 설정 정책과 source-preparation SDK의 두 ADR은 `Proposed`다. 실제 operator/producer 요구가 확인되기 전에는 필수 구현 목록에 넣지 않는다.

## 다시 구현하지 않을 것

- supplemental threshold binding과 ready/failed admission drain, B09 fresh-join runtime pins·loaded-grammar provenance·Unicode 17 lowering, scored NL OR, source-attested typo ranking/cursor, JS ABI15 grammar는 현재 소스에 있다. 그 producer/consumer 연결 및 영향 테스트만 다시 확인한다. B08 티켓의 옛 runtime-pin 미수정 문장은 현재 소스와 불일치한다.
- 기존 evaluator/planner/IR, gram shortlist/OSA cache, touched-shard reuse, completed-response Quanta/Semble clock, scale/open-loop harness를 재사용한다. 별도 scorer·IR 버전 쌍·하네스, 전체 CoIR/CORE 수입, 새 Semantica API는 위 실패를 해결하는 근거가 없다.
- 과거 300-query semantic과 1,196-query lexical, B08 13,347파일과 B09 11,695파일, old/new engine capture, file/chunk/symbol unit은 결합하지 않는다.

## 이번 통합의 검증 범위

- `VERIFIED`: 5개 핸드오프, 관련 현재 티켓·선택된 소스, `git status`/HEAD/로컬 ref, staged whitespace check, 위 3개 원본 terminal, 기록된 두 PID 부재를 읽기 전용으로 확인했다.
- `FAILED`: 현재 `git diff --cached --check`의 vendor patch whitespace.
- `NOT_RUN`: 이 문서 작성 중 Rust/Python tests, 새 AI 검수·제품 호출·대규모 캡처, 정식 성능/scale, CI/release/deployment. 과거 handoff의 좁은 통과 숫자를 현재 overlay 전체의 검증으로 합산하지 않았다.
