# RBR-11 — Zero-RSS live parent 아래 프로세스 누락 수정

## 현행 판정 — 2026-09-26, `f9c3b4dc` + 공유 dirty

- 구현 관측: `run.py`는 live zero-RSS connector를 소유 그래프에 유지하고 malformed/duplicate PID·missing root를 fail-closed 처리한다. macOS live fixture는 실제 자식 PID/positive RSS를 관측한 뒤 timeout을 시작하며 종료 시 child를 reap한다. cleanup `EPERM`은 성공으로 간주하지 않는다. 아래 `rss_kib > 0` 선필터와 `[100]`은 **수정 전 재현 기록**이며 같은 결함의 재구현 작업은 없다.
- 검증 경계: Python authority는 현재 283개이나 소비자 수정 후 전체 actual collection/terminal·source-stable receipt는 미발급이다. 과거 272/276/280 결과는 현재 증거가 아니다. fixture 코드 존재와 지원 플랫폼 owner proof를 구분하고 clean resource replay/receipt는 `NOT_RUN`으로 유지한다.
- 잔여: ps snapshot은 PID 시작시각/identity를 묶지 않으므로 빠른 PID 재사용까지 증명하지 못한다. 이를 legacy diagnostic 한계로 유지하고, 지원 플랫폼 owner evidence·실제 PID/RSS/cleanup·완전성 부정 fixture·최종 resource replay를 고정 source에서 재검증한다. 과거 peak RSS는 소급 교정 불가. [중앙 코드 감사](CURRENT-AUDIT.md), [잔여 작업](GAP-REGISTER.md).

- 우선순위: P0. sampler 수정 코드는 관측됨; 현 소스 전체 proof는 미발급. [현재 전수 판정](CURRENT-AUDIT.md). 선행: 없음; inventory는 RBR-00과 함께 갱신.
- 원 감사 상태: 수정 전 dirty 코드에서 deterministic fixture 반례 `FAILED` 재현. 아래 원인·반례는 역사적 입력이며 현 코드는 수정됨. 실제 과거 캡처의 누락량은 미측정.

## 과거 후속 감사 — 현 source terminal이 아님

2026-09-26 후속 감사: `ps`의 malformed/duplicate PID 및 root 미관측을 성공 snapshot으로 해석하는 두 번째 fail-open 경로를 RED로 확인했다. `_process_tree_sample`은 이제 이 입력을 `RunError`로 거부하고 `run_monitored_process`는 artifact `complete=false`와 error를 남긴다. 정상 종료 직후 root가 사라졌지만 process group도 비어 있는 경우는 앞선 유효 sample을 무효화하지 않는다; 남은 자식 group이 있으면 불완전으로 처리한다. 시간 의존 테스트는 deterministic process state fixture로 바꿨다. Quanta resource를 불완전으로 변조한 final verdict도 `resource_accounting_incomplete`를 반환한다. 새 회귀 identity를 Python proof inventory에 등록했다. 최종 테스트 bytes의 dirty focused 결과 4 passed/268 deselected와 verdict 1 passed/271 deselected, Python inventory 272/272 일치. 이어 동일 Python 입력 해시의 전체 suite는 272 passed/32 subtests(exit 0, 165.75s)였다. Rust source가 실행 중 이동해 clean-source receipt는 `NOT_RUN`.

## 파일·함수와 원인 — 수정 전 역사 기록

- [run.py](../../../../tools/benchmark/retrieval/run.py) `_process_tree_sample`: 수정 전에는 `rss_kib > 0` 조건으로 그래프 노드를 제외한 뒤 PPID reachability를 계산했다.
- [test_retrieval_benchmark.py](../../../../tools/ci/tests/test_retrieval_benchmark.py) `test_process_tree_sampler_excludes_zombie_processes`: 수정 전 fixture의 살아 있는 startup parent/descendant까지 제외하는 expectation이 있었다.

독립 불변식: 살아 있는 PID 104가 root 100의 자식이고 PID 105가 104의 자식이면, 104의 RSS가 0이어도 positive-RSS 105는 소유 프로세스다.

```text
PID  PPID  RSS_KiB  STATE
100  50    1024     S
104  100   0        S
105  104   4096     S
```

수정 전 결과 `[100]`, 기대 positive-RSS 측정 PID `[100, 105]`. 입력과 당시 실제 출력은 [감사 근거](audit-evidence.json)에 고정한다.

## 수정 설계

1. syntactically valid process topology와 측정 가능 resource rows를 분리한다. live zero-RSS 연결 노드는 PPID 그래프에서 유지한다.
2. 소유 집합을 결정한 뒤 resource policy를 적용한다. zero RSS는 missing과 다르다. 기존 artifact가 positive RSS만 받는다면 그래프 provenance에 노드를 보존하고 metric row만 제외한다. 자손을 같이 버리지 않는다.
3. zombie/dead row 처리, stale/reused PID, root 미관측/종료, ps 실패/깨진 행을 별도 상태로 정의한다. incomplete topology에서 완전한 process-tree peak를 보장했다고 표시하지 않는다. snapshot 방식의 한계도 유지한다.
4. 현재 zombie regression의 정상 live-descendant 기대값을 수정하고 zero-RSS root/intermediate cases를 추가한다. Linux 전용 owner evidence와 legacy ps sampler를 혼동하지 않는다.
5. resource proof/schema가 바뀌면 freeze/replay와 실제 플랫폼 validator를 함께 수정한다. RSS를 1로 치환하거나 unknown을 0으로 채우지 않는다.

## 테스트·완료

- live zero-RSS parent 아래 positive child가 포함됨; 순서가 뒤바뀐 ps 행에서도 동일.
- multi-level zero parents, unrelated positive processes, zombies, malformed/duplicate PID, missing root, ps command failure.
- 기존 child-count/timeout-cleanup 회귀 유지. 지원 플랫폼의 실제 자식 프로세스 smoke에서 소유·측정 연결 확인.
- resource rows와 graph completeness가 replay에서 검증되며 새로운 테스트 identity가 inventory에 등록됨.

공통 [TEST-PLAN](TEST-PLAN.md) 적용. 이 수정으로 과거 peak RSS가 자동 교정되지는 않는다. 과거 캡처는 해당 limitation을 표시하고 필요한 비교를 새로 수행한다.
