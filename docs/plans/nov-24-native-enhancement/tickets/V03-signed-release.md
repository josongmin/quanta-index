# V03 · 단일 통합 오너와 서명 출하 인계

## 현황

보존 후보의 집중 회귀와 Caddy·etcd Record, Large 결과는 각 동결 입력에 귀속한다. [03 설치 원장](../../nov-20-native-cutover-closeout/tickets/03-signed-published-cp2.md)의 S `2efdb35a328`·Q `28784da2692e+ovl-cb270e7e…` 준비 기록은 승인 Q **2/3**, S SQL **27/48**까지 확인했다. 이 준비본은 이후 물리 Source 경로가 유실되어 독립 검토 전에 퇴역했다. 퇴역 manifest `3c1f4e3b4c8b…`의 사유는 `PHYSICAL_SOURCE_MISSING`이며, 서명 Linux 설치·제품 실행은 `NOT_RUN`이다. [04.4](../../nov-20-native-cutover-closeout/tickets/04-maintained-test-migration.md)의 최종 유지 본문은 **0/128 `NOT_RUN`**이다. [08 활성화](../../nov-20-native-cutover-closeout/tickets/08-release-activation.md)의 T/A/B와 운영 증명은 열려 있고 저장된 aggregate는 `production_ready=false`다.

다음 P07 준비 실행 `p07-source-current-cohort.JWdBz8`은 Q 작업 트리를 수집하는 동안 overlay digest와 unstaged diff가 바뀌어 `SnapshotSourceDriftV1`, exit 1로 종료됐다. 이 실행의 후보 준비는 `FAILED`이고 서명·설치 본문은 `NOT_RUN`이다. 11월 25일 다시 확인한 활성 pin의 S·Q 물리 경로는 둘 다 부재했고, 후속 후보의 캡처·독립 검토 기록도 없었다. 기존 P07 경로에는 안정된 S·Q 입력, 독립 검토와 승인 pin이 필요하다. [준비 원로그](/Users/songmin/Library/Caches/quanta-index/p07-source-current-cohort.JWdBz8/prepare.log)

Q01 집중 실행 40/41과 수정 후 실제 CLI exact 1/1을 로컬 수용 범위로 인수한다. 최신 후보 원장의 승인 S/Q 동결본과 동일 후보 P10 Linux ELF가 아직 없어 출하 입장은 `BLOCKED`다. Root/F가 승인 후보에 Q01의 Native 태그 83과 가산 Catalog 스키마를 결속한 뒤 `just rust-public-api`를 실행한다. V03의 서명 Linux 설치·이전 설치 복귀는 그 후보에서 실행한다. 공용 API 검사와 설치 본문은 `NOT_RUN`이다. [Q01 마감 순서](Q01-sdk-codesearch.md#합류-절차) · [후보 원장](../../nov-23-repomap-retirement/04-integration-acceptance.md#candidate-custody) · [공용 API 검사](../../../../tools/ci/lint/check-public-api.py)

## 작업 범위

- 공유 SDK/daemon 설정·등록·CI·출하 manifest 수정은 V03 한 통합 오너가 순서대로 합친다. Q01–Q03 승인 결과와 S01–S02, D01–D02, V01의 기존 소유 결과를 입력으로 받고 중복 검증 레인을 만들지 않는다.
- 동일 source/Cargo·feature·설정·정책·바이너리와 Linux 설치 신원을 [04 유지 테스트](../../nov-20-native-cutover-closeout/tickets/04-maintained-test-migration.md), [05 공개 호출자·벤치](../../nov-20-native-cutover-closeout/tickets/05-bench-callers-hosted-ci.md), [06 퇴역](../../nov-20-native-cutover-closeout/tickets/06-tantivy-retirement.md)의 원 증거와 대조한다.
- [09.3a 정책](../../nov-20-native-cutover-closeout/tickets/09-native-search-gaps.md#093--nativezoekt-동일-조건-비교의-입력과-합격선)을 최종 후보 확정 전에 인수한다. V02 결과는 Zoekt 비교가 출하 게이트로 확정된 경우에 필수 입력이다. I01–I03은 해당 IDE 기능을 출하할 때 수용하고, P01–P03은 각 개선 범위와 기존 제품 게이트에 따라 인수한다.

## 완료 조건

- [ ] Q01–Q03의 승인된 검색 계약과 S01–S02/D01–D02/V01의 기존 담당 원장 수용 결과를 같은 후보에 결속한다. [04.4](../../nov-20-native-cutover-closeout/tickets/04-maintained-test-migration.md)·[05.6](../../nov-20-native-cutover-closeout/tickets/05-bench-callers-hosted-ci.md)·[06.4/06.5](../../nov-20-native-cutover-closeout/tickets/06-tantivy-retirement.md)의 기존 분모를 실제 본문과 원 영수증으로 수용한다.
- [ ] [03](../../nov-20-native-cutover-closeout/tickets/03-signed-published-cp2.md)의 승인 Linux 서명 설치·복구와 [07.3–07.8](../../nov-20-native-cutover-closeout/tickets/07-product-qualification.md)의 SDK60, Large, ANN4K/whole SDK, 독립 Hybrid, E07 및 두 기존 기준본 비교를 각 원 오너 결과로 확인한다. Zoekt 수치는 09.3a 승인 범위만 적용한다.
- [ ] [08.1–08.7](../../nov-20-native-cutover-closeout/tickets/08-release-activation.md)의 격리 T, 통합 A, 코드 자격, 실제 운영 입력·선택 CAS, 통합 B/P11, 최종 aggregate를 순서대로 인수한다. 첫 durable commit 전 복귀와 이후 restore-forward를 구분하고, 필수 `FAILED`·`BLOCKED`·`NOT_RUN`이 남으면 승인하지 않는다.

## 의존·인계

기존 D/E08/F 및 각 코드·제품 오너가 원 영수증을 발급한다. V03은 동일 후보 결속과 공유 변경의 단일 합류를 책임지고, 운영 선택·최종 증명은 [08](../../nov-20-native-cutover-closeout/tickets/08-release-activation.md)의 권위에 인계한다. 새 검증 작업은 원 담당 티켓에서 실행·판정한다.
