# P08 · Sidecar 부모 디렉터리 정책 통합

생성 fallback의 동일 디바이스 검사와 Semantic Sidecar의 held-FD 후속 수정은
`VERIFIED`. [Runtime RCA](../../nov-5-indexing-bugbash/RCA.md)

## 문제와 수정 범위

[PublicationFileCapacity](../../../../crates/quanta-index-searchd-runtime/src/publication_file_capacity.rs)의 기존 부모 탐색만 root와 같은 디바이스를 확인했다. 부모가 없는 경로에서는 생성 fallback이 이 정책을 적용하지 않았다.

- 기존 탐색과 생성 fallback이 디렉터리 type·권한·owner·root device 검사를 공유한다. Sidecar는 같은 namespace lock에서 부모 생성과 path claim을 마치고 검증된 FD를 사용한다.
- 경로 거절은 원래 ticket을 소비하기 전에 일어난다. 수정 범위는 Sidecar의 `begin_create_with_reserved_output_in_private_parent`다.
- 기존 SQLite VFS 관측 테스트를 현재 Core 오류 계약과 맞췄다. `BeforeOpen`의 SQLite capacity 분류와 요청·잔여 용량 수치를 모두 비교한다. [SQLite VFS 테스트](../../../../crates/quanta-index-searchd-runtime/src/publication_sqlite_vfs/tests.rs)

## 검증

- 동일 디바이스의 기존 부모와 신규 nested 부모에서 예약했다. 기존 symlink·unsafe mode·부모 교체·busy 경로 거절과 Sidecar 정산 회귀가 통과했다.
- macOS 분리본의 private root 아래에 별도 HFS+ 디스크 이미지를 마운트해 실제 다른 `st_dev`를 사용했다. 기존 부모 탐색과 생성 fallback에서 거절했고, 외부 디렉터리 생성과 추가 차감이 없었다. 같은 ticket으로 동일 디바이스에 다시 bind해 예약을 정리했다.
- 2026-11-08에 `./scripts/cargow nextest run --locked --offline`으로 private parent·Sidecar 10건, SQLite VFS 1건, Core byte/file 거절 2건을 선택·실행해 13/13 통과했다. 실제 mount 회귀 1건은 분리본에서 실행했다.

## 후속 수정

- **P1 · bind 이후 부모 교체.** 기존 pathname cleanup은 교체된 부모의 동명 파일을 지우고 원래 임시 inode를 남길 수 있었다. 예약된 Semantic 작성자는 `PublicationOutputPermit`으로 Runtime의 held-FD 생성·rename·취소를 사용한다. 정리는 원래 writer와 임시 inode를 대조한 뒤 수행하며, rename 성공 뒤 sync가 실패하면 해당 inode의 pending custody를 보존한다. [Semantic 작성자](../../../../crates/quanta-index-semantic/src/durable_write.rs)
- **같은 ticket 재시도.** staging 선언이나 Control 복구 등록이 실패하면 시작 전 child를 원래 permit으로 반환하고 실패한 bind의 path·inode claim을 해제한다. 게시 이후 이동은 기존 `PathTicket`의 FD rename과 별도 정산을 사용한다. [Sidecar permit](../../../../crates/quanta-index-searchd-runtime/src/publication_sidecar.rs)

2026-11-08에 현재 작업본에서 `./scripts/cargow --lane shared nextest run --locked
--offline -p quanta-index-semantic -p quanta-index-searchd-runtime --lib`의 집중 회귀
36건을 선택·실행해 36/36 통과했다. 생성 전 부모 교체 거절과 복원 후 재시도,
생성 후 동명 파일 보존과 원래 임시 inode 정리·환급, staging 충돌과 Control
등록 거절 후 같은 permit 재시도, 디렉터리 간 이동의 inode 보존을 확인했다.
기존 Native Object 취소·부모 교체와 Semantic promotion 실패 회귀도 통과했다.

같은 날 Native Object 변경 이후 Sidecar의 시작 전 부모 검사를 현재 공용 검증에
다시 연결했다. 동일 selector 36건이 다시 통과했다. Nextest run
`616f24d4-e746-4c12-b7a0-0ddab44cb2bc`.

후속 하드닝에서 다른 디바이스의 기존 staging 부모가 bind를 통과하는 것을
재현했다. Sidecar는 이제 bind 중 held staging·target 부모의 device를 비교하고,
거절 시 기존 rollback으로 시작 전 ticket을 반환한다. 같은 부모 FD를 복제한
경로는 추가 stat 없이 통과한다.

macOS의 별도 HFS+ 이미지로 새 회귀가 수정 전 1/1 실패하고 수정 후 통과했다.
`QIDX_P08_TEST_IMAGE`를 설정하고 `--run-ignored all`로
`foreign_device_staging_bind_refuses_and_keeps_ticket`을 선택한다.
기존 집중 회귀와 합쳐 37/37이 통과했다. Nextest의 기존 테스트 `LEAK` 표기
1건은 같은 소스에서 단독 재실행해 `PASS`였다. Nextest run
`2b811a26-9d6c-48d4-9eb8-fe4471425b70`, 단독
`b5d9ec0c-082e-4f7e-bd0c-2f96886e77d6`.

reader pin 한도에서 Sidecar의 게시와 용량 정산은 성공했지만 lease 이전이
거절되면 남은 writer pin과 namespace claim이 고착됐다. 이전 거절 시 원본
pin을 checked close하고 정산된 mutation을 해제한다. 이미 게시된 inode의
용량과 원래 lease 오류를 보존한다.

동일 분리본에서 `committed_output_releases_claim_after_lease_ceiling_refusal`은
수정 전 `delete path busy`로 실패했다. 수정 후 Semantic·Runtime 집중 회귀
38/38이 통과했다. 분리본은 당시 Lexical 타입 캐스트의 컴파일 문제만 따로
보정했으며 현재 P08 소유 소스와 해시가 일치했다. Nextest run
`3e3993f0-4296-4043-8603-df28acac309c` 및
`2b099bea-b765-4450-9aff-22f8f8a54b39`.

후속 감사에서 bind rollback, 생성 거절 정리, writer close의 보조 오류 문맥을
binder의 기존 `append_failure`로 통합했다. 원 오류의 분류와 메시지는 보존한다.
현재 작업본의 `cargow nextest run`에서 실제 HFS+ mount 회귀를 포함한 P08 38건,
P04 검증 4건, P09 로컬 IPC 7건이 모두 통과했다. Nextest run
`20c73bdc-2e04-4a3a-8ca7-534601d7c583`.

bind 이후 부모의 inode가 유지돼도 쓰기 권한이 바뀔 수 있다. Sidecar는
부모 이동과 생성을 같은 정책으로 검증하고, namespace move ticket은 검증한
부모 FD를 보유한다. Sidecar 생성·게시·정산과 외부 writer 훅에서도 현재
부모의 소유자·권한·장치를 확인한다. `chmod` 거절 뒤 원래 ticket 재시도,
실제 mount 거절, Source Upload·RepoMap을 포함한 통합 회귀 93/93이 통과했다.
Nextest run `4eb023e4-a5af-462e-b5b0-0d24e8cf9a84`.

완료된 Sidecar 삭제도 기존 `DeleteTicket`에서 private parent를 검증한다.
unlink 전 거절은 원래 permit을 보존한다. unlink 후 정산이 거절되면
같은 ticket이 보존되며 일반 삭제 호출이 그 결과를 소비할 수 없다.
`./scripts/cargow --lane shared nextest run`의 집중 실행은 37/37 통과했다.
실제 unlink 뒤 일반 삭제 거절, 부모 `chmod` 거절, 권한 복원 후 같은
ticket의 최종 환급을 검증했다. Nextest run
`ccf3939f-520d-4163-aa01-690c9fc893bd`.

Sidecar rename과 일반 rename의 대기 작업을 구분한다. 이미 rename된 Sidecar
티켓은 새 호출이 가져갈 수 없으며, 명시적 복구 경로가 기존 FD로 정산한다.
원래 출력 핸들은 같은 대상에 대해 정산만 재시도할 수 있다.
2026-11-09 집중 실행에서 실제 rename 후 교차 재진입 거절, 부모 권한 복원 후
원래 티켓 정산, 대상 변경 거절과 inode 보존을 확인했다. Go 조회를 포함한
81/81 통과. Nextest run `d5d5a9cc-51ce-4c76-9b48-a44412fd0a9e`.
