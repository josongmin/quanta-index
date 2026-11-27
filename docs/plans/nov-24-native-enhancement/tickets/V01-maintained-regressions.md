# V01 · 유지 회귀와 의도적 퇴역 판정

## 현황

Native 전환의 삭제된 테스트 ID와 Runtime·Lexical 오라클은 [04 유지 테스트 원장](../../nov-20-native-cutover-closeout/tickets/04-maintained-test-migration.md)의 출하 roster로 관리한다. 현재 roster는 **128개 실명·17타깃·9패키지**, 최종 clean Q·승인 Linux x86-64의 실제 본문은 **0/128 `NOT_RUN`**이다. 승인 old-Q Linux ELF의 SHA와 BOOT만 확인됐다.

11월 24일 작업 트리에서 `python3 tools/ci/check-tantivy-retirement.py`의 의존성·소스 인벤토리는 `VERIFIED`(exit 0)였다. `--check-migrations`는 삭제된 Lexical 테스트 90건과 Runtime 그룹·오라클의 이관·퇴역 증거가 부족해 `FAILED`(exit 1)였다. 06.4/06.5의 동일 출하 후보 검증은 열려 있다. [06 퇴역 검사](../../nov-20-native-cutover-closeout/tickets/06-tantivy-retirement.md)

수집기는 재사용 manifest의 검증 때 읽은 SHA-256을 보관한다. 검증 뒤 내용이 바뀌면 로스터가 거절하고, CLI는 검증한 해시를 출력한다. 수집·이관 Python 회귀 **123/123 `VERIFIED`**. 정식 유지 테스트는 0/128이다. [수집기](../../../../tools/ci/retirement_nextest.py#L844) · [manifest 교체 회귀](../../../../tools/ci/tests/test_retirement_manifest_digest_pin.py)

Nextest inventory는 suite 키와 `binary-id`가 선언된 패키지·대상 종류·타깃 이름과 일치해야 수용한다. 이전에는 통합 테스트 ID를 같은 패키지의 별칭으로 바꿔도 원래 이름의 실행 증거로 읽었다. ID 검사·이관 수집·manifest digest Python 회귀 **173/173 `VERIFIED`**. [파서](../../../../tools/ci/nextest_events.py#L137) · [Nextest ID 규칙](https://github.com/nextest-rs/nextest/blob/cargo-nextest-0.9.104/nextest-metadata/src/test_list.rs#L260)

기존 `quanta-xfs-proof:nov24`의 네트워크 차단 실행에서 독립 Rust 본문 **1/1 `VERIFIED`** (`15787bb7-9414-47c3-99eb-00a5c1e2bc1d`). 별도 실행은 승인 old-Q SHA-256과 UID 1000에서 query·control·ingest 소켓 BOOT를 확인했다. [기존 실행 입력](../../nov-20-native-cutover-closeout/tickets/04-maintained-test-migration.md)

로컬 `quanta-retirement-tools:nov24-amd64` (`sha256:15261735bb37e447f3d51a9a257d2c4996a4aec71cf95c8b3287c7a92a75a408`)에 Rust 1.92.0, SHA를 확인한 Nextest 0.9.104, Debian 서명 패키지의 protoc 3.21.12와 protobuf 헤더를 공급했다. 네트워크 없이 UID 1000으로 Lance의 `ann.proto`와 전체 import를 descriptor로 컴파일했다. 현재 작업 `Cargo.lock` SHA-256 `673a18b8fee17887f8589b8a10e3c2331d8a757daae73c37943b61c99c9b714f`의 registry 아카이브 **582/582** SHA 일치, 같은 이미지의 `./scripts/cargow --lane test-workspace-lane metadata --format-version 1 --locked --offline --no-default-features --filter-platform x86_64-unknown-linux-gnu` **544 packages · exit 0**이다. 최종 clean Q를 고정한 뒤 입력을 다시 대조해야 한다. [Lance 공급자와 ABI](../../nov-23-vendor-retirement/10-execution-work-breakdown.md#lance) · [CircleCI 도구 설치](../../../../.circleci/config.yml#L43)

## 작업 범위

로컬 도구 이미지에서 파생한 `quanta-retirement-v01-roster:nov24-amd64` (`sha256:643e2424a074c4a94fcf49ff2aa09b5d32a2a484be0745494cb5b66086dab3a1`)에 Git, uv 0.9.11, Python 3.12.12와 `uv.lock` SHA가 일치한 parser wheel 5개를 공급했다. 네트워크를 끈 읽기 전용 게스트에서 커밋 `28784da2692e`의 `retirement_nextest --list-roster`가 **128개 이벤트 · exit 0**이었다. 실제 제품 본문은 **0/128 `NOT_RUN`**이다.

같은 이미지에서 승인 old-Q ELF SHA와 크기를 대조하고 UID 1000의 임시 디스크에서 BOOT 소켓 **3/3 `VERIFIED`**였다. 커밋 `28784da2692e`의 lock에 고정된 registry 아카이브 **583/583**의 SHA 및 sparse index 항목 **541/541**을 호스트의 읽기 전용 공급 경로에서 확인했다. 이 수치는 해당 커밋의 실행 입력에 해당한다.

- 삭제된 각 ID의 원래 동작·공개 호출자·실패 오라클을 대조한다. 유지할 계약은 Native의 실제 SDK·데몬 본문과 독립 기대값으로 연결한다. [기존 공개 호출자](../../nov-20-native-cutover-closeout/tickets/05-bench-callers-hosted-ci.md)
- 기존 `tantivy_smoke`·`sealed_manifest`·`text_authority_shards`·`boot_inventory`와 Runtime Risk/Extended의 장애·복구 terminal은 각각 Native 이관, 승인 old-Q 호환, 엔진 내부 퇴역 중 하나로 판정한다. F15 손상 후 재시작과 FileBody 변조·누락·symlink, BOOT 경로 거절, cold reopen 격리처럼 엔진 이름과 무관한 실패 계약은 실제 대체 본문으로 유지한다. [06의 이관 범위](../../nov-20-native-cutover-closeout/tickets/06-tantivy-retirement.md)
- 의도적으로 퇴역할 계약은 영향받는 호출자, 대체 호출 또는 명시적 오류, 해당 거절 회귀와 사유를 [04](../../nov-20-native-cutover-closeout/tickets/04-maintained-test-migration.md)·[05](../../nov-20-native-cutover-closeout/tickets/05-bench-callers-hosted-ci.md)에 기록한다. 단순 삭제나 정적 inventory 통과를 이관 완료로 계산하지 않는다.
- 기존 `retirement_nextest` roster와 [06 출하 검사](../../nov-20-native-cutover-closeout/tickets/06-tantivy-retirement.md)를 사용한다. 별도 수집기·중복 CI 게이트를 만들지 않는다.

## 완료 조건

- [ ] [04.4](../../nov-20-native-cutover-closeout/tickets/04-maintained-test-migration.md)의 최종 clean Q·승인 Linux x86-64에서 선택한 실제 ID 전부의 PASS 또는 검토된 의도적 퇴역 처분을 같은 source·feature·바이너리·원 이벤트에 결속한다.
- [ ] [06](../../nov-20-native-cutover-closeout/tickets/06-tantivy-retirement.md)의 `--check-migrations` 및 출하 의존성·번들 검사가 동일 후보에서 통과한다. 유지 계약은 실제 호출로, 퇴역 계약은 명시적 거절로 확인한다.
- [ ] 보존한 결과를 [08 통합 A](../../nov-20-native-cutover-closeout/tickets/08-release-activation.md)의 기존 인수에 전달한다. 미실행 ID는 `NOT_RUN`, 필요한 입력 부재는 `BLOCKED`로 남긴다.

## 의존·인계

Q01–Q03의 검색 계약 결정과 [09.1 기본 검색 선택](../../nov-20-native-cutover-closeout/tickets/09-native-search-gaps.md)이 이관 오라클을 정한다. 기존 04/05 오너가 본문과 호출자 결과를 소유하며, V03 통합 오너는 [07 제품 검증](../../nov-20-native-cutover-closeout/tickets/07-product-qualification.md) 및 08의 동일 후보 증거를 인수한다. Zoekt 비교 정책은 [09.3](../../nov-20-native-cutover-closeout/tickets/09-native-search-gaps.md#093--nativezoekt-동일-조건-비교의-입력과-합격선)을 따른다.
