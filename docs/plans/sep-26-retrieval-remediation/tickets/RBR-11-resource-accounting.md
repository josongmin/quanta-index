# RBR-11 — Zero-RSS live parent 아래 프로세스 누락 수정

- 우선순위: P0. 구현/회귀 검증: `NOT_RUN`. 선행: 없음; inventory는 RBR-00과 함께 갱신.
- 감사 상태: 현재 dirty 코드에서 deterministic fixture 반례 `FAILED` 재현. 실제 과거 캡처의 누락량은 미측정.

## 파일·함수와 원인

- [run.py](../../../../tools/benchmark/retrieval/run.py) `_process_tree_sample`: `rss_kib > 0` 조건으로 그래프 노드를 제외한 뒤 PPID reachability를 계산한다.
- [test_retrieval_benchmark.py](../../../../tools/ci/tests/test_retrieval_benchmark.py) `test_process_tree_sampler_excludes_zombie_processes`: 현재 fixture의 살아 있는 startup parent/descendant까지 제외하는 expectation이 들어 있다.

독립 불변식: 살아 있는 PID 104가 root 100의 자식이고 PID 105가 104의 자식이면, 104의 RSS가 0이어도 positive-RSS 105는 소유 프로세스다.

```text
PID  PPID  RSS_KiB  STATE
100  50    1024     S
104  100   0        S
105  104   4096     S
```

현재 결과 `[100]`, 기대 positive-RSS 측정 PID `[100, 105]`. 입력과 실제 출력은 [감사 근거](audit-evidence.json)에 고정한다.

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
