# 05 · 포크 잔존 검사, 제품 수용과 절체

상태 **NOT_RUN**. Root/F가 01–04의 포크 제거 결과와 07–09의 엔진 수용을 같은 출하 후보에 모아 기존 E07/E08과 설치 큐에 넘긴다. 이 문서는 출하 판정 경계를 정의한다.

## 합류와 회귀

1. 고정 Q/S revision·lock·target·feature와 실제 출하 빌드 입력을 결속한다. Q workspace와 영향받은 S workspace에서 `cargo metadata`의 패키지 source·manifest 경로, Cargo.lock의 registry/checksum, compiler 산출물의 대상 입력을 대조한다. `./scripts/cargow metadata --locked --format-version 1`와 `./scripts/cargow tree --workspace --all-features --target all --locked`는 Q 측 인벤토리 확인 명령이다. 실제 출하 target/feature, 테스트·벤치·fuzz·no-default의 resolved 입력도 확인한다. source/config·CLI override와 캐시에 있던 바이너리가 검증을 우회하지 않아야 한다. [Cargo source 식별](https://doc.rust-lang.org/cargo/commands/cargo-metadata.html) · [Cargo override 경계](https://doc.rust-lang.org/cargo/reference/overriding-dependencies.html)
2. Cargo root·nested·CI bootstrap·source-closure·test authority·deny·Just·Python 실행 경로의 사라진 파일 참조를 정리한다. 외부 의존성과 무관한 테스트나 historical 문서 근거는 기존 권위에 남긴다. 삭제 예정 13개와 Unicode 자료의 이동 경로를 최종 소스에서 대조한다.
3. 소유 검사에는 기존 `just rust-test-authority`, `just rust-wire-inventory`, `just rust-fuzz-smoke`와 `just rust-profile test-integration-storage`·`test-integration-semantic`·`test-daemon` 중 변경 범위에 해당하는 경로를 사용한다. 최종 Q는 `just rust-profile verify-rust` 및 출하 검증으로 묶는다. 세부 명령의 선택자는 실행 전 현재 Justfile과 등록 인벤토리에서 확인한다. [Justfile](../../../Justfile) · [테스트 등록](../../../tools/ci/test-authority.toml)
4. 기존 승인 바이너리와 포크 없는 후보를 같은 입력·적용 범위에서 비교한다. SDK60의 Caddy·etcd, Large·XL, ANN4K·제품 4K, E07 꼬리 지연·freshness·RSS·물리 사용량과 signed 설치·복구를 현재 원장에서 받는다. 실패했던 기존 4K 단계는 기초 진단으로만 쓰고 후보의 당일 본문으로 통과 여부를 결정한다. [E07](../oct-22-finalizing/E07-performance-operations.md) · [제품 원장](../nov-20-native-cutover-closeout/tickets/07-product-qualification.md) · [설치·활성화](../nov-20-native-cutover-closeout/tickets/08-release-activation.md)

## 엔진 기준선과 포크 전환 비교

| 비교 단계 | 실제 입력과 판정 |
| --- | --- |
| 기존 실패 재현 | 기존 포크 후보에서 [07 Product4K](07-engine-product4k.md)의 최초 실패 단계와 [08 Source DELTA](08-engine-source-delta.md)의 제품 불변식을 기록한다. 컴파일 실패, admission 거절, 본문 실패, 시간 초과를 구분한다. |
| 포크 유지·엔진 수리 기준선 | 4K·Text 수정과 V2 연결을 동일 Q/S 후보에 고정한다. 첫 게시, SDK 변경, 기존 읽기 뷰·재시작, 07·08의 독립 기준을 실행한다. 이 후보가 실패하면 포크 제거로 인한 성능 회귀율의 분모로 쓰지 않는다. |
| 포크 제거 후보 | 위 기준선과 동일한 엔진 수리·기능 범위에서 포크 교체의 코드·lock·파일 수명·쿼리 결과를 대조한다. 파일 포맷·quota 변경은 실제 의미와 동등성을 먼저 잡고 비교한다. 서로 다른 제품 기능이 섞이면 동일 코호트의 비교가 가능한 실행으로 다시 구성한다. |
| 최종 서명 후보 | [09 검색·출하](09-engine-query-and-release.md)의 동일-view 품질, SDK/CLI, E07과 설치 복구를 기존 담당 실행에서 인수한다. 07과 08에서 바뀐 흐름을 최종 후보가 포함하는지 확인한다. |

공통 코퍼스·필터·모델·provider·parser 정책·qrels, 쿼리와 후보 깊이, Work 상한, 워밍업과 부하, 호스트·스토리지, feature와 seed를 기록한다. Text와 Semantic을 둘 다 켠 4K 본문, Lexical만 쓰는 별도 Large 후보, 랭킹 합성 fixture는 결과를 각각 원래 분모에 귀속한다. 기준선과 후보가 둘 다 실패하면 공통 실패일 가능성을 검토하고 동일 단계 증거를 먼저 대조한다. 후보에서 처음 생긴 실패는 해당 01–04 변경의 최초 달라진 경계부터 조사한다.

## 구·신 버전과 상태 조합

| 실물 조합 | 확인할 결과 |
| --- | --- |
| 이전 S/Q + 이전 승인 Root | 기존 공개 조회·게시 결과와 남아 있는 상태의 기준을 수집한다. Source·Text·Semantic·History, 사용 중인 overlay/Graph 지원 범위를 구성대로 기록한다. |
| 새 S/Q + 이전 승인 Root 사본 | 정상 부팅에서 기존 exact/Latest 조회, cursor·ID 규칙, 삭제·retention 상태와 동일 operation replay를 확인한다. 포맷 파괴 변경을 승인한 경우에만 이 본문을 **승인 이관 → 새 Root → 데이터·검색·영수증 대조**로 전환한다. |
| 새 S/Q + 새 Root | FULL·DELTA·Watch → ACK → SDK/CLI 검색과 새 PID를 확인한다. SQLite·Lance 각각에 실제 quota 거절, worker 종료, ACK 유실을 주입하고 같은 operation 재개 후 중복 게시·유실·회수 부채를 확인한다. |
| 이전 S → 새 Q / 새 S → 이전 Q | 현재 지원하는 혼합 조합은 실제 SDK 요청으로 호환을 확인한다. 지원 종료를 승인한 조합은 연결·설치 선택 또는 요청 admission에서 명시적으로 거절하고 Stage, Catalog, Source, published head에 변경 0을 확인한다. 호환 거절을 최초 쓰기 이후로 미루지 않는다. |
| 새 ABI/설정과 이전 설정·운영 도구 조합 | 기존 설치 입력과 옛 설정의 실제 호출자를 대조한다. 퇴역한 VFS·파일별 한도나 진단 옵션은 롤아웃 전에 소비자가 새 정책으로 이행했는지 확인한다. 잘못된 Root 또는 집행되지 않는 quota를 선택한 경우 활성화를 거절한다. |

## 배포와 복구

- 먼저 이전 승인 상태 복사본이나 격리 설치에서 새 바이너리로 재오픈·게시·검색·재시작을 확인한다. quota가 실제 대상에 집행되지 않거나 기존 상태 형식이 읽히지 않으면 활성화를 보류한다.
- 배포 담당이 게시자를 멈추고 기존 쓰기 차단·정산으로 pending operation과 마지막 ACK frontier를 확인한다. 이전 서명 실행물·선택·Root/체크포인트를 같은 경계로 확보하고 기존 방식으로 후보 선택을 절체한다. 새 Root에서 첫 durable 게시가 생기기 전에는 마지막 ACK와 대조해 이전 승인 선택을 복구할 수 있다.
- 첫 durable 게시 이후에는 ACK 송신 여부와 별개로 신규 확정 상태가 있을 수 있다. 새 쓰기를 차단한 뒤 Catalog의 실제 head와 operation 결과를 먼저 확인한다. 기존 승인 restore-forward가 새 frontier를 모두 보존한다는 실물 증거가 있을 때만 이전 실행물을 다시 사용한다. 그 증거가 없으면 이전 Root로 단순 복귀하지 않고 새 선택을 차단·복구한다. [상태 절체 런북](../../operator/state-cutover-runbook.md)
- RepoMap·Tantivy 퇴역과 동일 릴리스에 합류하면 같은 후보의 유효한 기존 실행을 인수한다. 변경된 영향 경로는 다시 검증하고, 포크 제거 자체를 이유로 관련 없는 작업을 재접수하지 않는다.

## 완료 조건

- [ ] 레포와 차기 출하 입력에 13개 대상 사본·경로 override·포크 전용 연결이 0개다. Q/S 잠금 빌드와 기존 등록 검증이 통과했다.
- [ ] 동일 최종 후보에서 ABI·wire 호환, 기존 상태 재오픈, SDK 게시·검색·재시작, ACK 유실 복구, 용량 초과 및 마지막 pin 뒤 파일·FD 회수 본문이 통과했다.
- [ ] 지원 Linux 프로필의 하드 한도와 E07 제품 조건, 실제 서명 설치·절체·복구 결과를 기존 릴리스 원장에 인수했다. 실행되지 않은 검증은 `NOT_RUN`, 입장 입력 부재는 `BLOCKED`로 남긴다.
- [ ] [파괴 변경 판정표](06-breaking-change-matrix.md)의 모든 영향 항목에 실행 증거, 대체 구현 또는 의도한 퇴역 및 소비자 이관이 연결돼 있다. 지원 버전 혼합에서 부작용 없이 호환하거나 거절하고, 첫 durable 게시 전후의 복구 선택을 각각 검증했다.
- [ ] 07–09의 잔여 엔진 작업을 해당 원장에서 종료했다. 기존 실패의 수리 결과와 포크 전환 회귀의 검증 결과가 같은 출하 범위 안에서 각각 추적된다.
