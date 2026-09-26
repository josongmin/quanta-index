# 최종 코드 감사 — SEP-26 Retrieval Remediation

> Archive status: `Historical audit ledger`. Initial observations are preserved here. Current decisions are in the [SEP-26 ADR set](../../../adr/README.md); current unfinished work is in [GAP-REGISTER.md](GAP-REGISTER.md).


> 아래는 티켓 작성 당시의 역사적 감사다. `8d9b9f37` 이후를 포함한 티켓별 현재 상태와 남은 게이트는 [CURRENT-AUDIT.md](CURRENT-AUDIT.md)에 기록한다. 이 문서의 '최종'은 현 소스 전체 검증·비교 자격을 뜻하지 않는다.

감사일: 2026-09-26. 최종 기준 HEAD: `33b24dd5df959f38c0df4717ff834b96750faf34` + 기존 dirty overlay. 시작 HEAD는 `e38e07865daf19661deaa5d1e580acc5814504ef`였고 작업 중 공유 main에서 다른 작업의 커밋이 들어왔다. 최종 검사 입력 57개를 해시 재대조하여 일치함을 확인하고 Python 집중 테스트를 다시 실행했다. 코드 구현은 이 작업에서 수정하지 않았다. 새 티켓 패킷만 작성했다.

2026-09-26 패킷 재감사: 관측 HEAD `937403350911ffe29246cc68c18f13da098f4991`, 작업 트리 clean 상태에서 아래 57개 검사 입력 및 설치 reference bytes를 재대조하여 drift 0개를 확인했다. `33b24dd5` 이후 새 커밋은 이 입력 밖의 searchd-runtime E2E 테스트를 수정했다. 이 재대조는 최신 HEAD의 전체 Rust/daemon 검증이 아니며, 아래 집중 Python 결과의 실행 revision도 변경하지 않는다.

정확한 시각, 환경, dirty 목록, 검사 입력별 SHA-256, 설치된 reference source hash와 probe 결과는 [audit-evidence.json](audit-evidence.json)에 있다. 해당 파일은 **한정된 코드 감사 기록**이지 완전한 source-closure receipt가 아니다. 이후 HEAD/관련 bytes가 바뀌면 재감사가 필요하다.

## 결론과 작업 매핑

| 관측 | 판단 범위 | 작업 |
| --- | --- | --- |
| SDK runner가 전체 문장을 native DSL로 전달; parser AND, compiler Must | 코드 경로 확인. 의도한 NL task와 API 계약 불일치; native 자체 결함 아님 | RBR-02 |
| QueryOutcome이 기존 response explanation/window 정보 대부분을 보존하지 않음 | 실행/기여/미관측 단계를 분리할 정보가 부족 | RBR-01 |
| symbols 빈 배열, route 3종, record verifier는 chunk ID만 인정 | 전용 symbol 평가 연결이 없음; chunk-only 원계약 위반은 아님 | RBR-04/05 |
| symbol text는 local/container name 중심; 별도 exact-name rank field 없음 | 현 ranker 결함 확정 아님. 재현한 순위 손실에만 조건부 적용 | RBR-08 |
| 1024-byte strict chunk가 mid-line 가능; scoring은 full-line projection | indexed/context 차이. 현재 density-aware NDCG는 의도된 계약 | RBR-06 |
| Rust syntax는 top-level item 중심, non-Rust/parse failure/oversize fallback | 다언어 함수 청킹으로 표현할 수 없음 | RBR-04/06 |
| pinned model test의 reference 비교는 한 문장 8성분; query truncation 정책 차이 | 모델 parity 공백; 짧은 과거 질의 실패 원인으로 확정하지 않음 | RBR-07 |
| ANN 결과가 k개면 exact completion 없이 반환 | count completeness와 exact recall은 다른 계약. 결함 단정 아님 | RBR-07 |
| Semble adapter가 기본 search 호출; alpha 0/1이어도 두 lane 실행 | 순수 lexical/semantic 속도로 해석 불가 | RBR-03 |
| hybrid fetch floor 100 vs semantic top-10 probe 11, 단계가 순차 실행 | 추가 작업량 확인. 실제 latency 기여율은 NOT_RUN | RBR-09 |
| semantic scope별 delete 이후 window append; embedding은 이미 batch | 최적화 후보, 비용·복구 영향 확인 전 변경 금지 | RBR-10 |
| Python collection 211 vs authority 210 | exact inventory 계약 불일치 재현 | RBR-00 |
| 새 Rust diagnostic test declaration이 authority에 없음 | static 누락 관측. 실제 Rust 수집/실행은 NOT_RUN | RBR-00 |
| positive-RSS descendant가 live zero-RSS parent를 통해 연결되면 누락 | 독립 process-tree 불변식의 fixture 반례 재현 | RBR-11 |
| 새 티켓 경로가 현 retrieval closure에 미포함 | 이 패킷을 계약으로 채택할 때 필요한 통합 작업 | RBR-00 |
| 조건부 티켓마다 동일 holdout으로 후보를 선택할 수 있었음 | 반복 평가가 holdout을 development 데이터로 바꾸는 계획 결함 | RBR-06/08/09/10/12와 TEST-PLAN 수정 |

## 이번에 실행한 검증

### 집중 Python 테스트 — VERIFIED, 범위 제한

```sh
python3 -m pytest tools/ci/tests/test_retrieval_benchmark.py -q -p no:cacheprovider -k 'retrieval_diagnostic or current_hand_calculated_rank_metrics or ndcg_credits_each_gold_span_once or current_partial_span or v3_byte_span_verification or v3_duplicate_candidate_byte_span or process_tree_sampler_excludes_zombie'
```

최종 HEAD 재실행 종료 코드 0, 원시 종료 요약 `8 passed, 203 deselected in 5.11s` (최초 실행: `8 passed, 203 deselected in 5.87s`). macOS Python 3.9 환경에서 실행했고 LibreSSL/urllib3 경고가 있었다. 네트워크 동작은 검사하지 않았다. 선택 테스트의 성공은 다음 두 반례를 상쇄하지 않는다.

### Python inventory 일치 — FAILED

```sh
PYTEST_ADDOPTS='-p no:cacheprovider' python3 - <<'PY'
import json
from pathlib import Path
from tools.benchmark.retrieval.proof_inventory import collect_pytest
actual = collect_pytest()['tests']
required = json.loads(Path('benchmarks/retrieval/proof-required-tests.json').read_text())['python']
print({'actual': len(actual), 'required': len(required),
       'extra': sorted(set(actual) - set(required)),
       'missing': sorted(set(required) - set(actual))})
PY
```

출력: actual=211, required=210, missing=[], extra=`tools.ci.tests.test_retrieval_benchmark.test_process_tree_sampler_excludes_zombie_processes`.

`diagnostics.rs`의 `uses_record_span_when_sdk_hit_is_unanchored`도 Rust required list에 없다. 실제 nextest 수집은 수행하지 않았으므로 Rust receipt 실패 실행으로 표현하지 않는다.

### Process topology 불변식 — FAILED

```sh
python3 - <<'PY'
from unittest.mock import patch
from tools.benchmark.retrieval import run
snapshot = '100 50 1024 1.0 S runner\n104 100 0 0.0 S startup-parent\n105 104 4096 3.0 S live-descendant\n'
with patch.object(run.subprocess, 'check_output', return_value=snapshot):
    rows = run._process_tree_sample(100)
print([row['pid'] for row in rows])
PY
```

실제 `[100]`. 독립 PPID transitive-closure oracle상 positive-RSS 측정 대상은 `[100,105]`. 필터링 전 topology가 아닌 resource-filtered rows로 소유 관계를 계산해서 발생한다. fixture 도달성은 재현했지만, 과거 실제 capture에서 이런 snapshot이 발생한 빈도나 누락 bytes는 아직 측정하지 않았다.

## 기존 dirty 변경의 처리

- record-normalized diagnostic spans/ranks 및 task-route/status 일치 검증은 이미 편집 중인 코드에 있다. 미구현 항목으로 중복 등록하지 않았다. Rust 실행 검증은 남아 있다.
- zombie 제외 변경 자체를 되돌리도록 계획하지 않는다. 같은 변경에 섞인 live zero-RSS 연결 노드 삭제를 RBR-11에서 분리 수정한다.
- `sdk_roundtrip.rs`, RepoMap/readiness/runtime 및 기존 문서의 다른 사용자 변경은 건드리지 않았다.
- 공유 main 편집은 허용되지만 이 감사 hash를 미래 변경의 증거로 사용하지 않는다.

## 제외·불확실성

- Rust compile/test, 실제 daemon proof, 최신 Quanta–Semble pair, qualified holdout/성능: `NOT_RUN`.
- 과거 600-task/2,808-candidate 재계산은 이전 감사의 역사적 근거다. 이번 티켓 작성 turn에서 재실행하지 않았으며 최신 코드 성과로 사용하지 않는다.
- 이번 검토는 retrieval 호출·표현·평가·관측·비교·주요 비용 경로다. 저장소 전체 correctness/security 감사는 아니다.
- 순위·청크·ANN·모델·delete 비용의 상대 기여도를 아직 확정하지 않았다. 따라서 해당 티켓은 계측과 조건부 변경으로 구성했다.
- 원시 Semble 출력은 비교 대상이며 gold authority가 아니다. 수동 승인 작업은 개발 backlog에서 제외했지만 외부 자격 조건을 충족했다고 가장하지 않는다.

## 계획의 확정 범위

확정한 것은 수정 경계, 의존성, 첫 실험 matrix, 독립 oracle, 선택/유지 기준이다. 재감사에서 조건부 후보를 development에서만 선택하고 한 조합을 최종 holdout에서 한 번 평가하도록 고쳤다. qualified primary는 기존 graded density-aware NDCG@10이며 rank/query/ingest 목표 지표와 표본 단위를 별도로 사전 고정한다. 실행 전 알고리즘 우승자나 개선 배수를 확정하지 않는다. 세부 실행 순서는 [INDEX](INDEX.md), 모든 티켓의 공통 합격 기준은 [TEST-PLAN](TEST-PLAN.md)을 따른다.

패킷 검증: Markdown 16개(작업 티켓 13개 포함), 상대 파일 링크 88개가 존재하며 code fence/공백 검사에 오류 없음. `python3 tools/prompt-manager/pm.py lint`는 `all 4 target(s) in sync`; 이는 생성된 agent 문서 동기화 검사이지 티켓 내용의 correctness 검증은 아니다. source hash 재대조에서 변경 0개. `git diff --check`와 별도로 untracked 티켓의 trailing whitespace를 검사했다.
