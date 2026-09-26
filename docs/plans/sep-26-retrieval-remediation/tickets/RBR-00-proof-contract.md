# RBR-00 — Proof inventory와 새 실행 계약의 증거 바인딩

## 현행 판정 — 2026-09-26, [중앙 코드 감사](CURRENT-AUDIT.md)의 공유 dirty source

- 구현 관측: `proof_inventory.py`의 exact-set 검증, retrieval source closure의 sep-26 경로, profile·diagnostic/replay 계약이 현재 owning 소스에 있다. 아래 210/211 및 closure 미포함은 **티켓 작성 당시 반례**이며 현 상태가 아니다.
- 검증 경계: 현재 authority는 Python319/Rust108/SDK18이다. 의도된 process preflight31을 기존288에 추가해 실제319 collector/verify와 fresh31/JUnit 정상·거부 증명 통과, 삭제/중복0이다. `/private/tmp/qi-rbr-inventory-319.bR3YFF/receipt.json`, SHA `94a1228222c361ba2b66eef5883eaf22de0ec8a3b69a39f0e235ff573d7f9185`. Rust108/SDK18은 exact selected/executed/passed 일치·failed0; `/private/tmp/qi-rbr-sdk-symbol-final.TEINH2/actual-terminals.json`, SHA `713a3cfd070b573ce27af094393712ada341a396285062e93508f7f4e46f6fdd`. Rust108 선택 입력 전후 동일, SDK 실행 당시 binary 두 개 전후 동일이나 source drift local이고 cached runner는 후속 rebuild로 변경됐다. 이전 전체288 실행의285pass/3fail와 fresh3pass, immutable owner283 전체, 새31 focused를 최신319 전체로 합성하지 않는다. full319/clean-source contract/SDK closure는 NOT_RUN이며 문서 변경도 closure 입력이다.
- 최종 zipapp manifest 재감사: genuine bundle bytes/hash와 모순되는 선언 metadata5종을 승인하던 경계를 explicit shape/type/order/duplicate 및 embedded canonical-byte binding으로 보완했다. malformed member도 typed refusal로 거부한다. 기존 bundle identity의 실제 isolated 실행 및 focused3 passed, inventory319 불변. 최종 immutable319 전체는 `/private/tmp/qi-rbr-full319-final.kmUWqp/`에서 별도 실행하며 terminal 전 NOT_RUN이다.
- 잔여: 최종 code/document bytes 고정 → 3역할 actual exact collection·negative fixture·terminal execution → 입력/바이너리/로그를 바인딩한 receipt. dirty local proof와 clean-source closure를 분리하고 변경 전 receipt를 재사용하지 않는다. [중앙 코드 감사](CURRENT-AUDIT.md), [잔여 작업](GAP-REGISTER.md).

- 우선순위: P0. Python inventory·closure 코드는 관측됨; 현 소스 전체 proof는 미발급. [현재 전수 판정](CURRENT-AUDIT.md).
- 선행조건: 없음. 공통 완료 조건: [TEST-PLAN](TEST-PLAN.md).
- 성격: 작성 당시 inventory 불일치 수정 + 새 티켓 계약을 위한 필수 통합. 현행 inventory 판정은 위 상태를 따른다.

## 근거와 수정 지점

- [proof-required-tests.json](../../../../benchmarks/retrieval/proof-required-tests.json): 작성 당시 Python authority 210개, 실제 collection 211개였고 zombie sampler 테스트 1개가 미등록이었다. 현행 수량은 위 판정을 따른다.
- [diagnostics.rs](../../../../benchmarks/retrieval/src/diagnostics.rs): 작성 당시 `uses_record_span_when_sdk_hit_is_unanchored`가 Rust authority에 없었다. 현행 Rust exact collection은 아직 재실행하지 않았다.
- [proof_inventory.py](../../../../tools/benchmark/retrieval/proof_inventory.py) `collect_pytest`, `verify_inventory_authority`: 정확한 집합 일치를 유지해야 한다.
- [source_closure.py](../../../../tools/ci/source_closure.py) `PROFILES["retrieval"]`: 작성 당시에는 sep-23만 포함했다. 현행 코드에는 sep-26 경로가 포함된다.
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
