# 03 · 저장 참조와 물리 퇴역

S Store, Q Catalog/GC, D/H가 맡는다. 새 후보의 논리 퇴역과 이전 root의 물리 회수는 각각 승인한다. 이전 설치의 root·백업·선택·서명·실행물 보존이 남으면 물리 회수는 **BLOCKED**다. [보존 결정](../../adr/NOV-23-001-repomap-retirement-and-source-custody.md) · [설치 수용](04-integration-acceptance.md#설치-절체와-중단-지점)

## 실제 소유 그래프

| 대상 | 회수 전 확인 |
| --- | --- |
| S Required·V9·ACK·CAS | active와 historical ACK, archive, 준비·보존 Source image, CAS hold와 reader pin, outbox intent·activation·selector를 확인한다. 이전 Required를 정산하고 archive가 exact ACK seal·원 Required 객체를 재생하는지 본다. |
| Q Native Graph와 Legacy imported | Native Graph의 owners/nodes/edges/incoming과 node support→rank, imported header/pages 및 nodes/edges/incoming/rank_rows를 보유하는 member·projection·view·BOOT를 확인한다. prior-file/endpoint는 같은 root의 조회다. 공용 Graph와 FileMap의 유지 참조를 보존한다. |
| Q Legacy 저장과 공유 저널 | `repomap_candidate_v1`, `repomap_activation_v1`, `repomap_quarantine_event_v1`와 Legacy 물리 파일은 Native GC 밖이다. 공유 `idempotency_v2`는 RepoMapBundle과 유지 ingest를 함께 보유한다. 이전 설치에서 미결·terminal receipt·유지 행을 재생한다. 전용 row 삭제 경로는 확인되지 않았다. |

### Q 회수 입장

- 이전 Native V5 `Required`는 pinned config에 있는 saved RepoMap 선택 경로와 SHA, locator, 게시·활성화 ACK를 읽는다. RestoreForward가 해당 파일과 이전 Root를 검증하므로 새 active selection의 성공만으로 saved selection, 전용 Root 또는 백업의 보존 기간이 끝나지 않는다. 운영 D/H가 복구 창 종료 승인과 active·saved selection, BOOT·백업의 참조 해제를 확인한다. [이전 Required 검증](../../../crates/quanta-index-searchd-runtime/src/native_boot/restore_forward/source_only_catalog_v7.rs#L488)
- 기존 `forget_generation`은 repo·revision·generation 전체의 operation kind를 지우고 invalidation을 쓴다. RepoMap과 유지 ingest가 같은 세대에 있을 수 있다. Q Catalog/GC는 전용 파일과 저장 행, 공유 view/member·BOOT 참조, reader/writer lease를 각각 정산한다. S Source lease와 FNL410은 기존 Store·S 오너가 판정한다. [공유 정산](../../../crates/quanta-index-catalog/src/idempotency.rs#L2380) · [기존 GC 호출자](../../../crates/quanta-index-search-plane/src/ingest_dispatcher/search_corpus.rs#L1589)
- **BLOCKED.** 이전 selection·Root inode와 quota project, BOOT·백업·replay의 보존 범위, 보존 종료 승인, 활성 reader/writer·FNL410 lease 해제 관측, 공유 Catalog의 유지 행 및 전용 파일 목록이 필요하다. 현재 기록에는 실제 설치 CAS와 보존 종료가 없다. [서명 절체 인수](04-integration-acceptance.md#설치-절체와-중단-지점)

## 실행 순서와 실패 시 경계

1. **보호 중인 쓰기를 정산한다.** 이전 승인 Root의 새 쓰기를 차단하고 Pending Required를 원 writer·원 요청으로 정산한다. Source lifecycle lease는 memo 캡처부터 Source CAS·live/historical ancestry·closeout까지 보유한다. 기존 FNL410의 Source CAS·RecordOnly ancestry 두 exact를 실행하고 별도 PID의 EX 대기·SH 해제를 실제 저장에서 관측한다. [Source lifecycle 수용](04-integration-acceptance.md#실제-본문과-통과-조건)
2. **독립 root라면 전체 퇴역 가능성을 입증한다.** 새 PID의 Source·History·유지 검색이 승인된 뒤 이전 root의 rollback/복구 창, 선택·백업·재생·reader와 열린 writer·FD를 확인한다. namespace·mount 및 공유 CAS/Catalog를 실제로 대조한다. 서명된 설치와 승인 보존 범위가 불명확하면 회수를 보류한다.
3. **공유 root는 전용 참조부터 해제한다.** `Reconfigure`가 `projections`를 보존하며 빈 `ProjectionAdvance`도 certificate·Source를 보유한다. 운영 절체가 이전 BOOT를 해제한다는 근거는 없다. 소유 Catalog에서 전용 member·projection·BOOT 참조를 실제로 해제하고, floor·pin·work·BOOT 조건을 충족한 view의 root edge를 정산한다. GC가 view·operation·ingress·work·bootstrap 및 부모를 대조한 뒤 restart-safe unlink/drain으로 넘긴다. 안전한 해제 경로가 없는 범위는 **BLOCKED**다.
4. **S 물리 삭제는 기존 certified retirement와 quota를 사용한다.** exact Required가 archive에 보존된 뒤 signed `Deleting` census와 writer/reader gate를 확인한다. 기존 inode의 transfer/issuer marker·token·aggregate body unlink/sync는 준비 aggregate 본문에 적용된다. Required CAS는 `delete_scoped`의 설치 quota 거절을 존중하고, Store certified retirement에서 마지막 참조·retained FD·재시도·quota를 같이 정산한다.
   V49의 공용 non-RepoMap Required 회수는 exact Cleaned와 서명된 census에서 대상 leaf를 선택하고, durable `Deleting`·writer의 no-follow 검증·unlink/fsync·`Deleted` CAS를 결속한다. 기존 RepoMap root 회수에는 이 장의 독립 root·공유 참조·보존 조건을 적용한다. [V49 Store](../../../../semantica-codegraph-v2/packages/analysis/quanta-v2/crates/quanta-runtime/src/indexing_machine_v2/store/native_required_member_reclaim_v1.rs) · [writer unlink](../../../../semantica-codegraph-v2/packages/analysis/quanta-v2/crates/quanta-runtime/src/retrieval/port_impls/index_projection_writer/prepared_commit_artifact_v2/source_bound_required_member_unlink_v1.rs) · [공용 회수의 실행 분모](../../../../semantica-codegraph-v2/docs/plans/oct-26-replay-query-read-custody/tickets/CURRENT_REMAINING.md#a-native-non-repomap-audit-20261123)
5. **Legacy 파일과 SQL은 별도 승인한다.** 이전 실행물 또는 승인 변환이 공유 저널의 RepoMapBundle·유지 범위를 읽는다. shared SQLite 파일을 유지한다. 전용 table 작업이 필요하면 schema/복구/백업/재시작을 포함하는 migration으로 승인한다.

## 완료 조건과 회수 증거

- 새 참조가 차단됐는지 S writer/selector와 Q Native·Legacy 정책을 통해 다시 본다. 승인 이전 설치에서 V9·Required·Q projection·exact 공개 결과를 재생하고, 신규 서명 설치에서 RepoMap receipt·binding·보호 route 0과 유지 Source·History를 확인한다.
- 객체·경로별 소유/공유, 정확한 참조, 보유 사유와 해제 근거를 기록한다. 외래 root·서명, 누락 ACK·미완료 outbox·active pin·stale view를 거절하고 중단된 unlink를 안전하게 재시도한다.
- 물리 파일·할당량 및 quota 전후를 대조한다. 전체 독립 root를 퇴역하면 전체 양과 RepoMap 귀속분을 기록한다. 유지 설치를 재시작해 참조와 용량을 다시 본다.
- 설치 CAS 링크 전 중단 회귀 `installed_quota_recovers_pending_reservation_from_actual_publication_v5`와 V48 signed SQL 314의 `cleaned_v9_memo_hold_retirement_batches_only_its_covered_owner_v1`는 기존 FNL410에서 수용한다. 최신 후보의 Rust·설치 본문은 **NOT_RUN**이다.
