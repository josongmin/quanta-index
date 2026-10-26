# Lexical 저장·검색·증분 검증 최종 구조안

현재 작업·상태·담당은 [Oct-22 SSOT](../oct-22-finalizing/README.md)만 갱신한다. 이 파일은 이전 링크의 진입점이며 별도 실행 큐가 아니다.

- [보존된 전체 본문](../../history/oct-10-index-closeout/LEXICAL-QUERY-PLAN.md)
- [검증 요약](../oct-22-finalizing/VALIDATION.md) · [실행 원문](../../history/oct-10-index-closeout/VALIDATION-2026-10-21.md)
- [현재 고정 계약](../oct-22-finalizing/README.md#epics) · [Native RFC](../../rfc/oct-10-native-incremental-vnext.md)

Accepted 요구와 미수용 조건은 유지한다. 과거 TODO·명령·담당을 새 작업으로 재발급하지 않는다.

## Historical section links

이전 fragment는 아래 anchor로 보존한다. 상세 본문은 [LEXICAL-QUERY-PLAN.md](../../history/oct-10-index-closeout/LEXICAL-QUERY-PLAN.md)의 같은 fragment에서 확인한다. 현재 상태는 SSOT만 따른다.

<a id="1-최종-판정"></a>
<a id="10-과거-비용-근거와-현재-검증-상태"></a>
<a id="101-이전-f16-검색-핵심-경로-구현과-검증"></a>
<a id="1010-수정-경로-재검증과-최종-포맷-분류-2026-10-10"></a>
<a id="1011-backuprestore-경계까지-실제-실행-2026-10-10"></a>
<a id="1012-optimized-large-실행의-실제-query-비용-결함-2026-10-10"></a>
<a id="1013-literal-수리-후-large-통과와-남은-갱신-비용-2026-10-10"></a>
<a id="1014-공통-map캐시보존-비용-회귀와-xl-실패-2026-10-10"></a>
<a id="1015-native-초기-구축의-중간-cow-게시-제거-2026-10-10"></a>
<a id="1016-통합-회귀의-실제-결과와-native-bootstrap-집중-검증-2026-10-10"></a>
<a id="1017-최종-경계-회귀-및-strict-lint-2026-10-10"></a>
<a id="1018-sdk-process-실행-실패의-분류와-fixture-수리-2026-10-10"></a>
<a id="1019-sdk-통과-large-재측정-및-xl-재실패-rca-2026-10-10"></a>
<a id="102-f16-독립-oracle-후속-실행과-paged-rewrite-경계-2026-10-10"></a>
<a id="1020-xl-반복-검증의-구조-수리-2026-10-10"></a>
<a id="1021-large-재실행과-측정-observer-분리-2026-10-10"></a>
<a id="1022-boundary-only-optimized-재측정-2026-10-10"></a>
<a id="1023-quota-실패-후-저장-중복-수리-2026-10-10"></a>
<a id="1024-새-저장-포맷-실행-검증과-진단-벤치-2026-10-10"></a>
<a id="1025-xl4-실패의-원인-경계와-중복-실행-방지-2026-10-10"></a>
<a id="1026-xl-초기-file-query-검증-거절과-segment-단위-교집합-계약-2026-10-10"></a>
<a id="1027-file-query-수리의-집중-검증과-실행-소스-결합-2026-10-10"></a>
<a id="1028-file-query-수리-후-fresh-release와-large-진단-2026-10-10"></a>
<a id="1029-file-query-수리-후-xl-누적-시간-초과와-retention-비용-경계-2026-10-10"></a>
<a id="103-paged-통합-후-추가-결함과-복구-배치-2026-10-10"></a>
<a id="1030-병렬-독립-rca와-과도한-수리-범위의-재감사-2026-10-10"></a>
<a id="1031-한-번의-실행으로-보는-waterfall-계약과-추가-감사-2026-10-10"></a>
<a id="1032-lq5-최소-준비-worker-계약-2026-10-11"></a>
<a id="1033-최소-준비-코드와-xl-종료-검증-2026-10-11"></a>
<a id="1034-lq5-잔여-계약-재대조-2026-10-12"></a>
<a id="1035-publication-capacity-foundation과-미연결-경계-2026-10-12"></a>
<a id="1036-통합-활성화와-daemon-수용-차단-경계-2026-10-12"></a>
<a id="1037-최종-잔여-작업과-native-분업-2026-10-12"></a>
<a id="104-배치-집중-실행과-실제-초기-게시-경로-2026-10-10"></a>
<a id="105-전체-owner-재실행에서-발견한-정렬복구-결함-2026-10-10"></a>
<a id="106-내구성자원-경계-보완과-재실행-2026-10-10"></a>
<a id="107-전체-owner-실행과-후속-구조-수리-2026-10-10"></a>
<a id="108-lexical-재실행과-정책-경계-정리-2026-10-10"></a>
<a id="109-daemon-전체-실행에서-발견한-소유권-회귀-2026-10-10"></a>
<a id="2-적대적-감사-결과"></a>
<a id="21-추가-적대객관-감사-계획의-누락"></a>
<a id="22-후속-재감사-실제-재사용-범위와-소비자-경계"></a>
<a id="23-호출-경로평가-범위-재감사"></a>
<a id="24-정렬파일별-조회실패-정책의-교차-감사"></a>
<a id="3-공개-근거와-적용-한계"></a>
<a id="4-저장-구조-선택"></a>
<a id="41-한-generation의-구성"></a>
<a id="42-증분-쓰기와-compaction"></a>
<a id="43-source-bundle과-identity-규칙"></a>
<a id="44-compaction-게시-범위"></a>
<a id="45-segment-routing과-실제-갱신-비용"></a>
<a id="46-문서버전-id-배정과-재시도"></a>
<a id="5-검증-계약의-의도적-변경"></a>
<a id="51-신뢰와-정확성의-범위"></a>
<a id="52-lifecycle와-다른-owner의-전수-비용"></a>
<a id="53-durable-publication과-실패-전이"></a>
<a id="54-shared-object-quarantine와-scrub"></a>
<a id="55-shared-gc의-mark와-객체-재사용"></a>
<a id="6-query-실행과-메모리"></a>
<a id="61-reader-뒤의-전체-집합정렬-비용-제거"></a>
<a id="62-물리-block과-logical-문서의-경계"></a>
<a id="63-필수-결과와-선택적-preview의-실패-정책"></a>
<a id="7-실행-순서소유권완료-조건"></a>
<a id="71-범위별-체크리스트"></a>
<a id="72-반드시-명시할-자원-정책"></a>
<a id="73-작업-분리와-통합"></a>
<a id="74-조건부-실험"></a>
<a id="8-적대적-acceptance와-성능-기준"></a>
<a id="81-실제-실행-범위와-테스트-매핑"></a>
<a id="82-완료-판정"></a>
<a id="9-포맷-전환백업복구"></a>
<a id="publication-admission-연결과-관측-경계"></a>
<a id="rca-1-작은-갱신-뒤에도-보존-확정은-전수-작업이다"></a>
<a id="rca-2-초기-구축은-작은-durability-group의-확정-비용도-크다"></a>
<a id="rca-2b-한-파일-delta도-touched-segment를-즉시-재작성한다"></a>
<a id="rca-3-과거-검색-거절과-현재-검색-범위를-구분한다"></a>
<a id="rca-4-전체-시간-제한과-관찰-비용을-분리해야-한다"></a>
<a id="근거-범위"></a>
<a id="일반-append의-hard-limit-복구-계약"></a>
<a id="후속-수리-순서와-수용-조건"></a>
