# Q02 · 인증된 Source 검색 범위

## 현황

**P0 · 부분 구현.** Native Text의 `repo:<pattern>`·`lang:<id>`는 인증된 Source 레코드에 적용되고, 메타데이터가 없으면 `InvalidInput`이다. 최상위 `language_any_of`도 LQ `lang`으로 내린다. 기존 레인 A2는 집중 **63/63 VERIFIED**를 보고했고, main에는 같은 경로·다른 repo, 낮은 점수의 유일한 허용 언어, 누락 메타데이터 회귀가 있다. revision이 붙은 `repo@rev`는 history producer가 없어 명시적으로 거절한다. 생산자 S01 결속과 현행 출하 후보의 공개 수용은 별개다. [preflight](../../../../crates/quanta-index-lexical/src/native/text_preflight.rs#L313) · [언어 lowering](../../../../crates/quanta-index-searchd-runtime/src/native_query_lowering.rs#L124) · [단위 회귀](../../../../crates/quanta-index-lexical/src/native/text/tests/metadata_filters.rs#L263) · [09.1c 레인 결과](../../nov-20-native-cutover-closeout/tickets/09-native-search-gaps.md#진행-기록)

## 작업 범위

- [x] 일반 `repo:<pattern>`·`lang:<id>`와 최상위 언어 조건을 인증된 Source 메타데이터에 결속해 순위·top‑k 전에 적용한다. 누락 메타데이터는 거절한다. 이 체크는 09.1c의 레인 검증 범위이며 전체 `repo`·`revision`·`path`·`lang` 공개 계약 완료를 뜻하지 않는다. [09.1c](../../nov-20-native-cutover-closeout/tickets/09-native-search-gaps.md#091--sdkcli-기본-text를-native에-연결)
- [ ] typed `repo`·`revision`·`path`·`lang` 범위와 고정 뷰의 Source 권위를 하나의 공개 계약으로 결속한다. `repo@rev` 지원 여부는 history producer와 별도로 결정하고, 미지원 상태에서는 명시적 거절을 유지한다. `matched`·top‑k 전에 범위를 적용하며 경로·확장자 추정값을 권위로 사용하지 않는다. [Source 신원](../../../../crates/quanta-index-contract/src/index.rs#L143) · [기존 revision 해석](../../../../crates/quanta-index-sdk/src/native_uds/root_scoped_view.rs#L58)
- [ ] [S01](S01-corpus-reconciliation.md)에서 실제 수집 범위·repo/revision/lang 메타데이터와 제외 사유를 인수한다. 이관·복원 뷰에 필요한 인증 메타데이터가 없으면 해당 필터를 명시적으로 거절한다. 선택한 revision이 사라지거나 커서의 뷰가 달라져도 섞인 결과를 내지 않는다. [보강 계획의 범위 계약](../readme.md#2-인덱싱-대상과-검색-범위를-명시)
- [ ] shared DTO·query 계획·필터 회귀는 Q02가 소유하고, 공개 SDK·CLI entry의 최종 연결은 [Q01](Q01-sdk-codesearch.md)에 인계한다. 공용 wire/Runtime 충돌은 [V03](V03-signed-release.md)의 단일 합류 경계를 따른다.

## 완료 조건

- [ ] 서로 다른 두 repo의 동일 상대 경로, 낮은 점수지만 유일하게 허용된 언어 결과, 삭제 파일, 파일 패턴·revision 제한, 메타데이터 없는 복원 뷰를 고정 fixture로 검증한다. 허용 결과가 top‑k 밖으로 밀리거나 `matched`에 금지 파일이 포함되면 실패다. 원문 검색 범위와 파서가 처리한 언어 범위를 별도 단언한다. [09.1c](../../nov-20-native-cutover-closeout/tickets/09-native-search-gaps.md#091--sdkcli-기본-text를-native에-연결) · [기존 Text UDS 회귀](../../../../crates/quanta-index-searchd-runtime/tests/native_index_uds/read_sessions/query/text_query_contract.rs) · [repo UDS 회귀](../../../../crates/quanta-index-searchd-runtime/tests/native_index_uds/read_sessions/query/repository_query_contract.rs)
- [ ] 공개 경로의 동일 뷰 결과와 커서가 후속 게시에도 유지되고 다른 뷰에서는 거절되는지 확인한다. Q01의 유지 세션 회귀는 [09.1 B2](../../nov-20-native-cutover-closeout/tickets/09-native-search-gaps.md#진행-기록)에 보고됐지만, S01 metadata와 결합한 동일 후보의 범위 결과는 아직 수용하지 않았다. 실제 SDK·CLI 호출 결과는 Q01/기존 [05](../../nov-20-native-cutover-closeout/tickets/05-bench-callers-hosted-ci.md#정식-dsl과-실제-제품-호출)·[07](../../nov-20-native-cutover-closeout/tickets/07-product-qualification.md#native-text와-literal-검증-순서)에 결속한다.

## 의존·인계

S01의 생산자 권위와 [09.1c](../../nov-20-native-cutover-closeout/tickets/09-native-search-gaps.md#091--sdkcli-기본-text를-native에-연결)의 범위 합격선을 받는다. main에 인증된 오버레이 repo/lang과 E2b 오버레이 경로 평가가 있다. SDK `v2` 커서 검증은 요청 repo·revision·고정 generation을 다시 확인한다. S01이 공급하는 메타데이터와 결합한 설치 후보 수용은 열린 상태다. Q01에 typed DTO와 오류 계약을, Q03에 사전 선택된 후보 범위를, V03에 동일 후보의 회귀 결과를 인계한다. [오버레이 경로 회귀](../../../../crates/quanta-index-lexical/src/native/text/tests/path_contains.rs#L373) · [SDK 커서](../../../../crates/quanta-index-sdk/src/client/native_code_search.rs#L735) · [09.1e E2b](../../nov-20-native-cutover-closeout/tickets/09-native-search-gaps.md#진행-기록)
