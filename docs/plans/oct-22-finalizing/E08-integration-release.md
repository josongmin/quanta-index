# E08 — 전체 통합·IntelliJ 비교·CI/release

상태는 [SSOT E08](README.md#e08). 원 요구 R40와 원래 전체 종료 판정을 소유한다. 기능/수명 oracle는 E01–07에서 한 번 만들고 이 에픽은 동일 source/input의 결과를 소비한다.

## Integration gates

1. 정적/공용 합류: actual producer→caller→consumer·지원 feature/import·typed cause·partial/Source/backing/drop·독립 gold와 공개 API/module/wire/Cargo 정합성.
2. 영향 owning: Q는 Justfile/`./scripts/cargow`, S는 기존 P Runtime·PUB/PY SDK/CP owning 요청. 같은 immutable RED를 filter/feature 변형으로 반복하지 않고 확인된 owner 원인을 고친 뒤 같은 rail로 재검증.
3. 통합 A/pre-switch: 실제 FULL/delta/query·5 durable cut·AST/BOOT candidate·고정4K·fault/pressure·Large/XL/운영·IDE/성능의 필수 범위.
4. E06 default switch.
5. 통합 B/post-switch: default smoke/rollback·current coherent full workspace·공개 API/module·exact-source hosted CI/release.

## Actual IntelliJ acceptance

[RFC 12.5절](../../rfc/oct-10-native-incremental-vnext.md#125-ide-and-indexing-acceptance-against-an-actual-intellij-baseline)가 원래 약속한 비교다. exact IntelliJ edition/build/plugins/JDK와 Quanta client/Runtime/Index, 같은 source/dependencies/action trace·하드웨어·cache 정의를 실행 전에 고정한다.

실제 renderer와 같은 지원 capability·독립 품질 gold를 비교한다. Core-only indexing과 IntelliJ 전체 semantic readiness를 비교하지 않는다. 모든 필수 comparable 시간/준비 metric의 Quanta/baseline 비율 상한95% CI≤1.0과 품질/잘못된 edit0·RFC absolute gates를 함께 충족해야 “IntelliJ급 이상”을 판정한다. sample/반복 수 부족·baseline/실제 client workflow 부재는 해당 claim 미수용이다.

## Release inputs

기존 [proof-authority](../../../tools/ci/proof-authority.toml)의 p03–p11 실제 activation/lifetime/query/SDK/provider/supervisor/readiness/state migration/exact-pair/운영 action·독립 observer를 원 registry/recipe로 수용한다. staged 선언은 코드 부재 증거가 아니고 local pass는 installed/hosted 증거가 아니다.

CODE_QUALIFIED·DEPLOYED·ACTIVATED·ROLLBACK_PROVEN을 별도 판정한다. 실제 target/authority/설치 provenance 누락은 해당 gate BLOCKED이며 새 target issuer/runner를 추측해 만들지 않는다. [기존 release 세부 계약](../../history/oct-22-finalizing/oct-17-index-closeout/tickets/W2-19-integration-acceptance.md)과 CircleCI exact-source/test-authority 계약을 유지한다.

## DoD

- [ ] [R01–R40 추적표](DOD.md#requirements)의 각 주 에픽 DoD와 실제 source-bound oracle가 충족.
- [ ] P2I01–08·필수 UX01–12/AUTO01–10·실제 client workflow/IntelliJ 비교를 수용. 조건부 N/A는 근거와 absent capability를 명시.
- [ ] A→switch→B 순서와 full-suite·정적/strict·공개 API/module·exact-source remote CI·release target 수용.
- [ ] 기존 A/D/C5 완료 범위는 해당 evidence를 재사용하고 변경 영향만 회귀. 전체 미수용을 이유로 재구현하지 않음.
- [ ] 필수 FAILED/BLOCKED/NOT_RUN·미해결 외부 입력·모호한 owner/누락된 시나리오0. 오래된 pass·partial/skipped/ignored를 합산하지 않음.
- [ ] 문서/SSOT/owner/실행 요약이 최종 source와 일치하며 [전체 DoD](DOD.md#final-dod)의 모든 조건이 닫힘.
