# 02 · 파일 경계를 넘는 경로 쓰기 배치

**보류.** 최근 제품 본문에서 동일 4,096개 SourceAdvance가 게시됐고 지연은 후속 materialization에서 발생했다. 대상 함수는 [기존 TEXT 작성 경로](README.md#채팅별-실행권과-재사용-범위)에 속한다. 경로 scratch가 지배한 별도 실측이 나오면 B가 입력과 비용을 제공한다. [최신 4K 결과](04-product-acceptance.md#기존-실행기-결과)

**착수 조건.** [01](01-attribution.md)에서 scratch 제출 비용이 확인됐을 때 TEXT 담당이 적용한다. 변경 위치는 `PathWrites`다. Tree·Layered가 이 writer를 공유하므로 두 값 형식의 회귀를 함께 실행한다. [경로 writer](../../../crates/quanta-index-lexical/src/native/text_path.rs#L256) · [Layered 호출부](../../../crates/quanta-index-lexical/src/native/text_segment/build_v2.rs#L594)

## 변경 단위

1. 현재 `emit_merge`는 owner별로 `PreparedRows<FundedEdit>`를 만들고 마지막 잔여분도 바로 제출한다. 같은 제한 버퍼의 소유권을 `PathWrites`로 옮긴다. 다음 파일의 편집을 같은 버퍼에 이어 쓰고, 기존 64행이 찼을 때와 `finish`에서 편집 커서를 열기 직전에만 제출한다. 빈 입력은 배치 0회로 종료한다. [현재 flush](../../../crates/quanta-index-lexical/src/native/text_path.rs#L432) · [64행 한도](../../../crates/quanta-index-scratch-sqlite/src/lib.rs#L33)
2. 행별 key와 memory lease는 SQLite에 성공적으로 전달할 때까지 보유하고 이후 반환한다. 기존 scratch 예약, 행별 과금, `PRIMARY KEY(k)`, expected/next 값과 정렬 커서를 유지한다. 부분 제출·마지막 flush가 실패하면 원래 오류를 전파하고 attempt 전체를 폐기한다. 성공처럼 root를 게시하거나 일부 행만 재시도하는 경로를 만들지 않는다. [기존 테이블과 실패 처리](../../../crates/quanta-index-lexical/src/native/text_path.rs#L313) · [기존 펀딩된 버퍼](../../../crates/quanta-index-lexical/src/native/text_memory.rs#L144)

## 검증과 완료 조건

| 입력 | 독립 기대값 |
| --- | --- |
| 편집 0·63·64·65행, 64번째 행에서 owner 교체 | 제출 횟수 0·1·1·2. 빈 배치와 누락·중복 행 0. EOF에서 잔여 행 반영 |
| 동일 토큰을 공유하는 서로 다른 owner, rename·delete, 동일 경로에서 새 SourceRef | 각 owner의 키와 값이 보존되고 expected/next 집합이 고정 입력과 일치. 기존 포맷은 본문만 바뀌어도 SourceRef 값을 교체 |
| 예산 거절 또는 SQL 오류를 중간·마지막 flush에 주입 | 기존 뷰와 root 보존, 실패 원인 전파, attempt 종료 후 메모리 반환. 독립 새 attempt 재시도 결과가 정상 FULL과 일치 |
| 같은 4K Tree 입력·동일 packing으로 구·신 writer 비교 | 편집 24,576행, **INSERT 제출 4,096 → 384회**, 편집 스트림·최종 root 동일. 정렬 페이지 순회의 24,577 논리 읽기는 별도. scratch 읽기·시간·메모리 peak의 실제 전후값 기록 |

기존 `rename_and_delete_emit_exact_unique_preconditions`, case alias와 값 형식 회귀를 재사용한다. 위 배치 경계와 실패 원자성은 같은 테스트 모듈에 보강한다. 아래는 구현 후 사용할 집중 선택이다. [기존 회귀](../../../crates/quanta-index-lexical/src/native/text_path.rs#L721)

```sh
./scripts/cargow --lane test-workspace-lane nextest run --locked --offline \
  -p quanta-index-lexical --lib --no-tests fail \
  -E 'test(/^native::text_path::/)'
```

집중 회귀와 배치 수가 맞아도 실제 비용이 줄지 않으면 이 변경으로 예산 실패가 해결됐다고 판정하지 않는다. 결과와 함께 [01](01-attribution.md)로 돌아가거나 초기 게시가 통과하면 [04](04-product-acceptance.md)로 넘긴다.

버퍼를 파일 사이에 유지하면 직전 파일의 잔여 key lease와 다음 파일의 토큰 준비 메모리가 겹친다. 기존 writer가 통과하던 메모리 한계에서 큰 경로가 배치 경계를 넘는 입력도 대조한다. 새 거절이 생기면 파일당 flush 제거만으로 동등성을 인정하지 않는다. [기존 임시 lease와 버퍼](../../../crates/quanta-index-lexical/src/native/text_memory.rs#L144)
