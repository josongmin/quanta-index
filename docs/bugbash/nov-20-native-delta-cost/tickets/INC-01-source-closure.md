# INC-01 · 한 파일 변경에서 무관한 Source 본문 복사·재해시 제거

담당은 Semantica E01·E02다. 경로는 `semantica-codegraph-v2/packages/analysis/quanta-v2/crates/quanta-runtime/src` 기준이다. [E01](../../../plans/oct-22-finalizing/E01-producer-generation.md) · [E02](../../../plans/oct-22-finalizing/E02-publication-recovery.md#delta-코드-합류와-검증-인계)

| 생산 경로 | 변경과 검증 계약 |
| --- | --- |
| SDK·Watch → committed Source census | 같은 발급자가 Source closure와 경로·기존 content digest의 witness를 보유한다. Native census에서 모든 본문을 새 String에 담던 경로를 제거했다. cold ItemIndex 복원과 ImportGraph 승계는 witness의 경로를 사용한다. 변경 파일의 checked FileText 대조는 원 Source에서 해당 경로만 빌린다. |
| 정확한 게시 준비 | FULL은 독립 입력의 digest 검사를 유지한다. Native DELTA는 snapshot, witness의 객체 동일성, Original grant를 확인한 뒤 Source 대여 범위 안에서 정렬된 경로와 본문 참조를 한 번 모아 lexical·structural batch를 만든다. 경로 조회에는 이진 탐색을 쓴다. shadow semantic의 분모는 같은 Source 세대의 전체 토큰 수다. 새로 읽거나 cold 복원한 member는 본문을 최대 4 KiB씩 선불 계상하며 Unicode 공백 기준 토큰 수를 한 번 계산한다. Source 트리는 세대별 총계를 보유한다. warm DELTA는 이전 총계에서 제거·교체한 leaf 값을 빼고 추가·교체한 leaf 값을 더한다. 같은 leaf를 유지하면 총계 연산을 건너뛴다. Native census와 게시 소비자는 인증된 세대의 총계를 읽는다. Source가 읽기를 최종 승인한 뒤에만 결과를 다음 게시 단계에 넘긴다. |
| ACK 뒤 literal 포트 | snapshot collector가 이미 만든 불변 파일 맵과 FileText의 비용·소유권을 함께 보유하는 Arc를 발급한다. Native 및 LocalChunk 조립은 같은 맵과 Original을 확인한 뒤 alias를 반환한다. 검색용 파일 문자열 또는 새 쓰기 Original을 만들지 않는다. |

생성·소비 심볼은 `try_collect_original_delta_members_into_slots_v3`, `OriginalDeltaLiteralCensusV3::with_borrowed_member_text_v3`, `SearchPlaneLiteralInputV3::with_files_v3`, `prepare_search_plane_aggregate_members_v1`, `NativeLiteralSnapshotRefV3::admit_alias_v3`다. 실제 포트 생성은 `sdk/search_builder/source_bound/snapshot_collector/subqueries/file_content.rs`, 두 반환 분기는 `authority_assembly/search_plane_query_ports.rs`와 `authority_finalize/native_tail_v3.rs`에 있다.

## 열린 Source와 세대 복원의 비용

이미 열린 Native Source를 게시할 때 shadow semantic 분모를 읽는 비용은 O(1)이다. SDK Store가 새 세대 쌍을 여는 비용은 별개다. V1 writer는 각 파일의 CAS payload에 전체 snapshot digest를 기록한다. 한 파일 변경으로 다른 파일의 CAS 참조도 달라질 수 있다. FD reader는 base 트리에서 같은 경로를 한 번 조회한다.

| base와 target 비교 | 처리 |
| --- | --- |
| 언어·본문 digest·parser digest·revision·CAS 참조 일치 | 검증한 기존 leaf를 retain한다. |
| 언어·본문 digest·parser digest 일치, revision 또는 CAS 참조 변경 | 새 CAS를 읽어 객체 digest, 세대·경로·언어, 실제 본문·parser digest를 검증한다. 검증이 끝나면 같은 Original의 본문 backing과 토큰 수를 승계한다. 임시 decode는 scratch 예산으로 처리한다. |
| 새 경로 또는 본문 축 변경 | target 본문을 검증해 새 backing과 토큰 수를 발급한다. |

새 shared header의 입출력은 member hydration DATA가 소유한다. 거절이나 late cancel 때도 이전 본문 또는 이미 생성한 header가 Source finisher까지 살아 있다. retained·selected 읽기는 첫 대여 거절을 각 receiver로 전달한다. 세대 구성에는 전체 member 경로 비교가 남고, V1 참조 변경 시 무관한 파일도 CAS 읽기·digest 검사가 필요하다. 복원 비용과 설치 SDK·Watch의 0 B는 별도 측정 대상이다.

## 완료 판정

1. SDK와 Watch에서 한 파일의 실제 편집을 각각 전달한다. 무관한 파일 수와 본문 크기를 따로 늘리며 census, 게시 검증, ACK 반환의 무관한 본문 복사·재해시가 각각 0 B인지 확인한다. 경로 순회와 shadow 토큰 계산은 별도로 계측한다.
2. 독립 입력의 누락·foreign·변조와 최초 work 거절은 기존 `source_closure_witness_rejects_missing_foreign_and_changed_bytes_v1` 및 `closure_validation_returns_first_work_refusal_without_later_callbacks_v3`에서 검사한다. 두 단위 본문은 독립 파일 맵과 work 콜백을 사용한다. 거절 뒤 Q로 전송한 요청이 0건인지는 실제 게시 테스트에서 확인한다.
3. `admitted_literal_snapshot_retains_original_until_last_alias_drops_v3`는 두 Original로 서로 다른 snapshot을 만든다. 발급 측 소유권을 내린 뒤 실제 Lane1 조회에서 변경 전·후 문자열과 공통 파일을 검사하고 두 비용 원장의 최종 환급을 확인한다. foreign lease 거절은 별도의 세 번째 Original에서 실행해 최초 거절이 유지되고 비용이 환급되는지 확인한다. 이 단위 회귀는 포트 발급 함수를 직접 호출한다. 실제 ACK가 반환한 `LanePortBundle.literal_port`와 SDK·Watch 제품 결과의 수용은 따로 판정한다.

4. Source owner 회귀는 UTF-8 경계를 가로지르는 Unicode 공백과 두 번째 구간의 예산 거절을 검사한다. 실제 Store가 발급한 세대 쌍에서는 토큰 수가 달라지는 교체, 삭제, 경로 변경의 총계를 독립적으로 계산하고 유지한 파일의 본문 주소를 확인한다. Native 게시 소비자의 회귀는 Source에서 받은 총계를 그대로 쓰는지 검사한다.

## 검증 결과 · 11월 21일

| 대상 | 관측 결과 |
| --- | --- |
| Source 토큰 계수 | S 고정 소스 `1fdf14305ccf`, wire lib, `feature-isolation:quanta-runtime-source-authority-wire.no-default.87900619151b`. QBC `20261121T060044.894958Z-fe68256f782b`에서 Unicode 공백·4 KiB 경계 및 두 번째 구간 work 거절 2/2 `VERIFIED`(exit 0). |
| durable Source 구성 | FD 없이 같은 wire 본문을 실행한 QBC `20261121T065926.274205Z-f942cc61aade`에서 2/2 통과했다. |
| SDK 세대 쌍에서 확인한 원인 | 최초 5개가 모두 저장소 identity에서 거절됐다. fixture가 manifest에 첫 Java 파일 경로를 넣고 pin에는 저장소 루트를 넣었다. 두 경로를 루트에 결속한 뒤 QBC `20261121T064143.526806Z-46ea441c9104`에서 3/5 통과했다. 두 실패는 수정·rename 뒤에도 내용이 같은 파일의 본문 주소가 달라진 경우였다. V1 member CAS는 전체 source snapshot digest를 포함하므로 다른 파일을 수정해도 같은 본문의 CAS 참조가 바뀐다. |
| 마지막 코드 수리의 검증 범위 | 같은 본문의 target CAS만 누락·손상시켜 typed 거절과 기존 base 가독성을 확인하는 SDK 본문을 추가했다. 세대 쌍 selector는 이제 6개다. 중간 FD 컴파일의 E0621은 base leaf와 Current의 대여 수명을 묶어 수리했다. 최종 본문 공유·header 수명·거절 전파 소스는 정적 확인까지 마쳤다. 최종 FD/non-FD 빌드, SDK 6개, foreign pin, Native 게시 소비자 본문은 `NOT_RUN`이다. |
| 다음 검증 | 기존 QBC lane에서 wire FD·non-FD의 `code_token_tests_v3`, SDK Store의 `original_full_pair_` 6개와 `original_full_input_rejects_authentic_foreign_repository_pin_v3`, Native 게시의 `search_plane_handoff_dispatch::literal_reads_v3::tests_v3`를 같은 소스로 실행한다. SDK 음성 본문은 prepared index를 그대로 둔 채 실제 Util member 객체만 깨뜨린다. |
| Source reader | 실제 borrowed reader를 독립 컴파일해 조회 결과, 빌린 본문의 주소, 토큰 수를 검사했다. 1/1 `VERIFIED`. |
| Original·ACK owner | QBC의 `quanta-runtime` lib, `feature-isolation:quanta-runtime.no-default.a744081b25ca`, 위 3번 selector. 전체 Q 의존성을 반영한 동결 입력에서 컴파일이 `E0502`로 끝났다(exit 101, 본문 0). 경로를 쿼리에서 빌려 둔 채 같은 쿼리에서 가변 FileText 대여를 시작한 것이 원인이다. |
| E0502 수정 | 기존 checked FileText accessor가 검증한 경로와 읽기 증명을 같은 콜백에 넘긴다. 공급자는 기존 읽기 대여와 최종 승인 순서를 유지한다. 경로·본문 복제는 추가하지 않았다. 원래 공개 accessor도 같은 구현에 위임한다. 후속 QBC `09c5a2dd659b`에서 제품 lib가 컴파일됐고 E0502는 재발하지 않았다. |
| ACK libtest | 같은 QBC는 테스트 크레이트 컴파일에서 exit 101로 종료됐다(121개 진단, 본문 0). 두 snapshot 참조와 그 소유자를 한 튜플에서 이동하던 ACK fixture의 E0505는 참조를 먼저 해제하도록 수정했다. 같은 테스트 바이너리의 공용 helper 가시성, ImportGraph import, FULL binding, SDK fixture의 대여, PTA 접근자, work 거절 타입과 의도적 panic 경계도 현행 API에 맞췄다. 후속 동결 소스 `c6d8848e1771`에서 제품·테스트 크레이트가 컴파일됐고, 두 snapshot의 조회와 마지막 참조 해제 후 환급을 검사하는 본문은 **1/1 통과(0.03초)**했다. 처음 제출한 짧은 모듈 selector와 실제 전체 테스트 경로가 달라 QBC 검증기는 exit 125를 반환했다. 뒤이은 exact 요청은 전체 재컴파일이 시작돼 중단했다(exit 130). |
| 동결 바이너리 직접 검증 | 같은 Cargo libtest 산출물 `quanta_runtime-8e16e20547d349ad`에서 `--list`로 전체 이름을 확인하고 `--exact --test-threads=1`로 세 본문을 실행했다. ACK snapshot 조회·환급, 누락·다른 snapshot·본문 변조 거절, 최초 work 거절 뒤 후속 콜백 0회가 **각각 1/1, exit 0**이다. 이 결과는 로컬 본문 실행에 한한다. 정식 QBC GREEN 영수증과 설치 SDK·Watch 제품 수용은 이번 실행에서 얻지 않았다. |

이전 ACK 단독 실행의 동결 입력은 `/Volumes/Extreme SSD 1/build-cache/scheck-owner-V4657t`에 있었다. 앞선 시도에서 누락된 S 모듈 경로, Q의 `TextPathRankedV1` 응답과 S lock의 `ciborium` 항목은 기존 구현과 맞췄다. SDK·Watch 결과와 무관한 본문 0 B 실측은 [INC-03](INC-03-product-acceptance.md)에서 인수한다.

**Source 원본까지 정적 대조.** `quanta-runtime-source-authority-wire`의 `source_read_v3.rs::read_member_parts_v3`는 경로와 객체 참조를 검증한다. `source_member_hydration_v3/index_member_v3.rs::source_text_v3`는 이미 보유한 문자열을 빌려주고, `content_digest_v3`는 저장된 digest를 읽는다. 최초 cold hydration과 initial pin 발급은 CAS 읽기·본문 해시·소유 문자열 생성 비용이 들 수 있다. SDK에서 FULL literal snapshot을 처음 만들 때도 `load_original_literal_files_v3`가 각 입력 경로의 FileText를 조회해 소유 문자열을 만든다. ACK에서 재사용하는 것은 이때 만들어진 맵이다. FD 기반 이전·이후 쌍을 열 때는 일치하는 member의 hydration을 재사용한다. 제품에서 0 B를 측정할 때는 이미 열린 Source의 변경 처리와 최초 열기 시간을 구분한다.

**남는 비용.** Source successor의 전체 경로 맵 복제(`quanta-sdk-runtime-executor/src/client/repo_state/authority/original_source_successor_v11.rs:191–298`), Q staging 전 전후 경로·digest 대조(`quanta-runtime-retrieval-kernel/src/index_sdk_ingress/native_source_validation.rs:1090–1199`), 첫 미결속 dispatch의 V9 ItemIndex 메모 인코딩(`native_memo_capsule_v4.rs:126–142,272–300`)에는 파일 수에 비례하는 비용이 남는다. 기존 전송기는 이미 변경분만 보내고 삭제에는 본문이 없다. SDK에서 기존 ImportGraph를 승계할 때의 경로 대조와 cold 복구는 해당 분기를 실제로 탔을 때만 귀속한다. 파일 수에 따라 원 예산을 넘거나 지연을 설명할 때 해당 단계를 기존 담당이 후속 수리한다. 이 티켓을 닫아도 전체 DELTA가 변경량에만 비례한다는 판정은 유보한다.

## Source V2 게시 연결 · 11월 22일

- V4 manifest는 기존 V1 인덱스와 V2 인덱스 주소·스냅샷 번호·세 루트를 함께 봉인한다. 인덱스는 manifest 주소를 입력으로 사용하지 않으므로 먼저 준비할 수 있다. Store의 V2 복원은 커밋된 세대 참조가 지명한 인덱스만 연다. 기존 commit 경로는 전환 권한이 없는 V4 준비 결과의 HEAD 갱신을 거절한다.
- E02는 Lexical 이벤트 계보를 확정한 직후, 산출물 생성 전에 V2 증명과 대조한다. 계보에는 stream·event·선행 event ID가 포함된다. 최종 검사에서 동결 이벤트의 payload SHA-256과 operation을 다시 확인한다.
- Store의 V4 최초 FULL 커밋은 기존 HEAD 잠금 아래 빈 HEAD와 같은 manifest 재시도를 처리한다. 기존 V1 HEAD에서 전환할 때는 Source pointer를 고정한 뒤 더 높은 `EquivalentSnapshot`을 비교·갱신한다. 호환 hydration은 cold V1 본문을 한 번 더 읽는다.
- E02는 승인된 pending attempt와 정확한 선행 V9 ACK에서 event·operation을 계산한다. DELTA는 pending 소유권 이전과 최초 V9 Preparing 삽입을 같은 트랜잭션에서 처리하며, V2일 때는 여기에 전이 receipt 주소도 기록한다. 재시도는 정확한 V9 행의 주소를 대조한다. Store는 CAS 주소, target participant의 manifest 주소, 양쪽 V4 seal을 검사한다.
- 변경 V4의 HEAD 게시에서 정상 closeout, startup roll-forward, Watch cold 복구가 같은 Source intent와 pointer를 Store에 전달한다. Store는 실제 Source manifest의 변경 행을 base에 적용해 V4 target 루트를 다시 확인하고, 정확한 Source pointer를 고정한 채 Store HEAD를 비교·갱신한다.
- SDK와 Watch의 공용 Facts 변경 경로는 V4 base를 확인하고, 실제 ordered mutation의 변경 본문 CAS와 C1 트리로 V4 target을 준비한다. V4에서 V1으로의 HEAD 갱신은 Source CAS 전과 HEAD 잠금 안에서 거절한다. Java 초기 세대는 운영 경로에서 V4를 사용한다.
- 완료된 Source 저널에 receipt 주소를 불변으로 기록한다. Store는 독립 C1 seal, CAS 주소, Source intent를 검사하고 Original Current 아래에서 증명을 빌려준다. receipt 인코딩과 재생 메모리는 실제 wire 길이와 변경 행에서 계산한 한도를 적용한다.
- Normal SDK, Facts, TaintRows, Watch는 E02가 산출물을 만들기 전에 Store 증명을 연결한다. Normal과 TaintRows의 Set/Delete는 완료 Source manifest와 요청 경로를 대조한다. Facts와 Watch는 기존 유료 ordered context로 Rename의 두 경로와 전후 digest를 건넨다. TaintRows cold 경로는 저널이 지명한 선행 Search 게시를 대조한 뒤 같은 공급자를 넘긴다.
- Runtime의 승인된 복구 경로는 먼저 정확한 V9와 전후 V5를 읽는다. 해당 산출물의 스냅샷·C1 루트·Lexical 이벤트·operation을 완료 Source 저널과 대조한다. 첫 V2 DELTA의 prior V5가 V1이면 실제 기존 게시 경로·내용 digest 목록을 C1 base와 대조한 뒤 dispatch를 허용한다.
- Native DELTA는 최초 V9 이전에 실제 준비 intent, ItemIndex 메모, V2 Source replay link를 서명된 pending carrier에 결속한다. 기존 V2 carrier는 읽을 수 있다. V3 carrier의 기존 DB 수용은 Store 스키마 36 마이그레이션으로 처리한다.
- Native DELTA의 최종 검사는 V5를 동결 digest와 본문 digest로 지정한 staging 경로에서 읽는다. Signed V3 carrier와 활성 PendingCarrier edge를 같은 SQLite 트랜잭션에서 기록한 뒤, 정확한 staging 본문을 최종 CAS 경로에 연결하고 동기화한다. Cold DELTA는 정확한 시도의 carrier에서 V5·동결 요청·Source link·ItemIndex 메모를 인증한다. Carrier만 확정된 경우 같은 V5를 최종 경로에 연결하며, 현재 custody를 확인한 뒤 빠진 marker와 같은 시도의 V9 Preparing을 설치한다.
- 재시작에서 carrier의 메모를 사용할 때 carrier와 V9의 V5 주소 및 Source replay link를 정확히 비교한다. V9가 비어 있으면 최초 삽입 전에 carrier의 link를 실제 V5의 Source witness, Lexical 이벤트 및 operation과 대조한다.
- Marker 설치에 쓰는 실제 Store SQL을 SQLite에서 실행했다. 현재 epoch와 정확한 원래·재시작 custody는 허용했고, 오래된 epoch·custody, 종료된 BoundV2 시도, 점유된 V9 identity, Retired marker edge는 거절했다. Rust Store 본문은 `NOT_RUN`이다.
- V36 Store writer는 버전 조회와 마이그레이션을 같은 `BEGIN IMMEDIATE` 안에서 수행한다. V35 carrier와 과거 custody를 기록한 DB를 실제 Store 진입점으로 두 번 여는 회귀를 추가했다. SQLite에서 이행 SQL과 역사 행 보존, FK 및 복원된 guard를 확인했다. Rust 본문은 `NOT_RUN`이다.
- Q DELTA 전용 seal은 현재 ingress의 fence에 mutation root·count·digest와 동일 ingress의 History 참조를 기록한다. fence가 회전하거나 ingress가 삭제되면 같은 트랜잭션에서 영수증을 제거한다. 기존 `SealMutations`는 동일 계약을 유지한다.
- S 스키마 37은 현재 Native V9 Dispatching의 revision에 대해 mutation root·count·History 참조를 Q seal 요청 전에 기록한다. cold dispatch는 같은 witness와 Q 영수증을 대조하고, 영수증이 비었으면 같은 ingress와 root로 seal만 재시도한다. 이어지는 V9 binding은 기존 CAS를 사용한다. SQLite에서 Q 영수증 무효화와 S witness의 revision·불변성·FK를 확인했다. 결합 Rust/설치 SDK는 `NOT_RUN`이다.
- S 스키마 38은 최초 Q staging 전에 해당 V9의 시작 기록을 남기고, 각 mutation page ACK 뒤에 누적 root와 건수를 기록한다. Q는 저장된 root가 기존의 미봉인 ingress에 확인된 경우에만 같은 operation의 fence를 갱신한다. S는 저장된 prefix 다음부터 진행하며 최초 seal witness가 최종 prefix와 일치해야 한다. S에 기록되기 전에 응답을 잃은 page는 다시 계산할 수 있다.
- 단일 page를 저장한 직후 프로세스를 끊는 Product 회귀를 등록했다. 재시작에서 mutation page를 다시 보내지 않고 같은 operation의 ACK와 G2 공개를 확인하도록 구성했다. SQLite에서 두 페이지의 누적 저장, 오래된 revision, seal과 다른 root, seal 이후 갱신을 검사했고 V37 seal 보존도 확인했다. Rust 및 설치 Product 본문은 `NOT_RUN`이다.
- Native TaintRows의 개인 cold bootstrap은 원래 idempotency key와 실제 Source 영수증이 일치할 때 기존 재시작 경로에 들어간다. 최종 정규 응답을 durable idempotency에 기록한 뒤 저널을 정리한다. 설치 SDK 본문은 `NOT_RUN`이다.
- Java Original FULL은 기존 HEAD와 Source 포인터보다 높은 분석 스냅샷이 있을 때 입력을 V4 `EquivalentSnapshot`에 건넨다. 포인터를 읽은 Store를 유지하고 HEAD 교체 시 같은 포인터를 재확인한다. 전환 HEAD가 이미 기록된 재시도는 같은 포인터에 결속된 선행 세대에서 다시 준비하고, 결과 주소가 기록된 HEAD와 같을 때 이어간다. 이 분기의 정상 기동 도달, 중단 후 재시도, 이어지는 첫 DELTA는 현행 결합 코호트에서 `NOT_RUN`이다.
- V4 변경 커밋의 동일 검증 단계 안에서는 이미 인증한 V1 target·predecessor closure를 정확한 인덱스와 결속해 다시 쓴다. Source CAS의 전후에 필요한 전체 본문 검사는 유지한다. warm DELTA의 무관 본문 0 B는 남아 있다.
- SDK Facts의 명시적 target은 기존 Q 게시 상태와 대조해 요청 target과 승인된 Host를 결속한다. 같은 Query 시도에서 읽은 이전 Search Product와 준비된 Source 입력을 기존 영속 레시피에 저장한 뒤 Source CAS에 들어간다. 재시도는 저널의 Host와 target을 확인하고, Source 결과에 결속된 semantic candidate를 다시 읽는다. Runtime 공개 경로와 검증 정책을 맞췄고 boundary preflight 24개 및 정책 테스트 77개가 통과했다. Rust 및 설치 Product 본문은 `NOT_RUN`이다.
- V9와 carrier 최초 조회가 모두 비어 있으면 signed SQL 한 문장으로 정확한 BoundV2 owner·epoch, pending phase, restart lease와 양쪽 단계를 다시 확인한다. 그 사이 게시가 진행된 경우와 owner가 바뀐 경우를 정확한 게시 전 상태와 구분한다. 현행 Store 스키마에 SQL 19개 조건을 실행했고 경계 검사 17개가 통과했다. 서명된 worker와 Runtime 결합 본문은 `NOT_RUN`이다.
- `native_delta_rearms_same_bound_attempt_before_v9_or_carrier_v3` Product 회귀를 등록했다. 획득한 BoundV2 상태를 기록한 직후 프로세스를 끊고, 같은 key·Q operation·closeout·attempt로 ACK와 G2 readback을 얻으며 Source CAS를 추가로 시도하지 않는지 확인하도록 구성했다. Product 본문은 `NOT_RUN`이다.
- 첫 pending carrier 쓰기는 같은 트랜잭션에서 V9 선점을 확인한다. 기존에 기록된 동일 carrier는 재시도할 수 있다. Native 경로는 writer의 현재 workspace lease까지 확인한다. 현행 SQLite 스키마에서 두 연결로 V9 선점, 정확한 retry와 stale lease를 재현했다. Rust 본문은 `NOT_RUN`이다.
- V38 다중 페이지 Product 회귀 `native_delta_resumes_after_durable_q_mutation_prefix_multipage_v3`를 등록했다. 변경 129건 중 처음 128건을 signed Store에 기록한 뒤 프로세스를 끊고, 같은 Q operation의 새 fence에서 남은 한 페이지와 누적 129건을 확인하도록 구성했다. Product 본문은 `NOT_RUN`이다.
- V39는 v159의 BoundV2 metadata와 같은 트랜잭션에서 인증된 SDK Source intent, Q target digest, plan·attempt·body version을 기록한다. 동일 Source intent나 attempt가 다른 결속을 얻지 못하게 하고, 정확 재시도는 저장된 결속을 재검사한다. V38→V39 스키마 적용, SQL 조건 18개, 경계 검사 17개를 확인했다. Rust 및 signed worker 결합 본문은 `NOT_RUN`이다.
- V39 signed Store의 lease 승계는 Source intent·Q target으로 결속을 찾고, 같은 BoundV2 owner·attempt·body version을 유지한 채 정확한 이전 OS lease만 CAS로 교체한다. 이미 V9나 carrier가 있거나, 다른 활성 시도·restart lease가 있으면 이 경로에서 승계하지 않는다. 실제 SQLite 문장으로 조건 17개와 carrier·lease 쓰기 두 순서를 확인했다. Rust와 설치 Product는 `NOT_RUN`이다.
- SDK private cold bootstrap은 Source 저널을 재확인하고 현재 Source 포인터를 고정한 채, 같은 pending DB의 signed writer에서 승계를 호출한다. 쓰기 작업에 journal과 실제 OS effect lease를 보존한다. 승계 뒤에는 기존 runtime generation으로 discovery를 다시 읽고, 후속 checkpoint가 같은 lease를 사용하도록 SDK가 보유한다. 결합 경계 검사 22개가 통과했다. Rust 및 설치 Product 본문은 `NOT_RUN`이다.
- Cold bootstrap의 signed epoch 갱신 전에 기존 checkpoint 호환성과 선행·완료 checkpoint를 검증한다. Runtime의 두 승계 모듈은 Unix에서 signed Store, SDK ingress, Original snapshot, 기존 retrieval authority를 함께 쓸 때 열린다. Feature closure 4개, 경계 검사 24개, 현행 Store 스키마와 lease 관련 실제 SQL 조건 12개를 확인했다.
- pre-V9 fixture에서 훅이 항상 전달하던 `None` 단언을 제거했다. 이 fixture는 signed V9 부재를 조회하며, 절단 시점의 durable carrier 부재는 별도 확인이 필요하다.

### 동일 시도 재시작과 Q 응답 경계

- Native SDK 복구는 signed Store에서 Source intent·Q target·body version, 현재 BoundV2 owner와 OS lease, V9·carrier 부재를 확인한 뒤 기존 scheduler를 다시 실행한다. 복구한 body는 실제 producer의 plan·outcomes·dedupe와 대조한다. 기존 replay 작성 시각을 사용하고, 새로 준비한 재생 요청을 저장된 요청과 대조한 뒤 같은 owner로 writer를 준비한다. 이 분기는 새로운 `StartBoundV2`나 Source CAS를 호출하지 않는다. pre-V9 절단은 실제 signed Store가 두 단계의 부재를 확인한 뒤 발화한다.
- Linux Product suite 네 건을 등록했다. pre-V9 동일 시도, 129건의 durable prefix, Q 페이지 ACK 후 S progress 기록 전 종료, Q seal receipt 확인 후 S 처리 전 종료다. 페이지 절단은 새 fence의 128·129 ACK를 요구하고, seal 절단은 재시작 전 Q의 정확한 영속 receipt를 읽는다. 네 selector가 각각 본문 하나로 해석됐다. 현행 SQLite 문장으로 정상·다른 Source/target·stale owner/lease·다른 body version·경쟁 시도·늦은 V9/carrier를 포함한 11개 조건을 확인했다. Rust 및 설치 Product 본문은 `NOT_RUN`이다.
- warm DELTA의 일반 수화와 Original 유료 V1 검증은 서로 다른 권한과 작업량을 확인한다. 유료 본문 검증을 유지한다. 0 B 수용에는 정확한 V1·V2 CAS 참조와 커밋 시점까지 유효한 불변성 증명, 실제 participant commit을 타는 계측 회귀가 남아 있다.

### 남은 구현과 검증

1. `native_delta_rearms_same_bound_attempt_before_v9_or_carrier_v3`와 `native_delta_recovers_pending_carrier_before_writer_markers_v3`에서 초기 부팅부터 동일 attempt·Q operation의 ACK, G2 검색 공개 및 Source CAS 1회를 확인한다. 오래된 stage는 기존 attempt·carrier·writer fence로 회수한다.
2. 단일 페이지와 새 다중 페이지 회귀를 실행한다. Page ACK가 S 기록 전에 유실되는 경우, Q가 seal을 영속화한 직후 응답이 유실되는 경우, fence 변경, binding 전후를 각각 확인한다. 동일 Source CAS, writer 준비, Q Submit 횟수와 History 참조를 확인한다. 현행 V39 signed SQL worker와 Q 서버를 같은 코호트에 설치한다.
3. Java FULL 전환 후 해당 스냅샷의 Product 기준 세대와 첫 V2 DELTA를 검증한다. Native FULL의 검증된 C1 tree를 보존해 HEAD·manifest·root가 일치하면 DELTA 준비에서 선행 V2 tree를 재구성하지 않는다. Target V1·V2 전체 본문 검사는 준비와 커밋에 남아 있고, 커밋은 그 전에 V1 target·predecessor도 hydrate한다. Source CAS 전후 각 단계의 손상 검사는 유지하면서 같은 단계 안의 중복 읽기를 줄이고, 변경분과 전체 호환 읽기를 각각 계측한다.
4. 기존 QBC lane에서 Wire와 Store의 focused 본문을 먼저 실행한다. 11월 22일 외장 볼륨의 미참조 snapshot 1개를 회수했다. 현행 캡처 두 번은 Quanta sibling overlay가 캡처 중 변경돼 중단됐다. 기존 동결본으로 실행한 Reader statement-set 요청 `875bf4e6c1b48bfc57260f3002ec44db`는 호스트 컴파일 admission에서 exit 75로 종료돼 본문 0개다. 후속은 SDK·Watch 변경과 손상된 receipt, Source CAS 직후 및 V9/V5 준비 후 중단 복구다. 정상과 재시작에서 실제 변경·삭제·Rename 결과, Source CAS와 게시 횟수, 무관 본문 바이트를 확인한다.
