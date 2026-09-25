# RBR-00 — Proof inventory와 새 실행 계약의 증거 바인딩

- 우선순위: P0. 구현/통합 검증: `NOT_RUN`.
- 선행조건: 없음. 공통 완료 조건: [TEST-PLAN](TEST-PLAN.md).
- 성격: 현재 inventory 불일치 수정 + 새 티켓 계약을 위한 필수 통합.

## 근거와 수정 지점

- [proof-required-tests.json](../../../../benchmarks/retrieval/proof-required-tests.json): 현재 Python authority 210개, 실제 collection 211개. zombie sampler 테스트 1개 미등록.
- [diagnostics.rs](../../../../benchmarks/retrieval/src/diagnostics.rs): `uses_record_span_when_sdk_hit_is_unanchored`가 Rust authority에 없다. 이 항목은 static 확인이며 실제 nextest inventory는 아직 실행하지 않았다.
- [proof_inventory.py](../../../../tools/benchmark/retrieval/proof_inventory.py) `collect_pytest`, `verify_inventory_authority`: 정확한 집합 일치를 유지해야 한다.
- [source_closure.py](../../../../tools/ci/source_closure.py) `PROFILES["retrieval"]`: 현재 계약 문서 경로는 sep-23만 포함한다. 새 티켓 계약을 사용하기 전에 이 패킷도 closure에 포함시킨다.
- [run.py](../../../../tools/benchmark/retrieval/run.py) `freeze_inputs`, protocol/spec 검증 및 replay; 같은 디렉터리의 pair-spec/run-manifest/runner schemas.

## 작업

1. 현재 Python/Rust/SDK collection을 소스에서 다시 얻고 정확한 authority로 갱신한다. 숫자만 늘리지 말고 각 identity와 owning rail을 검토한다. 조건부 skip을 pass로 계산하지 않는다.
2. 새 티켓 디렉터리를 retrieval source closure에 추가하고 추가·수정·삭제·관련 dirty 변화의 거부 테스트를 넣는다. 테스트는 `test_write_verification_receipt.py`의 기존 closure fixtures를 확장한다.
3. RBR-01/02/03/04가 사용할 canonical profile 계약을 한 번에 정의한다: query input policy, original/effective query identity, producer/parser profile, comparator settings/actual execution, diagnostic schema. 각 필드의 authority, hash owner, frozen copy, validator, consumer를 명시한다.
4. 기존 internal IR에 병렬 legacy/current twin을 만들지 않는다. wire/artifact schema 변경은 producer·consumer·negative fixture·replay를 함께 수정하고 옛 캡처는 immutable 역사로 보존한다.
5. 새 설정을 바꿔놓고 이전 capture/receipt를 재사용하는 mutation을 거부한다. profile을 admission 없이 qualified evidence로 승격하지 않는다.

## 합격 기준

- 실제 수집 집합과 Python/Rust/SDK authority가 각각 일치하고, missing/extra/duplicate mutant가 실패한다.
- 새 계약 문서/producer/config의 변경이 관련 source/protocol binding을 무효화한다.
- 후속 티켓의 raw producer→frozen manifest→replay validator 연결에 default로 숨겨지는 설정이 없다.
- 공용 schema/run.py 변경은 한 통합 단위로 적용한다. 후속 테스트가 추가될 때마다 inventory를 재검증한다.

## 제외

현재 dirty tree를 억지로 clean으로 만들거나 기존 proof guard를 완화하지 않는다. 문서와 inventory 수정만으로 실제 pair 자격이 생겼다고 표시하지 않는다.
