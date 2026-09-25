# SEP-26 Retrieval Remediation — 작업 티켓

작성일: 2026-09-26. 상태: **RBR-00 일부·RBR-11 구현 완료(2026-09-26 2차), 나머지 구현 NOT_RUN**.

최종 감사 기준: `33b24dd5df959f38c0df4717ff834b96750faf34` + 기존 dirty 변경. 시작 기준은 `e38e07865daf19661deaa5d1e580acc5814504ef`였으며, 공유 main 커밋 후 검사 입력 57개의 해시를 재대조하고 집중 테스트를 재실행했다. 이 패킷은 구현 완료나 비교 우위의 증거가 아니다. 최종 재감사 근거는 [AUDIT.md](AUDIT.md), 실제 관측값·파일 해시는 [audit-evidence.json](audit-evidence.json), 공통 완료 계약은 [TEST-PLAN.md](TEST-PLAN.md)에 있다.

기존 [RB-00~RB-06](../../sep-23-retrieval-bench/tickets/INDEX.md)의 fail-closed 계약은 유지한다. 이 패킷은 후속 개선의 현재 작업 순서다. 과거 캡처·verdict는 수정하지 않는다.

## 범위와 결정

- 즉시 구현: 입력 정책, 실제 lane/응답 진단, 비교 profile, 심볼 연결, 검증 inventory, resource sampler 수정.
- 필수 실험: 청크/평가 span 분해, encoder parity, exact-vs-ANN, query/ingest 단계 비용. 결과가 나와야 알고리즘 변경을 선택한다.
- 조건부 구현: exact-name ranking, fetch 정책, ingest delete 최적화. 측정 없이 새 기본값·모델·랭커를 지정하지 않는다.
- 기존 200개 질의는 development 데이터다. holdout 정의·분리는 튜닝 **전에** 고정한다. 조건부 티켓의 후보는 development에서 선택하고, 함께 고정한 한 후보 묶음을 최종 holdout에서 한 번 평가한다.
- 수동 라이선스 승인·독립 심사자 섭외·운영 호스트 확보는 개발 작업 티켓에 넣지 않는다. 해당 입력이 없는 경우 개발/진단 작업은 진행하되 qualified quality/performance 주장은 내지 않는다.
- 코퍼스, 모델, 대용량 trace, 실행 산출물은 레포 외부. 이 디렉터리에는 티켓과 작은 감사 근거만 둔다.

## 티켓과 의존성

| 티켓 | 우선순위 / 성격 | 선행조건 | 산출물 |
| --- | --- | --- | --- |
| [RBR-00](RBR-00-proof-contract.md) | P0 / 확정 수정 | 없음 | inventory 일치, 새 계약의 source closure, [profile provenance](PROFILE-CONTRACT.md) |
| [RBR-01](RBR-01-diagnostics.md) | P0 / 확정 구현 | RBR-00 계약 합의 | response/trace 및 stage timing 관측 |
| [RBR-02](RBR-02-query-policy.md) | P0 / 확정 구현 | RBR-00 | native/literal/NL 질의 정책 |
| [RBR-03](RBR-03-semble-profiles.md) | P0 / 확정 구현 | RBR-00 | native-default / controlled 비교 분리 |
| [RBR-04](RBR-04-symbol-producer.md) | P1 / 기능 연결 | RBR-00 | source-bound 다언어 SymbolRecord + combined scope |
| [RBR-05](RBR-05-symbol-route-proof.md) | P1 / 기능 연결 | RBR-01/02/04 | symbol route + 공통 결과 증명 |
| [RBR-06](RBR-06-span-chunking.md) | P1 / 계측·실험 | RBR-01/02/03 | rank/context 분리, 기존 청커 대조 |
| [RBR-07](RBR-07-semantic-parity.md) | P1 / 원인 실험 | RBR-01/03 | full-vector parity + exact/ANN 분해 |
| [RBR-08](RBR-08-symbol-ranking.md) | P2 / 조건부 변경 | RBR-05/06 | ranking 변경 또는 근거 있는 유지 결정 |
| [RBR-09](RBR-09-query-performance.md) | P2 / 조건부 변경 | RBR-01/02/03/07 | fetch/ANN 비용-품질 frontier |
| [RBR-10](RBR-10-ingest-performance.md) | P2 / 조건부 변경 | RBR-01 | delete/append 비용 분해와 안전한 최적화 |
| [RBR-11](RBR-11-resource-accounting.md) | P0 / 재현된 결함 | 없음; inventory는 RBR-00과 통합 | live zero-RSS parent를 통한 descendant 보존 |
| [RBR-12](RBR-12-evaluation-closeout.md) | P1 / 검증 통합 | 준비는 즉시; 최종 평가는 적용 티켓 종료 후 | frozen holdout, fresh receipts, 실제 pair/replay |

## 실행 순서와 중단 기준

1. RBR-00·11 수정, RBR-12의 평가 계획/분리 고정. 이어 RBR-01·02·03 구현.
2. 같은 외부 코퍼스·같은 원문 질의로 새 development baseline 확보. native-default 비교와 통제 실험은 별도 spec/output으로 실행.
3. RBR-04→05 연결. RBR-06/07 실험 및 RBR-09/10 계측. 한 실험에서 모델·청커·질의·랭커를 동시에 바꾸지 않는다.
4. 관측된 원인만 RBR-08/09/10의 실험 profile에 반영한다. development에서 후보 하나를 고정하고, 제품 기본값 승격은 RBR-12의 단일 최종 평가 후 판정한다. 증거가 부족하면 기존 기본값을 유지한다.
5. 계약 문서·코드·테스트를 고정한 뒤 RBR-12 proof와 단일 holdout pair를 수행한다. 동시 변경 조합은 묶음으로 채택하거나 유지 결정한다. 최종 상태·결과표는 레포 외부 증거에 발행한다. source-bound 문서를 결과 확인 후 수정한다면 proof와 pair를 새 revision에서 다시 발급한다.

공유 main의 dirty 편집은 허용한다. 기존 변경을 reset/stash하지 않는다. `sdk.rs`, `main.rs`, `diagnostics.rs`, `record.rs`, `run.py`, schemas, proof inventory는 티켓 간 공용 파일이다. **한 번에 한 통합 담당만 같은 파일을 수정**하고, 티켓별 작업 결과를 직렬 통합한다. 이 계획은 별도 에이전트 실행을 지시하지 않는다.

Rust-heavy 검증과 성능 측정은 경쟁 writer/build가 없는 구간에 수행한다. 구현 중 dirty 결과는 진단으로 기록하고, 현행 clean-source proof 요구를 우회하지 않는다.

## 완료 표기

각 티켓은 구현, focused verification, integration, qualification을 별도 상태로 갱신한다. 상태 값은 `VERIFIED / FAILED / BLOCKED / NOT_RUN / NOT_APPLICABLE`이다. 조건부 티켓은 실험 근거와 유지 결정이 검증되면 종료할 수 있으나, 실행하지 않은 최적화를 완료라고 쓰지 않는다.

### 2026-09-26 2차 라운드 상태

| 티켓 | 구현 | focused verification | integration | qualification |
| --- | --- | --- | --- | --- |
| RBR-00 | 진행 중: inventory 일치(python 218/rust 68/sdk 12, 역할별 verify 통과), sep-26 closure 등록+거부 테스트, [PROFILE-CONTRACT](PROFILE-CONTRACT.md) 정의. schema/validator 배선은 RBR-02+와 함께 | `VERIFIED` — 3역할 inventory verify, receipt closure 24 passed | `VERIFIED` — clean-source closure receipt `2146054505486b0ab37b7f7eb88dc7546273f115134d6dc0935384b12f679b60` @ `b4e21b50` (818 files, capture+verify exit 0). 이후 변경분은 커밋마다 재발급 | `NOT_APPLICABLE` |
| RBR-11 | `VERIFIED` — 소유 그래프에서 live zero-RSS 연결 노드 보존, resource policy는 소유 집합 확정 후 적용. zombie 회귀 기대치 수정 + 반례 7종 추가 | `VERIFIED` — 감사 oracle `[100,105]` 해소, sampler 8+실제 프로세스 smoke 1 passed, 풀 파일 218 passed | `VERIFIED` — RBR-00 동일 receipt @ `b4e21b50` 커버 | 실제 프로세스 smoke `VERIFIED`(macOS) |
| RBR-02 | `VERIFIED` — `query_plan.rs` 정책 엔진(native/literal/NL 토큰 OR, typed refusal, 4중 identity SHA), `RouteQuery` planned 입력, 작업당 1회 계획 공유, `--query-input-policy`. **v4 record 수직 완성**: runner.schema v4(query_input_policy+query_identity), Rust record·Semble 어댑터·merge 생산 v4, evaluator/run.py v3(역사)/v4 분기 + Python 독립 재도출 oracle(`query_plan.py`), tamper 거부 10종. 잔여: 실 daemon 문장/식별자 distractor fixture(sdk rail) | `VERIFIED` — query_plan 13 + Rust lib 48 + chunking 20, Python 290 passed(신규 v4/replay/tamper 포함); inventory python 229·rust 68·sdk 12 | `NOT_RUN` — 커밋 후 receipt | `NOT_APPLICABLE` |

RBR-01~10, RBR-12: `NOT_RUN`. 위 표의 검증은 dirty 작업 트리에서 수행한 진단 실행이며, TEST-PLAN §3의 clean-source proof rail은 커밋 후 별도 발급한다.
