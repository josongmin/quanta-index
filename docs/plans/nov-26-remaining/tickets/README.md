# 11월 26일 Native 잔여 코드 작업

2026-11-26 KST 대조 기준은 Q `28784da26`, S `6e29b0f8f8d` 및 관련 작업 트리다. 실행 결과는 각 행의 원장에 기록한 입력과 selector에 귀속한다.

## 새로 손댈 코드와 담당

| 티켓 | 처리 유형 | 기존 담당과 완료 결과 |
| --- | --- | --- |
| [01 · Store warm 준비·확정](01-store-sparse-warm-commit.md) | **inventory 중복 해시 제거 합류** | Wire 집중 **1/1 `VERIFIED`**다. Store 작업량 회귀는 이전 S에서 **compile 101 · 본문 0건**이었다. Taint 소스 수리가 합류해 현행 S의 기존 exact 재검증이 남았다. HEAD 잠금 대기 중 필수 메타데이터 변조 거절은 S `b839416a02a`·`f4cd45f97f2`에 합류했다. S `fd4407dc01d`의 재실행은 Runtime 컴파일 진단 17건으로 **exit 101 · 본문 0/1**이었다. `9e62675492f`의 후속 경합 회귀는 DFG import 누락으로 **exit 101 · 본문 0/1**이었다. Java 초기 V4의 검증 거절을 receipt 복구에서 성공으로 되돌리는 경로를 S `bfef0a661f5`에서 수리했다. 같은 S의 typed-reopen은 Runtime 컴파일 오류 4건으로 **exit 101 · 본문 0/1**이었다. S `e44c865a1af`의 typed-reopen은 Java import E0432로 **exit 101 · 본문 0/1**이었다. `b20fa6e267e`에서는 별도 Facts 오류 반환 E0308로 **exit 101 · 본문 0/1**이었다. 오류 변환 후속은 Store lib test 진단 16건으로 **exit 101 · 본문 0/1**이었다. 해당 7개 파일 수정은 `f3619319779`에 합류했다. revision 맵 비교 수리 뒤 드러난 Store 관측자 가시성 4건도 소스에서 수리했다. typed-reopen은 컴파일 후 본문 **1건 실행·실패 1건 · exit 101**이었다. Store가 같은 C1 Current를 빌려주도록 수리했으며 기존 요청의 후속과 Java HEAD 복구가 남았다. 무관 본문 0 B는 설치 Source CAS 보관 권위가 없어 **BLOCKED**다. 지원되는 보관 권위와 실제 IFX150 계측이 필요하다. |
| [02 · ItemIndex 증분 합성](02-itemindex-sparse-delta.md) | 기존 SDK·QE 담당 | `증분 SDK · 변경 경로로 다음 세대 생성` 담당이 공유 seed의 witness 수리를 `077bc3cdf56`에 합류했다. ItemIndex는 영향·변경·삭제만 반영하는 target Current 발급과 Evidence·seal·V9 소비자 이관이 남았다. 격리 count 후보는 해당 담당에서 선별한다. |
| [S02 · V1 호환 pin·revision](../../nov-24-native-enhancement/tickets/S02-single-edit-delta.md#작업-범위) | 기존 Source·SDK 담당 | Facts·disk의 공동 C1 발급과 Source 결속을 인수한다. `issue_ordered_original_from_c1_v2`는 여전히 V1 pin용 전체 헤더·flat digest를 만들고 InputStore는 revision을 전체 투영한다. 이 전환은 01의 물리 CAS 읽기와 구별해서 계측한다. [발급과 revision 경계](../../../bugbash/nov-20-native-delta-cost/tickets/INC-01-source-closure.md#현재-남은-제품-코드--2026-11-26) |
| [03 · Source scrub·재구축](03-native-source-scrub-rebuild.md) | 기존 D02에 인계 | 명시적 scrub 작업 트리 합류 후 Catalog에 보존 뷰 인벤토리·감사 주기·완료 행 재감사 CAS를 추가한다. 기존 Runtime 스케줄러 연결과 독립 FULL 검증·CAS 절체가 이어진다. |
| [Q01 · CLI·SDK 조회 세션](../../nov-24-native-enhancement/tickets/Q01-sdk-codesearch.md#잔여-코드와-재현-조건) | 기존 Q01 담당이 검증 중 | Open/Release 불확실 결과 정리, 재생 보관 압박, 보호 경로 변경 뒤 CLI `--after` exact-view 조회가 작업 트리에 있다. 집중 코호트는 **40/41**, fixture 수정 뒤 실패한 CLI의 정확한 재실행은 **1/1**이다. 후보 합류와 영향받은 공개 회귀를 기존 Q01에서 확인한다. |
| [Q02 · 인증된 공개 범위](../../nov-24-native-enhancement/tickets/Q02-query-scope.md#작업-범위) | 기존 Q02·S01 담당 | Text preflight의 인증 `repo`·`lang` 필터와 SDK의 고정 뷰 해석을 인수한다. 남은 범위는 공개 typed `repo`·`revision`·`path`·`lang`과 S01 메타데이터 결속이다. SDK·CLI 연결은 Q01에서 인수한다. |

## 합류와 실행 인수

| 작업 | 담당과 남은 조치 |
| --- | --- |
| [04 · IFX150 게시·복구](04-ifx150-source-publication-integration.md) | 앞선 Runtime 수정과 최신 실패의 S 측 수리 커밋을 인수했다. 검증한 후속 SDK 동결본 `7626200803b`는 **compile exit 101 · 0/5**다. Regex API·현행 wire를 함께 담은 Q 공급, Q IPC 훅·S selector 합류, 실제 설치 본문이 남았다. |
| [인접 Watch V16·SDK V7 복구](../../../../../semantica-codegraph-v2/docs/plans/nov-21-architecture-audit/validation-closeout.md) | S `86ecd07699d`에서 Watch cold DELTA 호출과 서명 SDK predecessor 판독·활성화가 합류했다. Watch SDK 구현은 작업 트리 합류·검증으로, SDK Apply·Rollback의 제품 호출 연결은 기존 SDK 복구 담당으로 인계한다. |
| [05 · Lexical 페이지 순위 재사용](05-lexical-ranked-page-integration.md) | 기존 P01·TEXT 담당의 작업 트리 본문 **4/4**를 인수한다. Q HEAD 합류, 1,000파일 비용과 최종 공개 커서 검증이 남았다. |
| [Q01 · 이전 세션 수리](../../nov-24-native-enhancement/tickets/Q01-sdk-codesearch.md#확장-frontdoor-실행과-세션-수리) | 로컬 **18/18 `VERIFIED`**. 실제 CLI 17회에서 매번 pin 0, 같은 보호 경로의 프로세스 간 이어 읽기와 Release ACK 재생을 확인했다. 위 세 경계 수리와 함께 기존 Runtime 담당이 합류한다. |

## 병렬 진행과 인계

1. **01은 Store writer와 durable HEAD에 집중한다.** 02의 target Current와 같은 Source generation을 사용한다. 02는 `native_delta_seed_v3.rs`의 이미 합류한 witness 수리를 기준으로 발급·소비 경로를 바꾼다. 04에서 새로 확인된 컴파일 수정이 이 파일에 겹칠 때만 기존 SDK·Runtime 담당이 합류 순서를 조정한다.
2. **Q 공유 파일은 기존 Runtime 담당이 최종 조정한다.** Q01은 세션의 키·Release·admission, P01은 같은 세션의 순위 저장, D02는 Catalog pin·감사·회수를 맡는다. 보관 한도에서 재생 가능한 기간은 기존 Q01과 Root/F가 공개 계약의 registry lifetime에 맞춰 결정한다. Release 경합에서는 순위 메모리·Catalog pin의 1회 해제를 같은 후보로 확인한다. Q02의 범위 DTO는 이 세션 수리와 별도 변경으로 갖고 간다. [세션 재생 계약](../../../adr/NOV-07-001-native-index-execution-and-custody.md#decision)
3. **Lexical 회귀는 기존 결과를 인수한다.** 132파일·3세대 DELTA·두 차례 SIGKILL의 독립 FULL 대조와 4,097파일 Tree·Layered SDK·CLI 2/2는 로컬에서 끝났다. 남은 공개 검색 골든·취소, Off·Mixed 및 기존 릴리스와 5쌍 비교는 기존 제품 수용에서 진행한다. [Lexical 실행 원장](../../nov-20-native-cutover-closeout/tickets/07-product-qualification.md#native-lexical-rca)
4. **Semantic은 기존 B의 4K RCA를 인수한다.** 이전 ANN4K는 1/1 통과했고 Product4K는 Source 게시 뒤 owner materialization에서 1,200초를 초과했다. 후기 Record의 현재 기록은 호스트 입장 거절과 trace 0건이다. B에서 같은 후보의 지배 구간을 측정한 뒤 코드 수리 범위를 정한다. [Semantic 4K 원장](../../nov-21-bugbash-source-advance-4k/04-product-acceptance.md#현재-코드와-실행-판정)
5. **서명 설치는 기존 P07에서 `BLOCKED`다.** 외장 볼륨과 QGL 경로는 다시 열렸다. S·Q 동결 경로와 정본 Product intake가 없고, 서명은 S 4역할 영수증 대기다. 입력 확보 뒤 같은 attempt·Q operation의 ACK·G2·Source CAS writer 1회·복구 0회, Java 첫 DELTA와 독립 FULL, warm 무관 본문 0 B를 확인한다. [설치 판정](../../../bugbash/nov-20-native-delta-cost/tickets/INC-03-product-acceptance.md#현행-판정--2026-11-26)
