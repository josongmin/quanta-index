# 03 · Native Source 전수 감사와 안전 재빌드

**기존 소유자:** [D02 · Pin 정산과 손상 감사·재빌드](../../nov-24-native-enhancement/tickets/D02-pin-scrub-rebuild.md). 이 문서는 D02의 남은 통합·제품 수용 조건을 좁혀 기록한다. 새 scrub 실행기나 별도 복구 오너를 만들지 않는다. Source 원본과 cut은 [S01](../../nov-24-native-enhancement/tickets/S01-corpus-reconciliation.md), 객체 설치·게시·ACK 경계는 [D01](../../nov-24-native-enhancement/tickets/D01-durable-recovery.md), 동일 서명 후보의 최종 수용은 [V03](../../nov-24-native-enhancement/tickets/V03-signed-release.md)에 인계한다.

## 현재 경계

- **합류 인계.** Q `28784da26`의 HEAD에는 아직 `source_scrub_progress.rs`와 `source_scrub_page.rs`가 없다. 두 파일은 기존 D02 작업 트리에 있으므로 D02 담당이 그 변경을 이어받는다. 신규 구현의 시작점은 이 API의 정기 데몬 호출과 검증된 FULL 안전 절체다.
- **로컬 `VERIFIED`:** 명시적으로 선택한 exact 뷰의 bounded Source owner/body 페이지, 격리된 뷰의 maintenance pin, 재시작 후 다음 페이지, 실패한 페이지의 진행 보류와 경합 CAS를 확인했다. Catalog pin `ea172a13` 1/1, Runtime 페이지 `67075a3d` 1/1, Catalog 퇴역·진행 `616537b1` 1/1, Runtime 재시작 `3ae68f74` 1/1이다. CAS `2b88ead7` 1/1은 안정된 Q cohort에 현행 Catalog 파일 4개의 동일 바이트를 적용한 **격리 실행**이다. [페이지 API](../../../../crates/quanta-index-searchd-runtime/src/native/source_scrub_page.rs) · [영속 진행](../../../../crates/quanta-index-catalog/src/native/authority/source_scrub_progress.rs) · [D02 증거](../../nov-24-native-enhancement/tickets/D02-pin-scrub-rebuild.md#수리-절차)
- **미완료:** 이 API를 데몬의 정기 대상 선정·실행에 연결한 전수 감사, 전체 대상/미검사 범위의 관측, 격리 뒤 권위 Source의 독립 FULL 구축·검증·교체는 구현 및 설치본 수용이 남았다. 기존 진행 행은 **한 exact 뷰**에만 적용된다. 그 행의 완료를 전체 코퍼스 감사 완료로 표시하면 안 된다. 현재 작업트리의 D02 구현과 로컬 회귀를 정식 통합·서명 제품 증거로 승격하지 않는다.

## 남은 코드와 불변식

**스케줄러 선행 조건.** Catalog의 시작 API는 완료된 행도 `Conflict`로 거절하고, 페이지 API는 완료 행에서 `None`을 반환한다. `pump_loop`는 세션 sweep·retention·reclaim·owned work를 구동하며 scrub API를 호출하지 않는다. 데몬 연결 전에 보존 뷰를 유한한 페이지로 열거하는 대상 목록, 감사 주기와 실행 ID를 담은 완료·미검사 상태, 완료 행의 재감사 CAS를 같은 Catalog 계약에 둔다. [현재 진행 행](/Users/songmin/Documents/code-new/quanta-index/crates/quanta-index-catalog/src/native/authority/source_scrub_progress.rs:215) · [유지보수 루프](/Users/songmin/Documents/code-new/quanta-index/crates/quanta-index-searchd-runtime/src/native_pump.rs:517)

1. **D02 유지보수 경로에 정기 전수 감사를 연결한다.** 기존 `NativeIndexMaintenance::start_source_body_scrub_progress`와 `scrub_source_body_progress_page`를 사용한다. 보존 중인 exact 뷰와 인증된 Source cut을 대상으로 선정하고, 페이지 크기 `1..=128`, 유지보수 예약·물리 I/O scope, 다음 실행 시점과 동시 실행 소유권을 제한한다. 대상 뷰 집합, 각 뷰의 진행·완료·미검사 범위, 실패 원인을 운영자가 구별할 수 있게 기록한다. 재시작 때는 같은 뷰의 마지막 커밋 커서만 재개한다. 새 Source cut은 새 대상으로 취급하고, 퇴역한 뷰의 진행을 다른 뷰로 이전하거나 완료로 위장하지 않는다.
2. **페이지·격리의 실패 의미를 유지한다.** 인증된 Source owner map과 선택 뷰를 매 페이지 대조하고, 격리된 뷰에도 유지보수 pin만 허용한다. 일반 조회의 격리 거절은 유지한다. 모든 선택 행의 record와 body를 검증하고 손상 객체를 참조하는 뷰를 영속 격리한 뒤에만 진행 CAS를 커밋한다. 취소·기한·예산 거절, 객체 누락/변조, 잘못된 cursor, 경합 또는 프로세스 중단은 정상 0건이나 전수 완료가 아니다. 실패한 페이지는 마지막 커밋 위치에서 재시도하며, 이미 격리된 뷰에서도 후속 손상을 찾는다.
3. **권위 Source에서 새 incarnation을 독립 FULL로 만든다.** S01의 인증된 파일 집합·본문 digest·Source cut을 고정하고, 손상된 Q 객체를 입력으로 재사용하지 않는다. 별도 인스턴스에 FULL을 구축한 뒤 독립 manifest의 경로·바이트·해시와 공개 SDK 검색 결과를 대조한다. 검증된 후보만 예상 head에 대한 CAS로 교체하고, 동일 요청·cut의 재시도와 ACK 유실에서 게시 receipt가 하나인지 확인한다. 교체 전 실패·ENOSPC·stale head에는 기존 정상 뷰와 pin을 보존한다. 교체 후에도 옛 격리 뷰를 임의로 정상화하지 않고, pin 해제와 보존 floor를 지난 객체만 회수한다.

## 합격 증거

- **기능:** 여러 뷰와 두 개 이상의 손상 객체를 가진 서명 설치본에서 주기 감사가 전체 대상·미검사 범위·완료를 정확히 보고한다. 손상 발견 뒤 서비스를 강제 종료·재시작해 같은 cut의 다음 페이지를 처리한다. Source record 길이/본문 불일치, 삭제·같은 크기 변조·symlink 대체, 외래 cut·퇴역·동시 실행을 포함한다. 정상 뷰 검색은 지속되고 격리 뷰의 일반 조회는 거절된다.
- **복구:** 같은 승인 Q/S 후보에서 독립 Source manifest와 FULL 결과를 대조하고, CAS 전·후 중단 및 ACK 유실을 재실행한다. 공개 SDK의 파일 집합·점수/결과를 독립 정답과 비교한다. 구형 root의 `RebuildRequired`와 위조 root의 `CorruptObject`를 구별한다. 혼합 결과, 중복 교체 receipt, 손상 후 정상 0건은 실패다.
- **운영:** 다른 활성 pin을 보호한 채 마지막 pin 해제 뒤 [E07](../../oct-22-finalizing/E07-performance-operations.md)의 60초 물리 회수와 디스크 압박 복귀를 같은 설치 후보에서 측정한다. 현재 로컬 pin·Layered 회수 통과는 이 제품 측정을 대체하지 않는다. 위 세 설치본 범위는 현재 `NOT_RUN`이다.
