# 구현·검증 계획

## B03-B2 — owner-local block의 물리 설치 단위

계약과 코드 합류 완료, f20 통합 집중 검증은 admission 대기·결과 NOT_RUN이다. a608의 typed fee 수리와 f19 집중 회귀는
통과했지만 r6 Medium도 bytes cap에서 실패했다. 다음 후보는 제품 예산을 그대로 두고
owner-local occurrence block의 한도를 **8,192B / 512 canonical terms**로 바꾼다.
소스 기반 seed42 투영은 Medium512→256개, Large12,288→4,096개 block이다.
이것은 물리 객체 수·payload의 추정이며 설치 호출 수나 속도 개선 증거가 아니다.
16KiB 한도는 이 입력에서 추가 이득이 없어 제외한다.

- QITXOC01의 기존 단일 wire/reader를 유지하고 bounds를 output policy에 결속한다.
  직전128행 policy와 leaf-only policy는 typed refusal로 검증한다.
- owner를 섞지 않는다. 동일 positions vector는 한 block에서 공유한다. singleton,
  oversize, nonempty-prior Patch의 canonical leaf와 unchanged old ref 보존은 유지한다.
- Full은 원래 owned 입력을 반환한다. 취소·memory/work 거절을 Full로 바꾸지 않는다.
  모든 metadata/payload/position allocation을 기존 grant로 입장시킨다.
- 하나의 physical ref에 512개 selector가 연결돼도 64행 SQL flush와 aggregate
  planned-confirmation receipt의 정확한 row/ref binding을 보존한다.
- code lane은 codec+codec oracle / producer+policy oracle / Kernel realstore oracle로
  분리한다. 각 lane은 Cargo와 외부 제품 실행을 하지 않고 Root가 검증을 직렬화한다.
- 집중 수용: literal wire와512행·8KiB 경계, cancellation/memory refusal, 64/256행을
  넘는 fanout, public reader의 첫/마지막 selector, delta·old pin·reopen·GC다.
- 제품 수용: Medium1M/512MiB, Large dispatch120초, 기존 query work/memory/SLO를
  유지한다. query64 cache의 더 큰 metadata와 cold retained-read bytes를 보고한다.
  객체 수 감소만으로 채택하지 않으며 외부 기존 채팅의 ANN/SDK 경로를 재구현하지 않는다.

합류 소스는 외부 stable `s3`의 `878be6c9e4e33dd35b9c80cc5b450c2cf3f6f099`다.
이전 a608 대비 codec/producer/reader gold/독립 oracle의6개 파일이 달라졌고, 소유
입력15개를 고정했다. 새 output policy(admission `[31;32]`)는
`b2c1df18b452485d69ef4e687b7a3519f07cfff76fe25f0a665d8706258d7d95`다.
직전 `eef15862…`(4KiB/128행)와 `559e0482…`(leaf-only)는 typed refusal
oracle로 남긴다. 두 이전 policy의 자동 변환·fallback은 없다. f20 Native/Kernel
batch는 queued 상태이며 아직 compiler/body/terminal 결과가 없다. r7 제품 실행은
미시작이다. 이 source에 f19/r6 증거를 승격하지 않는다.

현행 RCA와 범위는 [README](README.md)와 [RCA](RCA.md)가 소유한다.
이 문서는 새 패킷의 계약과 완료 조건이다. B01 계약은 확정했고 B02 생산자·소비자와
독립 테스트 소스를 합류했다. B02의 Core331개·Text154개와 Python1,040개 영향 회귀는
통과했고, strict Clippy와 실제 생산자→소비자 캡처는 아래 상태를 따른다. 새 mr2에서는
실패-prefix6계수와 capture health를 확인했으나 제품 full ACK는512MiB cap에서 거절됐다.
RR 후속 counter 종료 순서 결함은 canonical parser와 공통 summary reader에서 수리했다.
관련1,024개 통과·2개 skip은 이 검증기 범위의 완료 근거다. B01–B04의 성능 구현·수용
상태는 각 패킷과 README의 원본 가용성 판정을 따른다.

## 고정 제품 계약

- Large4,096-source SourceAdvance의 서버 dispatch120초와 기존 명시적 work caps를 사용한다.
- 정상 ACK 뒤 current/old search, delta/noop/delete, drain/reopen을 검증한다.
- FULL·auto1000, GC·reserve·physical capacity와 원 memory/work 거절 계약을 적용한다.
- identity, 실제 payload 형식, root/namespace 현재성, cancellation과 오류 settlement를 검증한다.
- Medium은 동일 UDS 경로의 진단 입력이다. Large 완료와 정식 overhead를 각각 판정한다.

<a id="execution-plan"></a>
## 확정 실행 순서 — 2026-11-07

목표는 원 Large120초에서 full ACK와 후속 정합성을 끝내는 것이다. 계측이나 테스트
통과만으로 목표를 닫지 않는다. 기존 B01/B02를 다시 구현하거나 새 티켓을 만들지 않는다.

| 순서 | 할 일 | 반드시 남길 결정/결과 |
| --- | --- | --- |
| 1 | normal cached build와 Medium Record 실행 완료 | build exit0·mr5 fixed512MiB 실패-prefix4계수/health 확인. CLI `build_binding=UNPROVEN`과 managed-fresh/제품 성공으로 승격하지 않음 |
| 2 | 같은 binary pair로 Large seed42 Off를 원120초·기존 caps에서1회 | lo5 `BuildPrepare` deadline FAILED. full ACK·후속 정합성 미도달, numeric cap 거절 없음 |
| 3 | 비용과 실제 생산자를 대조해 A/B 또는 실제 owner 수리·보류를 판정 | 수정할 함수, 없앨 정확한 작업, 예상 절감 단위, 권위/수명 계약, 반증 oracle를 고정. 여러 가설을 동시에 구현하지 않음 |
| 4 | 공통 계약을 먼저 합류하고 생산자·소비자·독립 oracle를 병렬 구현 | 아래 독점 범위 준수. Root만 공통 타입·parent/spool 접점과 통합 수정 |
| 5 | 코드 합류 뒤 영향 검증을 한 큐에서 실행 | 정적 대조→compile/소유 oracle/strict→정상 binary1회→Medium Record1회→Large Off1회 |
| 6 | 원 계약으로 완료 판정 | 변경 후 Large120 full ACK와 후속 정합성이 실제 통과. 비용 수리의 인과 비교와 정식 tracing overhead는 각각 별도 판정 |

Medium은 진단이며 fixed512MiB의 성공을 별도 explicit funding인 Large의 실행 선행조건으로 두지 않는다.
Medium 실패를 보존하고 원 Large caps로 독립 판정한다. 파일 수16배·bytes23.42배인 Large의 시간 예측으로 쓰지 않는다.
첫 Large가 deadline 실패하고 Medium만으로 동일 경로를 고를 수 없을 때에만 같은
binary의 Large Record1회를 추가한다. 실패한 source·입력·호스트 조건을 그대로 둔
반복 실행은 하지 않는다. 새 결과가 기존 변경으로 목표를 충족하면 불필요한 B03은 폐기한다.

Root는 정상 binary pair의 원본 build receipt·SHA·고정 source를 함께 보존하고 같은
cohort의 관측에는 재사용한다. 각 관측마다 fresh managed build를 다시 하지 않는다.
prebuilt CLI가 build_binding=UNPROVEN으로 남기는 제한을 임의로 고치거나 정식
qualification으로 승격하지 않는다. 제품 ACK/정합성 결과와 해당 build-binding의
한계를 분리해 기록한다. 제품 코드가 바뀐 경우에만 새 cohort와 정상 binary를 만든다.

## 중복 제외와 소유 경계

새 작업 전 HEAD·dirty hunk·호출 경로를 읽고 기존 담당의 의미상 수정과 비교한다.
파일이 같아도 독립 함수면 분리할 수 있지만 같은 hunk·authority·proof rail은 합류 owner가
한 번만 변경한다. 기존 담당의 실시간 활성 여부는 이 문서 작성만으로 확인되지 않는다.

| 기존 영역 | 이번 패킷에서 소비할 결과 |
| --- | --- |
| Native Text256 packet·Small/Block·inline·unchanged·exact dedup | 현재 구현과 독립 oracle 재사용 |
| occurrence count memo/spill, Kernel read_retained 위임 | 현재 호출 경로와 grant 경계를 먼저 확인 |
| fresh claim·allowance·Catalog/VFS·checkpoint·GC | 기존 소유자 수정 보존. 이 패킷을 이유로 재구현하지 않음 |
| producer/DELTA·SDK·ANN·overlay·Watch·release | 해당 기존 owner와 E01–E08 수용 경계 |
| Python child-root 수리 | 현재 코드와 과거 consumer a41830d의 영향 결과 재사용. 외부 consumer 컷은 현재 부재 |

<a id="b01"></a>
## B01 — 계수와 유효성 계약

상태는 CONTRACT_FIXED이며 Root가 계약과 공용 접점을 소유한다.

### B02 현행 계수 계약

기존 NativeTextInventoryDiff lane2 span 하나에 다음6개 u64를 각각 한 번 기록한다.
Record에서0은 관측된0이고, counter family가 없으면 미관측이다.

| Core counter / wire key | 의미 |
| --- | --- |
| TextOccurrencePriorValidationAttempts / text_occurrence_prior_validation_attempts | prior 쪽 full payload count 검증 함수 호출. 거절·실패도 호출 수에 포함 |
| TextOccurrenceNextValidationAttempts / text_occurrence_next_validation_attempts | next 쪽에서 RAM/spill 재사용으로 끝나지 않아 full count 검증 함수를 호출한 수 |
| TextOccurrenceRamReuses / text_occurrence_ram_reuses | 같은 attempt의 이미 검증된 RAM count 반환 |
| TextOccurrenceSpillReuses / text_occurrence_spill_reuses | 같은 attempt의 confirmed spill에서 이미 검증된 count 반환 |
| TextOccurrenceEqualReuses / text_occurrence_equal_reuses | old와 next가 같은 identity여서 이미 읽은 prior count를 next에도 사용 |
| TextOccurrenceValidationFailures / text_occurrence_validation_failures | 위 prior/next count 검증 함수가 Err를 반환한 수 |

attempt/term/SQL 사전 거절은 payload 검증 호출 전이라 해당 attempts/failures에
포함하지 않으며 Diff span의 실패로 남긴다. failures는 두 attempts의 합 이하이다.
이 계수는 object-port validation 호출이며 syscall·실제 bytes·고유 reference 수가 아니다.
전역 unique 수는 미관측으로 고정한다. 추가 set/SQL 없이 현재 권위에서 얻을 수 있는
정확한 계수만 먼저 구현한다.

Off는 기존 diff를 직접 수행한다. Record는 고정 크기 stack totals만 사용하며 완료와
Err 모두 span 종료 전에6개 값을 발행한다. overflow는 CounterOverflow health를 남겨
증거를 무효화하고 제품의 원래 결과를 유지한다.

V5에서만 새 family를 허용하고 owner는 NativeTextInventoryDiff lane2로 제한한다.
완전한 capture의 family는6개가 함께 있고 중복이 없어야 한다. 종료 뒤 counter는
동일 timestamp도 거절한다. 이전 capture에 family가 없으면 재사용0으로 채우지 않는다.
소비자는 request ancestry별로 성공·실패 Diff 범위와 계수를 보고하고 full ACK나
전역 unique·물리 I/O로 승격하지 않는다.

독립 예시는 새 leaf 최초 검증1회/다음 RAM reuse1회, confirmed spill reuse1회,
prior 검증1회, equal identity의 prior 검증1회+equal reuse1회, 검증 실패의
attempt1/failure1이다. 실패한 검증 결과는 재사용하지 않는다.

편집 전9개 영향 파일은 checkout 밖
`~/.codex/artifacts/nov-5-indexing-bugbash/b02-before-20261105-sk6a9gkl`에 보존했다.
이는 영향 소스 기준선이며 전체 build/performance cohort로 사용하지 않는다.

먼저 기존 `validated_new_count`와 prior 읽기 경로에서 다음 의미를 고정한다.

| 항목 | 필요한 구분 |
| --- | --- |
| 검증 요청 | 신규 confirmed reference와 prior reference |
| 결과 재사용 | RAM memo, confirmed spill, 실제 payload validation |
| 반복성 | 전체 호출과 기존 exact window/confirmed 집합이 보증하는 고유 reference |
| I/O | 논리 get, 실제 신규 write, SQLite requested bytes를 별도 단위로 유지 |
| 시간 | 부모·하위 CPU/wall의 중첩과 미관측 범위 |
| 실패 | 실패 전 관측값과 성공한 전체 요청의 값을 구분 |

새 unbounded set이나 reference별 SQL을 계측용으로 추가하지 않는다.
고유 수를 기존 권위와 예산 안에서 정확히 얻을 수 없다면 계수 domain을 좁히고
전역 고유 수는 미관측으로 남긴다. 각 count의 scope와 exact 여부를 먼저 합의한다.

기존 batch/span 단위 aggregate를 확장한다. per-object trace event나 별도
관측 파이프라인은 필요 근거가 있어야 발급할 수 있다. Off 경로에는 신규
clock/집합/SQL/trace emission이 생기지 않아야 한다.

완료 조건은 독립 예제로 신규1회·반복·prior·memo/spill·실패의 기대 count를 고정하고,
기존 Core 진단 타입·wire·schema·소비자의 변경 지점을 한 계약으로 정리하는 것이다.
코드 편집 전에 해당 소스 기준선을 보존한다. 코드를 먼저 합류하고 실행을 몰아서 하는
순서는 기준선 소스를 버리는 이유가 아니다. B03 효과 비교의 두 arm에는 같은 B02 계측을
적용하고, 비교할 수리 hunk의 차이만 소스와 실행 파일에 결속한다.

<a id="b02"></a>
## B02 — 최소 계측과 소비자 연결

### 현재 소스와 정확한 잔여

- Text 생산자, Core6개 enum, daemon wire/schema·이름 golden, Python parser와
  요청별 report/product/canonical 전달 경로는 소스에 존재한다.
- 새 `native/text/tests/occurrence_validation_trace_tests.rs`에는 RAM·spill·prior·equal,
  실패 비메모화와 Off/Record 동등성의 독립 기대값이 있다. `text.rs`의
  `mod tests`에 등록해 실제 빌드·실행했다.
- 기존 `native/text/tests/diff_trace_tests.rs::run_diff`의 외부 Diff wrapper를 제거했다.
  Lexical·Diff span이 각각 하나인지, 실제 object-port child의 귀속이 맞는지 확인한다.
- 전체0 family, prior 검증 실패, payload 검증 전 term/attempt 거절의0 attempts/failures,
  overflow의 health/전체 family 생략 회귀를 추가했다. 기존 cancellation·손상·수명 회귀는
  재사용한다. Text 소유154개 테스트에서 새8개와 기존 Diff 귀속 회귀가 실제 통과했다.
- 기존 Core `scoped_diagnostic_context(None)`은 inherit 계약이다. 호출부의 mask 주석과
  다른 기존 seam을 발견했다. 원 계약은 유지하고 `scoped_diagnostic_override(None)`을
  명시 mask API로 추가했다. 같은 private RAII를 재사용하며 Off는 request clone/clock을
  추가하지 않는다. Lexical 관측 helper의 child=None과 Diff span 생성 실패를 연결했다.
  요청 identity·deadline·provider 권한·work 사용량·취소·unwind 복원과 실패한 Diff의
  하위 span 미발행을 Core331개·Text154개에서 확인했다.
- Python4모듈 배치의 첫 실행은 FAILED:1,038pass/2fail/2skip이었다. 새 fixture의
  raw/parsed 인자 순서와 overflow 오류 문구 기대값을 교정한 뒤 같은 배치는
  VERIFIED:1,040pass/2skip이었다. wrong-version의 빈 regex 경고도 제거했다.
  skip은 prebuilt capture의 managed-build receipt 부재라는 기존 조건이다.
- strict Clippy는 FAILED다. 최초 dependency 포함 명령은 다른 Contract Go trace의5개
  지적에서 멈췄다. Core/lexical `--no-deps` 진단도 다른 ranker·reader·packed writer
  등의 지적 때문에 실패했다. 이번 계수 생산자/테스트 지적은 수정했으며, 마지막
  테스트 표현식 lint 교정의 좁은 재검증은 정상 빌드 뒤의 같은 admission 큐에 넣어
  300초 입장 대기 후 exit124로 종료됐다. 테스트 본문0개로 NOT_RUN이며 다시 시작하지 않았다.
  이 진단을 전체 gate 통과로 바꾸거나 다른 담당의 기능을 재구현하지 않는다.
- 당시 B02 완료 시점에는 실제 V5 생산자→소비자 검증과 Large120 성능 수용·정식 tracing overhead가 NOT_RUN이었다. 현재 B1 실행 판정은 README가 소유한다.
- 이번 잔여 감사에서 영향 경로의 tracked diff에 대한 `git diff --check`는 VERIFIED다.
  이는 whitespace 검사이며 컴파일·동작·성능 수용 근거가 아니다.

2026-11-06 KST, 공유 dirty HEAD `de16b193b`에서 실행한 영향 검증 명령:

```sh
./scripts/cargow --lane test-workspace-lane test -p quanta-index-lexical --lib native::text:: --locked --offline -- --quiet
./scripts/cargow --lane test-workspace-lane test -p quanta-index-core --lib --locked --offline -- --quiet
.venv/bin/python -B -m pytest -p no:cacheprovider -q --tb=short \
  tools/ci/tests/test_causal_cost_timeline.py tools/ci/tests/test_causal_cost_profile.py \
  tools/ci/tests/test_causal_cost_capture.py tools/ci/tests/test_causal_cost_canonical.py
```

최종 본문 결과는 Text exit0/154pass(0.14초), Core exit0/331pass(0.82초),
Python exit0/1,040pass·2skip(49.06초)다. Rust 최초 빌드는2분03초였다.
mask 회귀의 초기 실패2회는 inherit API를 mask로 오인한 수리가 불충분했음을 보였다.
Core 신규 테스트의 타입 추론·unused-mut 컴파일 오류도 수정 후 같은 배치에서 통과했다.
포맷과 Python Ruff E9/F도 통과했다. 마지막 Text 테스트 표현식의 lint 교정은
`#[cfg(test)]` 한 곳이며 위 전체 통과 이후 변경이다. 그 영향8개 재실행 상태는 별도다.

B01 완료 뒤 코드 작업을 다음과 같이 병렬화한다. 공용 parent와 schema 합류는 Root 한 곳이다.

| Lane | 독점 변경 범위 | 결과 |
| --- | --- | --- |
| Text | `native/text.rs`의 기존 count 검증·diff 관측 함수와 그 영향 테스트 | 합의한 신규/prior/reuse 계수만 기존 scope에서 누적 |
| Consumer | 기존 Python trace/canonical 소비 함수와 해당 독립 fixture | 계수의 domain·누락·실패를 유지하고 범위가 다른 값을 합산하지 않음 |
| Oracle | 기존 Text/Storage fixture의 해당 시나리오와 기대값 | 구현과 독립적으로 count·손상·오류·메모리 기대값 고정 |
| Root | Core 진단 enum·wire/schema·parent 연결과 최종 문서 | producer부터 consumer까지 같은 계약으로 합류 |

Text lane과 Oracle lane은 테스트 함수와 parent 등록 hunk를 시작 전에 나눈다.
새 crate, 설치 backend, service, persisted format은 이 패킷의 산출물이 아니다.

완료 조건은 공통 entry point의 Off/Record 동작, 정상/실패의 bounded aggregate,
정확한 counter와 source 연결을 집중 검증하는 것이다. 아직 실행하지 않은 검증은
NOT_RUN으로 기록한다. 실제 관측에서 비용이 줄어들지 않으면 계측 성공만 기록한다.

<a id="b03"></a>
## B03 — 검증 결과 전달과 구조 수리

상태는 B03_B1_PRODUCT_FAILED_TYPED_FEE_FOCUSED_VERIFIED_MEDIUM_FAILED_B2_PENDING이다. File get 이중 SHA·Catalog registration
재인코딩 제거와 work delta 연결은 집중 검증했지만, 후속0a97e858의 실제 Medium은
512MiB cap, Large Off/Record는 원120초 deadline에서 실패했다. B03-B를 일반적인
cold read 증가만으로 중단한 이전 판정은 아래 원 기준 정정으로 대체한다.
owner-local block의 집중 회귀는 composite local proof로 통과했다. 실제 Large의 첫
Patch 활성화는 `d8e2c52d` 후보에 포함돼 해당 f17 transition test가 통과했지만
mr5 Medium에서는 실제 활성화됐지만 512MiB bytes cap에서 FAILED다. 같은 r5 pair의
lo5/lr5 Large Off/Record도 원120초 `BuildPrepare` deadline에서 FAILED다.
최신 `a6083fb2` 후보의 typed confirmation fee는 코드 구현과 f19 영향 집중
검증을 마쳤다. r6 normal cached pair build exit0 뒤 mr6 Medium도 bytes cap에서
FAILED다. Large120은 NOT_RUN이다. 이전 `d8e2c52d`의 제품 결과를 새 후보에
승격하지 않는다.

### 선택표와 필요한 관측

| 관측 결과 | 결정 | 하면 안 되는 변경 |
| --- | --- | --- |
| 현재 Large가 원 계약으로 통과 | B03 성능 패치 불필요. 남은 통합/overhead 범위만 진행 | 이미 해결된 비용을 이유로 포맷 개편 시작 |
| 신규 full validation이 실제 critical path 비용이고 현재 저장 경로가 추가 read 없이 인증된 payload/count를 생산·전달할 수 있음이 입증됨 | B03-A 검증 결과 전달 | 입력 position 길이·미검증 SQL count를 인증값으로 사용 |
| 같은 Native occurrence 생산자의 작은 신규 파일·namespace/create·sync가 큰 비용 | B03-B Lexical occurrence 묶음의 최소 설계/독립 fixture부터 검증 | 전역 Storage segment backend를 바로 신설, generic batch 크기만 증대 |
| 비용이 SourceApply·map·Catalog·기존 객체 검증에 있음 | 해당 실제 owner의 중복 작업1개를 특정해 다음 hunk를 정함. A/B는 그 원인에 대해 폐기 | occurrence 계수만 보고 unrelated phase를 수리 |
| 남은 wall이 주로 대기이고 CPU/I/O 귀속 불명 | 동일 binary의 필요 진단1개에서 기존 물리 phase와 제한된 OS thread sample로 분리. 비교 불가면 인과 주장은 BLOCKED | wall−CPU 전체를 host-only로 설명하거나 무변경 Large 재실행 |

6개 Diff 계수는 검증 호출/재사용 수다. 고유 reference·syscall·제거 가능한 시간을
보증하지 않는다. 현재 Native 경로에는 object-kind별 물리 집계가 연결돼 있지 않다.
legacy LexicalStore의 object_kind_aggregate를 Native 실측으로 사용하지 않는다.

51b70b34와 동일한 현재 Text 본문에서 기존 ancestry만으로 귀속이 가능함을 확인했다.
Prepare owner/lane35/잔여 flush의 설치 가능 경로는 `OccurrenceWrites::flush`뿐이고,
그 함수의 유일한 설치는 `context.put_batch(&self.inputs)`다. inventory map 설치는
별도 NativeTextInventoryMapApply sibling이다. 따라서 이 고정본의 가장 가까운
Prepare 조상을 가진 physical packet은 occurrence 설치로 읽는다. 추가 wrapper는 불필요하다.

이는 관측된 해당 subtree의 귀속이며 전체 capture의 per-kind census·고유 reference 수를
뜻하지 않는다. 기존 packet 입력·신규 write·phase wall/CPU를 사용하고 신규 write는
성공한 publish/confirm과 함께 해석한다. per-reference event·전역 unique set·새 글로벌
kind 계수는 추가하지 않는다. C17 옛 source byte를 다시 확보하지 않고 이 증명을 과거
C17 전체 capture에 소급하지 않는다.

추가 코드 대조로 좁힌 판정 축은 다음과 같다. 이 목록을 새 미구현 티켓으로 발급하지 않는다.

- NativeTextPostings/NativeTextRoot는 입력 loop 뒤에 있다. Prepare prefix의 개선만으로
  후반 완료를 가정하지 않는다. inventory Block은 posting에서 금지되므로 두 비용을 분리한다.
- lane35 CPU가 큰 경우에만 세 번의 bounded pending 탐색/encode 비용을 검토한다.
  RAM64+spill은 이미 있으므로 새 cache 구현을 기본 수리로 잡지 않는다.
- File get의 read_wire→decode_stored_wire에는 동일 wire hash의 정적 중복이 있다.
  영향이 확인되면 private 검증 결과를 재사용하는 좁은 수리를 검토하되 scratch/resident
  decode의 인증을 제거하지 않는다. 같은 검증을 다른 phase로 옮겨 속도 수리라고 하지 않는다.
- semantic 미설정/provider_units0 fixture의 Lance/DataFusion compile은 빌드 graph 비용이다.
  제품 Text120초의 semantic 실행 원인으로 분류하지 않는다.

<a id="b03-a"></a>
### B03-A — 검증된 결과를 실제 소비자까지 전달

선택 조건은 신규 first-validation의 실제 비용과 전달 가능한 기존 권위가 모두 있는 것이다.
next_validation_attempts가 작거나 대부분 prior 읽기이면 이 후보를 폐기한다.
설치 뒤 새로 같은 read를 추가해 단지 Diff 밖으로 이동시키는 변경도 폐기한다.

우선 기존 `StoredObject`, `read_retained`, confirmed-count와 attempt/grant를
재사용할 수 있는지 확인한다. 새 권한 타입이 필요하면 어느 생산자가 실제 저장 bytes를
검증하는지, 무엇이 identity·형식·count를 보증하는지부터 고정한다.

수정 범위는 생산자의 실제 저장/검증 결과, Kernel wrapper/port의 해당 전달 경로,
`ConfirmedOccurrences::validated_new_count_observed`와 diff의 해당 소비 부분이다.
Root가 authority·attempt·reference·count·memory lifetime과 confirm 실패 시 무효화를
고정한 뒤 writer/reader를 나눈다. DTO만 추가하거나 검증 없이 count를 메모하는 것은 제외한다.

필수 불변식은 다음과 같다.

- candidate position 길이나 미검증 SQL 값만으로 validated count를 만들지 않는다.
- 다른 identity·잘못된 payload·잘못된 count를 기존 gold와 같은 경계에서 거절한다.
- 수리의 결과를 실패한 install·미완료 confirm·다른 attempt에서 재사용하지 않는다.
- namespace 현재성과 실제 저장 authority의 검증을 보존한다.
- retained payload와 metadata를 원 메모리 grant에 포함하고 해제·취소를 검증한다.
- 실패·재시도·no-replace loser·기존 객체 경로에서 일관된 상태와 원 오류를 유지한다.

설치 API를 바꾸는 경우 실제 File backend, Kernel wrapper와 모든 caller를 같이
검증한다. mock·serializer·한쪽 DTO의 성공만으로 완료 판정할 수 없다.

<a id="b03-b"></a>
### B03-B — Lexical occurrence 저장 단위를 묶기

상태: IMPLEMENTED_FOCUSED_VERIFIED_PRODUCT_NOT_RUN. 원문 RFC12.1–12.5/E03/E07 대조 결과 universal
cold query bytes 비증가는 원 hard gate가 아니었다. 이전 자체 폐기 기준을 정정한다.
block은 구현됐지만 제품 채택 전이며 원 query work/memory/SLO/gold·old/current pin·
증분 locality를 고정 profile에서 만족해야 한다. Large120과 ANN4K 원 한도는 불변이다.

이 후보는 inventory의 기존 Small/Block을 다시 만드는 작업이 아니다. term별 position
payload를 작은 bounded immutable block에 묶어 물리 객체 수를 줄이는 후보이다.
현재 map의 ObjectRef/child-edge와 Storage 설치 API를 먼저 재사용한다. 전역 object
store format/segment/locator/GC authority 개편은 이 단계에서 제외한다.

구현에 적용한 계약은 다음과 같다.

- block 내용은 한 owner의 logical inventory로 한정한다. 현재 physical packet이 여러
  file/ordinal을 담는다는 사실을 block 내용의 허용 범위로 사용하지 않는다. 서로 다른
  owner의 같은 term·다른 positions는 다른 block ref여야 한다. 완성된 block들의 물리
  put_batch는 기존처럼 파일 경계를 넘어 공유할 수 있다. 동일 bytes block alias는 허용한다.
- 하나의 decoder owner에서 term을 지정해 positions/count를 읽는다. key 정렬·고유성,
  position 형식·범위, 빠진/다른 term, 끝의 쓰레기 bytes를 전체 검증 후 결과를 노출한다.
- physical confirmation은 ref 단위, validated count는 `(ref, canonical stored term)`
  단위다. 같은 ref의 서로 다른 tf가 RAM/spill에서 섞이지 않아야 한다.
- bootstrap은 bounded block을 쓸 수 있다. 증분은 변하지 않은 term의 기존 block ref를
  유지하고 변경/new term만 새 leaf에 둔다. 한 term 변경으로 모든 posting ref를
  다시 쓰는 설계는 폐기한다. 삭제는 해당 mapping만 제거한다.
- block 크기·retained bytes·decoder scratch·query 재사용은 기존 memory/work grant로
  먼저 승인한다. object 최대 크기를 그대로 block 기본 크기로 쓰지 않는다.
- count뿐 아니라 BM25 tf, phrase의 term별 positions, sensitive/folded surface와
  old/current snapshot 검색이 기존 gold와 같아야 한다. 같은 block을 여러 query cursor가
  다시 읽어 bytes/CPU가 늘어나는지 확인한다. 필요한 cache는 query-local bounded
  authenticated DATA 재사용이며 전역 cache를 신설하지 않는다.
- Text policy/형식 계약과 old pinned snapshot 읽기를 Root가 함께 고정한다. 기존 leaf와
  새 표현은 단일 lexical 읽기 owner가 다루며 silent fallback이나 generation 혼동은 금지한다.
- 동일 physical ref를 여러 map entry가 참조하는 GC edge/order/중복을 보존한다.
  마지막 참조·pin이 없어지기 전 block을 회수하지 않는다.

코드 접점은 `text_occurrence.rs::OccurrenceReader`의 codec/selector,
`text.rs::{OccurrenceWrites,prepare_inventory,validate_posting_hit}`,
`ConfirmedOccurrences`와 `text_spool.rs`의 확인/count 키다. 단일 현재 정책의
정확한 leaf/block wire와 bounds는 Root가 합류 전에 고정한다.

폐기 조건도 고정한다. 기존 position leaf의 term 간 exact dedup을 잃어 실제 신규 파일
수가 줄지 않거나, 원래 고정된 query work/memory/latency·pin 유지 한도 또는
region-local delta bound를 위반하면 채택하지 않는다. Cold read bytes의 증가량도
측정·보고하되 그 자체를 universal zero-amplification 위반으로 판정하지 않는다. 기존 query/memory/work 한도를 유지하고 새 임의 성능 목표를 만들지 않는다.
별도 ANN 원4K의1M reads/512MiB·SDK30s/worker10s와 Large4,096-source120s의
workload/caps를 혼합하거나 한쪽을 완화하지 않는다.

### B03-B 후속 계약 검토 — 원 기준 정정

- RFC12.1의 metric별 선고정 region bounds와12.3/12.5의 고정 query mix/SLO가 authority다.
  모든 cold query의 bytes가1B라도 늘면 폐기하는 조건은 이 문서가 추가한 과도한 기준이었다.
  이를 원 제품 gate 완화로 표현하지 않는다. 새 후보 측정 전에 기준을 고정한다.
- 후보는 owner 내부 term→positions-vector dictionary이며 서로 같은 positions는 같은
  vector slot을 사용한다. 제안 고정 크기는 payload4,096B/128 term rows, 벡터별 기존
 8,192 positions 상한이다. 폭을 N에 따라 키우지 않는다. 단일/oversize/delta는 기존
  leaf를 사용하고 capacity-full과 실제 memory/work/cancel 거절을 다른 결과로 구분한다.
- 새 policy는 허용된 leaf/block variant와 숫자 한도를 명시한다. 구 policy member는
  기존 typed refusal을 유지하며 startup rewrite/drop이나 silent fallback은 없다.
  미배포 구 codec의 영구 reader는 요구되지 않는다. 실제 구 refs/pins이 남아 있는
  전환은 별도 compatible executable/artifacts 유지 또는 cutover 거절 계약을 따른다.
  새 policy 내부의 old/current generation·reopen·pin은 반드시 보존한다.
- count 재사용은 `(ref,term)`으로 선택한 완전 인증 DATA여야 한다. ref-only scalar memo는
  leaf에만 유지하고 block에는 완전 검증된 bounded layout/cache를 사용한다. physical
  confirm 자체는 ref 단위다. query-local cache는 실행 범위를 벗어나지 않는다.
- 우선 기존 r4 pair의 실제 work delta를 확인한다. 위 wire/cache ABI를 Root가 확정한
  뒤 codec/producer/독립 oracle의 파일을 분리해 구현한다. 중간 결과를 제품 개선으로
  승격하지 않는다. 일반 Storage segment backend나 기존 inventory Block을 재구현하지 않는다.

### B03-B1 — codec·selector ABI 고정

원 Large Off(lo4)는0a97e858에서도 BuildPrepare deadline으로 실패했다. 같은 pair의
Large Record(lr4)도 원120초 deadline에서 실패했으며 새4축 계수를 수집했다. 세 개의 작은 수리만으로120초를
충족하지 못했으므로 다음 구조 후보의 producer/reader/독립 oracle를 파일별로 분리한다.
원 query/memory/work/SLO·old/current pins·delta bounds가 채택 조건이다.

새 current policy의 leaf/block variant이며 구 policy 자동 재해석은 없다. Block은 한
owner의 inventory 안에서만 구성한다. 같은 positions 배열은 block 내부 dictionary의
한 vector를 공유한다. JSON leaf는 singleton/oversize/new changed term용으로 유지한다.

고정 binary: `QITXOC01` 8B magic, BE u16 term_count/u16 vector_count/u32 reserved0
(총16B header), 정렬된 row들의 `(BE u16 key_len,key,BE u16 vector_index)`, 그 뒤
vector들의 `(BE u16 count,count×BE u32 position)`이다. payload≤4096B, term rows2..128,
vectors1..term_count, 각 positions1..8192·strict increasing. vector first-use ordinal이
0..n 순서이고 미사용/중복 vector·중복/역순 key·bad selector·trailing bytes는 거절한다.

private ABI(코드 완성 전 임의 success/stub body 금지):

- `text_occurrence::block::{MAX_BLOCK_BYTES=4096,MAX_BLOCK_TERMS=128,LAYOUT_POLICY}`.
- `BlockDraft::new()->Result<Self,IndexError>`, `len()->usize`,
  `try_push(PreparedKey,PreparedPositions,&WorkBudget)->Result<BlockPush,IndexError>`.
  `BlockPush::{Added,Full{term,positions}}`; Full은 원 소유 입력을 반환하며 budget/cancel
  거절을 Full로 바꾸지 않는다. Dictionary는 동일 positions의 보관을 한 번만 한다.
- `BlockDraft::finish(self,&WorkBudget)->Result<SealedBlock,IndexError>`.
  `SealedBlock::{Leaf{term,positions},Packed{payload:StagedPayload,terms:PreparedRows<BlockTerm>}}`,
  `BlockTerm{key:PreparedKey,vector:u16}`. empty는 거절하고 single은 Leaf로 반환한다.
- `CheckedBlock::parse(StoredObject,&WorkBudget)->Result<Self,IndexError>`가 full preflight한다.
  `object(&self)->&StoredObject`, `selection(&self,term:&[u8],&WorkBudget)->Result<Range<usize>,IndexError>`는
  소유 payload의 선택 vector BE u32 범위만 반환한다. 원 authenticated StoredObject와
  DATA lease를 layout이 소유해 외부 payload 혼합 및 term당 SHA 재계산을 없앤다.
- 전체 wire/row/dictionary/layout allocation을 원 memory scope에서 before-birth 승인한다.
  Producer는 완성된 다른 block들을 기존 put_batch에서 함께 설치한다. same-ref confirm은
  physical ref 단위, term별 count는 완전 검증된 block layout으로 분리한다.

파일 one-writer: codec은 새 `text_occurrence/block.rs`, producer는 새
`text/occurrence_pack.rs`, 독립 codec oracle는 새 `text_occurrence/block/tests.rs`.
Root만 `text.rs`, `text_occurrence.rs`, `text_memory.rs`의 mount/policy/cache/caller 접점을
수정한다. 기존 inventory Block/P1-A·Core/Storage/Catalog/SDK를 이 패킷에서 재구현하지
않는다. Worker Cargo0, Root 정적 합류 뒤 영향 검증을 한 큐에서 수행한다.

### B03-B1 실제 합류와 검증 큐

f9 frozen source에서는 bootstrap caller만 `OccurrenceWrites::pack_owners`를 선택하고
empty prior에서 `occurrence_pack::prepare_owner_blocks`를 호출한다. 그러나 실제
Large harness의 첫 profile은 step995, 첫 SourceAdvance는 step1059의 Patch다.
Bootstrap-only 조건으로는 원 Large full 경로에서 B1을 켜지 못한다. 현재 후보
`d8e2c52d`는 `Bootstrap` 또는 `Patch(delta.prior.owners.0.is_none())`를 선택한다.
profile-first 초기 Patch의 변경 transition test는 f17에서 통과했다. Sparse의
nonempty prior leaf와 old-ref bound는 보존한다. 이후 Patch의 변경 term은 leaf다.
Delta는 prior positions를 term-aware reader로 인증 비교하고 같으면 old ref를 유지한다.
Query의 cache는 diagnostic observer와 독립이며 Off/Record의 제품 경로가 같다.

합류한 독립 oracle:

- `text_occurrence/block/tests.rs`: literal wire·distinct tf·dictionary dedup·malformed15경계·
  128 rows/4096B Full·cancel/memory denial·layout/output grant 환급.
- `text_occurrence/tests/reader.rs`: block selector별 count/positions·leaf no-cache·cap2 축출·
  wrong retained DATA/refusal/cancel·초기 foreign ref·DATA/layout/position lease 수명.
- `text/tests/occurrence_blocks.rs`: actual plan/prepare bootstrap→delta→old/current ticket와
  object-port 재생성 query, 1018-position oversize leaf,132 canonical rows→2physical blocks,
  후속 설치 output mismatch에서 map publish0.
- Kernel `native_lifecycle/occurrence_blocks.rs`: 실제 FileObjectStore/SqliteCatalog에서
  source 먼저→Reconfigure bootstrap, delta의 block+leaf 공존·tf/positions/phrase,
  old/current exact pins·재오픈과 마지막 pin/history/custody 해제 후 occurrence block reclaim.

포맷·diff 공백·hexagonal 검사는 VERIFIED다. 독립 소스 감사에서 확정 P0/P1은 찾지
못했으나 이것은 실행 성공이 아니다. Native 범위 테스트의 첫 admission은300초 뒤
exit124/body0이었다(NOT_RUN). 같은 controller가 끝난 것을 확인한 뒤 wait7200인
단일 successor를 공용 큐에 넣었다. 첫 successor는17분42.653초 후 입장해48.192초
컴파일했고, 새 oracle의 unused-results5건에서 FAILED(exit101)로 끝났다. 테스트 본문은0이다.
Splice의 즉시 drop과 atomic 이전값의 명시적 소비로5건을 수정했다. 그 successor는
f5/f6/f7 고정본 검증으로 이어졌으며 결과는 아래 이력에 기록한다. 가드를 우회하거나
worker Cargo를 병렬 실행하지 않는다.

```sh
QUANTA_INDEX_RESOURCE_WAIT_SECONDS=7200 ./scripts/cargow --lane test-workspace-lane \
  nextest run -p quanta-index-lexical --lib --locked --offline \
  -E 'test(native::)' --test-threads 2
```

위 admission·재큐 기록은 과거 f7 이전의 경과다. 현재 후보는 f15 Native292개와
기존 Kernel lifecycle12개, f17 변경 transition1개의 composite local proof가
VERIFIED다. f16 Native 소유 Clippy 진단0이지만 전체 lexical Clippy는 다른 owner
33개 진단으로 FAILED다. 새 정상 cached pair build는 exit0이고 mr5 Medium 제품
body는 bytes cap에서 FAILED다. lo5 Large Off도 원120초 deadline에서 FAILED다.

첫 독립 source 대조에서 다음 candidate에 필요한 파일은12개였다: lexical 기존5개
(`text/text_occurrence/text_memory/text_query/text_reader.rs`), 신규5개(codec·producer와
독립 oracle3개), Kernel lifecycle parent mount와 새 oracle2개다. 기존 r4의16개 owned
입력 중 이번 수정 외 drift는 없고 text.rs만 B1으로 달라졌다. 전체 dirty tree를 복사하지
않는다. 최종 local snapshot은 Kernel `text_entry_maps.rs`를 더해13개를 복사했다.
외부 `candidate-build-03.py`는 terminal까지 완료됐다. normal cached build exit0,
같은 controller 환경의 mr5 Medium은 producer exit1/512MiB bytes cap FAILED다.
`pipeline_completed=true`는 controller 종료만 나타내며 제품 수용이 아니다.
영향 검증 VERIFIED manifest를 요구하고 동일 안정 s3 경로·r4 private tool aliases를
재사용한다. 새 build와 Medium을 한 controller의 같은 env에서 실행해 r4 PATH drift의
재발을 막는다. normal cached/prebuilt UNPROVEN과 qualified managed-fresh는 계속 구분한다.

추가 계약 대조에서 bootstrap vs delta의 raw artifact/logical_commitment 동일성은
요구되지 않음을 확인했다. RFC의 law는 같은 SourceCut의 membership/postings/statistics·
exact query/ranking 동등성이다. Kernel oracle는 같은 ticket에 기존 incremental instance와
새 bootstrap instance를 함께 두고 input root/policy·stats·TF/positions·부재 term·keyword/
phrase BM25 matched/hits를 비교한다. 이 동등성 oracle의 두 source-order case는
f15 Kernel 집중 범위에서 통과했다. 정확 d8e full Kernel13 동시 실행과 새 a608
source의 검증은 각각 별개다.

공개 `with_inventory_reuse_limit(0)`의 “Zero disables reuse”를 B1에서도 보존한다.
처음 합류에서는 block cache가 무조건 켜지는 P2를 정적으로 발견해 수정했다.
Confirmed/prior preparation의 zero 경로는 uncached reader를 선택한다. 다음 호출 전
current parsed DATA를 지우고 원 get/authentication/charge를 다시 수행한다. 파생 count
memo도 제공하지 않는다. `new(0)`의 invalid-window 거절은 그대로이며 uncached는
내부의 명시적 physical policy다. default/query window 숫자와 output_policy는 불변이다.

실제 소스 동결 검증 이력:

- Main의 재큐가 helper 리팩터 중에 입장해 intermediate method E0599에서 실패했다.
  테스트0이며 Root가 편집과 검증을 직렬화하지 못한 실행 오류다. 완성본 제품 결함으로
  승격하지 않는다. 이후 mutable main 대신 안정 s3 경로의 clean commit에서 검증한다.
- `cc6b1e1`/f5는 정상 컴파일 후 Native292개 중110개를 실행했다.109pass/1fail,
  fail-fast로182개 미실행·558개 scope 제외다. 실패는 새 codec의 row key end가 뒤의
  2B vector selector까지 포함한 결함이다. key bytes를 읽은 직후 end를 고정해 수리했다.
- `36a883a1e44274524803c736ed2128a8f06e1800`/f6는 같은12개 소유 파일을 다시
  고정했고 Native 전체 선택 범위에 `--no-fail-fast`를 사용했다. 이전 f5 실패는 보존한다.

이전 동결본 `642f74ac35487623bb3c1d686074367e1a98ddb8`/f7의 이력이다.
f6는 Native 전체292개 중291pass/1fail이었다. 남은 오류는 새 test port의
RegisteredObject.bytes가 payload 길이만 반환해 기존 inventory Block의 wire 길이
검사에 거절된 fixture 결함이다. 16B header+32B×children+payload와 object-v1
hash domain의 실제 framing/identity를 적용했다. 제품 검증의 wire guard는 유지했다.

f7의 동일 Native scope는 VERIFIED:292pass/558 scope 제외, body3.881초다.
codec literal gold·reader7개·bootstrap/delta/fault4개·기존 Native 영향 범위를 포함한다.
source/tool 전후 검사를 통과했다. f7 Kernel은 stale generic fixture로 FAILED했고,
같은 source의 f8은9pass/3fail이었다. f8 실패는 current MapBlock fault target,
Inline fixture의 cap35>32 강제, Shared64 batch 내 pending dedup reserve oracle의
세 결손이었다. 기대값과 fixture를 실제 계약에 맞게 수리한
`4b06b203e636b2e303652a3f55429b506c2fcd01`/f9에서 Kernel12pass/65 scope 제외,
125.827초, source/tool guard0으로 VERIFIED다. 이 결과에는 기존 source-before-profile
Bootstrap lifecycle이 포함되며 실제 harness의 profile-first Patch는 포함되지 않는다.

f10 strict Clippy는 같은 f9 source에서6개 lib 지적으로 FAILED였다. B1 소유2개인
`block.rs`의 `single_match_else`와 `text_occurrence.rs`의 긴 doc 지적은
공유 Main에서 수정됐다. f16의 lexical lib/tests `--keep-going`은40→33개
다른 owner 진단을 확인했고 Native 소유 진단0이다. 전체 Clippy는 FAILED다.

f11 Tools03의 old Record epoch는 producer body0으로 거절됐다. 15개 path에서
`st_dev`33→30만 달라지고 hash/inode/mtime 등은 같은 새 Tools03 invocation epoch를
고정했다. guard를 약화하지 않았다. f12는 입장 대기 prefix 뒤 PID/open FD가 없어
NOT_RUN으로 종료됐다. 새 epoch의 f13(source347)은 Native292pass, Kernel13 중
11pass/2fail이었다. raw JSON leaf 단정은 public reader TF/positions로 교체했다.
f15(`68ea46acdc952b9506ecded6d75f22b79cd421a7`)는 Native292pass, Kernel13 중
12pass/1fail이었다. 남은 같은-content 다른-ref-mapping에 block id 동일성을 기대한
oracle을 동일 ordered ref mapping의 Tree→Block roundtrip으로 교정했다.
physical map wire/ordered children·pin/GC 검증은 유지했다.

현재 후보 `d8e2c52de9f0e058475aa25e3d746cdad3015672`의 f17 변경 transition
test는1pass/77 scope 제외·6.337초다. d8e2↔68ea diff는 Kernel test 파일1개만이므로
f15 Native292+기존 lifecycle12+f17 변경 case1의 composite local proof는 VERIFIED다.
같은 exact commit의 full suite·제품 qualification이 아니다. `candidate-source-03.json`
focused gate는 VERIFIED이며 Main HEAD/index는 불변이다. `candidate-build-03.py`의
normal cached r5 build exit0, 같은 controller env의 Medium body는
536,866,122B+14,133B에서 FAILED다. `r5/terminal.json`의 `pipeline_completed=true`
또는 controller exit0을 제품 성공으로 읽지 않는다. lo5 Large Off는 원120초
deadline FAILED다. `l8ctl/terminal.json`의 nested execution은 `lo4`를 잘못 읽어
145.286325초로 기록했으므로, lo5 직접 `execution.json`의226.898901초와 refusal을
사용한다. 이전 r4 Large120 실패도 유지한다.

seed42 Large 입력8,609,531B는 확인된 값이다. 현재 코드에서 약33,716 unique
leaf2.94MB를 12,288 block23.93MB로 바꾸는 수치는 **코드 기반 추정**이다.
실행 측정이 아니며 bytes 증가 위험이 있다. 원 region metric bounds·고정 E07
mixed query SLO·work/memory cap으로 채택을 판정한다. 객체 수 감소만으로 채택하지 않는다.

### 계약 합류 후 병렬 소유

동시에 선택한 수리는 하나다. 아래 lane은 최대3개의 source worker이며 heavy proof는
Root만 실행한다. 공용 hunk를 동시에 편집하거나 peer/다른 채팅에 직접 연락하지 않는다.

| Lane | A 선택 시 독점 범위 | B 선택 시 독점 범위 |
| --- | --- | --- |
| 생산자 | 선택된 실제 저장/검증 owner의 result 생성과 실패·정산 | `OccurrenceWrites/prepare_inventory`의 bounded packing과 unchanged 보존 |
| 소비자 | Native count/diff의 권위 소비·수명 | `text_occurrence.rs` term-aware decoder와 query/phrase 호출부 |
| 독립 oracle | count 권위 위조·손상·다른 attempt·실패 confirm·cancel/refund | codec gold, 두 term의 다른 tf, 단일-term delta/delete, phrase, snapshot/pin/GC와 fault |
| Root | 공통 port/type, Kernel/wire 연결, parent 등록, 최종 합류 | 공통 wire/policy, RAM/spill selector 키, shared `text.rs` 선언/parent, 최종 합류 |

Root의 shared `text.rs` 변경은 계약/선언을 먼저 합류하고 freeze한다. 이후 writer와
reader는 겹치지 않는 함수만 편집한다. 독립 test는 별도 파일, parent 등록은 Root다.
이미 소유한 다른 변경의 active 여부는 코드/현재 claim 근거 없이 추정하지 않는다.

정적 완료 조건은 실제 producer→caller→consumer, 남은 ref-only count 소비자,
recipe/decoder/old snapshot, memory owner와 error settlement를 전부 대조하는 것이다.
독립 oracle는 구현의 helper로 기대값을 다시 계산하지 않는다.

<a id="b04"></a>
## B04 — 합류·검증·제품 수용

### 현재 고정본과 실제 실행

외부 `/Users/songmin/.codex/qib6-_0r6mac7/s`의 clean Git source는
`51b70b3470ecbb58beebf757831eb53db85aade6`이다. 원 공유 HEAD `de16b193b`의
tracked/nonignored-untracked3,120개 regular file을 보존하고 필요한 Core 테스트
교정을 반영했다. 원본 inventory는 외부 sibling `source-manifest.json`이 소유한다.
공유 HEAD/index를 변경하지 않고 외부 검증 저장소에만 snapshot commit을 만들었다.
고정 이후 공유 트리의 마지막 테스트 lint 표현식 변경은 제품 빌드에 포함되지 않는다.

다음 명령은 실제 시작했으나 성공 terminal/receipt 전에 중단됐다.
2026-11-06 KST 재확인에서 기존 tool handle89187이 없고, 해당 외부 root의 실행
프로세스와 build log를 연 FD도0이었다. 원 로그는01:10:57의 compile prefix에서 끝났고
종료코드는 알 수 없다. 성공이나 제품 성능 실패로 추정하지 않는다.
당시에는 Medium 제품 결과와 새 계수 분석이 없었다. 이후 정상 재개와 mr2의 실제 결과는 아래 최신 절을 따른다.

```sh
/Users/songmin/Documents/code-new/quanta-index/.venv/bin/python -B \
  -m tools.benchmark.retrieval.causal_cost_capture \
  --cwd /Users/songmin/.codex/qib6-_0r6mac7/s \
  --source-revision 51b70b3470ecbb58beebf757831eb53db85aade6 \
  --managed-build --backend index-native --tier medium --seed 42 \
  --native-trace-mode record --native-resource-sampling off \
  --trace-max-output-bytes 268435456 --disk-observation boundary-only \
  --max-seconds 300 --out-root /Users/songmin/.codex/qib6-_0r6mac7/m
```

위 명령의 cwd는 고정 source 디렉터리다. stdout/stderr와 fresh target은
`m/build/`에 보존 중이다. source/custody 검사가 실패하면 해당 실행을 실패로 남기며
guard나 제품120초를 완화하지 않는다. 완료 전 동일 managed build를 다시 시작하지 않는다.

### 중단 확인 후 캐시 재사용 재개

외부 source는51b70b34 clean 상태, Native 제품 접점108개 중107개가 공유 트리와
동일하고1개는 cfg(test) lint 표현식 차이다. 관련7crate 전체536개 Rust 파일을 대조한
추가3개 차이는 별도 File paged wire 변경이다. frozen 전체 crate를 최신 공유 전체
검증으로 취급하지 않는다. B03 Native 제품 코드와의 새 drift는 관측하지 못했다.

`resume-01.py`의 정상 재개는 Cargo 실행 전 공용 admission300초 대기에서 exit124로
끝났다. `r1/terminal.json`이 실제 종료를 기록하며 compiler/제품 본문은 시작하지 않았다.
이 controller가 종료됐음을 확인한 뒤 `/Users/songmin/.codex/qib6-_0r6mac7/resume-02.py`를
실제 시작했다. 입장 대기만7,200초로 바꿨으며 제품 dispatch120초나 work 한도,
capacity guard를 변경하거나 생략하지 않았다. 시작 PID71704/2026-11-06 01:29:39 KST다.

source/config/dependency/tool을 전후 검사하며 기존 `m/build/target`을 재사용하고,
동일 canonical cargow/NATIVE_BUILD_ARGS의 정상 bench pair를 완성한다.
별도 detached controller가 실제 자식 종료코드와 `r2/terminal.json`을 남긴다.
이 실행은 managed-fresh build receipt를 만들거나 이전 성공으로 가장하지 않는다.

실제 resume02는 admission758.451초 뒤 compiler에 입장했고39분39초 compile 후 exit0이다.
정상 binary pair는 `r2/normal-build-result.json`에 기록했다. 이후 mr2 Medium fixed Record를
실행했으나512MiB 누적 work bytes에서 실패해 controller가 종료됐다. `r2/terminal.json`의
ended UTC17:24:18.584와 실제 PID 부재를 확인했다. mr2의 CLI2는 producer exit1을 포함한
제품 실패이고, 성공 prebuilt에서도 남는 build_binding=UNPROVEN과 구분한다.

원 controller의 'Medium product/canonical 성공 뒤에만 Large' 조건은 과도한 실행 순서였다.
Medium fixed512MiB와 Large explicit512GiB는 다른 profile이다. mr2 실패를 유지하고
외부 `large-01.py`가 동일 source·pair·filtered environment로 `lo2`를 독립 실행한다.
PID28518/2026-11-06 02:34:43 KST 시작을 실제 command/start-time과 daemon 자식으로
확인했다. 원120초·67M/512GiB를 유지하며 새 build나 guard 우회는 없다.
근거는 외부 `l1/{live,terminal}.json`, `lo2/execution.json`에 남고 원본 mr2는 보존한다.
이 실행의 종료 결과가 나오기 전 제품 성공을 판정하지 않는다.

### 새 원 Large Off 결과와 단일 Record 진단

`l1/terminal.json`과 `lo2/execution.json`은 Large Off의 producer exit1을 기록한다.
outer timeout=false, source/pair 전후 동일, 원explicit67M/512GiB다. 실제 거절은
BuildPrepare의 deadline이며 work bytes4,715,789,654이고 numeric/memory/external
refusal이 아니다. 상세 수치는 [새 RCA](RCA.md#새-lo2-tracing-off에서도-원120초-실패-재현)를 따른다.

Off만으로 Text의 세부 실패 구간은 알 수 없어 계획한 Large Record1회를 실행했다.
외부 `large-02.py`/`l2/`가 동일 filtered environment·pair로 `lr2/`에 기록한다.
원120초와 work caps, sampling off, boundary-only를 유지하고 trace 출력만 Record로
변경했다. Record도174.409초·exit1/BuildPrepare deadline으로 종료했다. 새 build나 반복 Off는 없다. 이 실행은 진단이며 full-success 비교 쌍이
없으므로 elapsed 비율을 정식 tracing overhead로 바꾸지 않는다.

닫힌 child451,717 events를 기존 V5 parser로 읽어 health/loss/open/lease0을 확인했다.
실패 요청의 SourceApply24.865초와 Text94.512초, Prepare/Map/Diff 및 설치 phase 호출
귀속을 [RCA](RCA.md#lr2-오프라인-분석-실제-도달-경로와-다음-코드-범위)에 기록했다.
같은51b pair의 추가 Large 진단이나 fresh build는 필요하지 않다. 이후 cold query/delta
당시에는 B03-B를 중단하고 File hash/work delta를 수리했다. 이 block 중단 판단은
아래 원 기준 정정으로 대체한다. r3는 compiler 전 취소했고 r4 빌드·세 실패 실행은 완료했다.

다시 기다릴 때는 handle 파일이나 lock만 보고 live라고 하지 않는다. PID의 실제
command/start-time, 자식/FD와 terminal을 확인하며, 살아 있는 동일 controller를 재시작하지 않는다.

Root가 실행 큐 한 곳을 소유한다. 코드 작업자는 정적 대조와 좁은 소스 검사를 먼저
완료하고 build/product를 각각 시작하지 않는다.

1. 현재 producer→Kernel→Text→Storage→Catalog/Runtime→consumer의 실제 경로를 대조한다.
2. lane별 diff와 독립 oracle를 합류하고 affected signature/schema/경계를 정적으로 검사한다.
3. 영향 compile/strict와 소유 회귀를 한 배치로 검증한다.
4. 새 결함이 있으면 해당 source의 원인 hunk를 고치고 영향 범위를 다시 검증한다.
5. 현재 source와 정상 harness/daemon, workload/caps·관측 도구와 모드를 고정한다.
6. candidate Medium Record1회로 동일 입력·결과·물리 packet과 제거한 작업을 확인한다.
   정량 speedup을 주장할 때만 비교 가능한 호스트의 paired 실행을 별도로 수행한다.
7. candidate Large Off1회에서 원120초 full ACK/search/reopen과 후속 lifecycle을 판정한다.

경합 중의 명시적 진단 실행은 기능·관측 경로의 근거다.
causal 효과 비교에는 비교 가능한 호스트와 일치한 작업이 필요하다.
최신 코드가 바뀌지 않았다는 이유만으로 같은 Large 실패를 반복하지 않는다.
새 실행은 가설·변경·측정 입력 중 무엇을 확인하는지 미리 명시한다.
과거 C16/C17 source cut·trace·외부 실행기의 경로는 현재 없다. 이번51b70b34는
별도의 새 cohort다. 새 실행의 보존 위치와 수명을 미리 정하고, 정리 가능한 캐시
하나만 원본 참조로 남기지 않는다. 새 cohort 결과를 과거 source의 replay로 부르지 않는다.

### 현재 후속 코드 고정본

File get의 이중 hash 제거·SourceApply/Text4축 work delta는 영향741개와 Kernel lib
check·hexagonal·module 검사를 통과했다. Catalog registration 재인코딩 제거는
집중37개를 통과했다. 강화 SourceApply integration body는 아직 NOT_RUN이다.

- r3/86e6130e는 Catalog 수리를 묶기 위해 compiler 시작 전에 취소했다(exit143).
  제품 본문0이며 성능 실패가 아니다. 당시 PID50125의 대기 기록은 과거 상태다.
- r4의 clean cut은 `s3` / `0a97e8589daa41b736cb6d59545b0116bb3b89c6`이다.
  candidate-source-02의16개 입력을 보존하고 정상 cached pair build를 완료했다.
  `r4/terminal.json`은 Catalog0/build0/pipeline_completed=true다. 공유 HEAD/index는 그대로다.
- mr4는12.356839초 producer1, fixed512MiB 초과다. used536865857+10600B,
  reads963417/writes588655/mutations401727. child56,184 events·loss/health/open/lease0.
  Lexical532359833B, Prepare261371848B, Map148640926B, Diff29935047B,
  Postings70301512B는 inclusive 계수다. 단계별 bytes를 합산하지 않는다.
- lo4는145.286325초 producer1, 원 dispatch120 deadline의 BuildPrepare 실패다.
  lr4는146.810698초 producer1, 요청120.152688초 deadline이며 Postings 미도달이다.
  lr4 child637,289 events의 손실/health/open/lease0이며 각 selected stage의4계수가 있다.
  상세 시간/계수·physical IO·실패 checkpoint는 RCA가 소유한다.
- 최초 m4ctl은 build/runtime PATH 차이로 producer 전에 거절했다(NOT_RUN).
  m5ctl은 PATH-only drift를 명시하고 private executable epoch를 재검사한 별도 runtime
  cohort다. lo4/lr4는 이 runtime env와 같다. prebuilt build_binding=UNPROVEN이며
  matched-build/quiet-host speedup·overhead qualification을 주장하지 않는다.

current-policy block의 profile-first Patch 활성화와 해당 Kernel oracle은
`d8e2c52d` local snapshot에 합류했다. r5 normal cached build는 exit0이고
mr5 Medium fixed512MiB는 실제 원 bytes cap에서 FAILED다. mr5 trace에서 physical
managed reserve input5,230→1,635, inventory occurrence validation3,083→512로
감소했지만 Prepare logical bytes는261,371,848→321,302,214로 증가했다. 이 비용을
charge site/데이터 단위로 귀속했다. `admit_inventory_confirmation`은 인증된 planned
행에도 `p/n` 각각 generic max195B를 사전 charge했다. 현재 행은 `p` Null/Object,
`n` Object로 검증되므로 `key+(1|33)+33+72`로 바꾸고 read1×/PK move2×에 적용했다.
행당 예상 감소는 prior Null 1,068B, prior Object 972B다. 예산·scratch reserve·
하드 SQLite page cap·typed corruption 및 post-install ref/count 재검증은 유지한다.
실제 총 행 수와 Postings 남은 작업이 확인되지 않아 Medium 통과·CPU 개선은 미입증이다.
같은 pair의 lo5 원 Large
Off는 `BuildPrepare` deadline에서 FAILED다: request124.435900초, producer226.898901초,
accepted10,550,723R/8,042,018W/8,296,724,947B/4,545,263M; numeric/memory/
external-resource 거절은 없다. l6ctl PATH-only
environment guard와 l7ctl tool mount-device guard는 body0/NOT_RUN이다. l8ctl은
새 tool epoch를 검사했고 r5 env와 PATH-only drift를 명시한 local runtime cohort다.
`l8ctl/terminal.json`의 nested execution은 구 `lo4` 경로를 읽은 copy bug이므로
lo5 직접 `execution.json`·refusal로 판정하고 원본 요약을 덮어쓰지 않는다.
같은 r5 pair의 lr5 Record도 request120.654초 deadline FAILED다. child V5는
858,005 events written/4,698 dropped/7 open으로 PARTIAL이며 닫힌 prefix의
LexicalBuild103.728초/CPU42.419초, Prepare41.232초/CPU14.563초,
Map31.917초/CPU14.817초, Diff11.428초/CPU7.386초다. 1,280 owners가
남아 Postings 완료에 도달하지 않았다. nested duration을 합산하거나 r4/Off와
속도·tracing overhead 비율을 내지 않는다. 상세 계수와 물리 I/O는 RCA가 소유한다.
s3는 해당 실행 중 변경하지 않았다. r4 실행이 모두 끝난 뒤 동일 source build 경로를 유지하고
기존 Git commit·pair를 보존해 매번 vendor PackageID를 바꾸는45분 재컴파일를 피한다.
변경된 source/config/deps/tool의 전후 binding은 다시 검사한다.

typed-fee 구현은 `text_spool.rs`와 독립 `planned_inventory.rs` 테스트를 추가해
소유 입력15개의 `c49b2a1fa0e3dc30d88e6189ef0cc0ddbac25bfa`에 고정했다.
literal Object wire33B와 Null/Object20,884B, Object/Object20,980B의 exact fee 및
exact−1 거절 시 planned row 비노출을 원 gold로 둔다. f18은 새 테스트 표현식의
임시값 참조 수명 E0716으로 compile FAILED/body0이다. encoded 값을 지역 변수에
묶은 test-only 수정의 새 frozen source는
`a6083fb22480786d5e8172057b6fbe291af26ee7`이다. f19 terminal에서 Native293개
통과/558 제외와 Kernel lifecycle13개 통과/65 제외, 두 exit0을 확인했다.
이 source의 영향 focused proof는 VERIFIED다. r6 normal cached pair build는 exit0이고
mr6 fixed512MiB Medium Record는 FAILED다. 원120초 Large Off는 NOT_RUN이다.
큰 비용 회귀가 실패하면 해당 charge/custody 원인만 수정하고 영향 범위를 다시 검증한다.

mr6 child trace는45,923 events complete/health·loss·open0이다. InventoryPrepare
accepted bytes는 mr5의321,302,214에서266,699,646으로54,602,568 감소했고,
이는51,126 planned Null/Object 행×1,068B와 정확히 일치한다. typed fee 수리의
WorkBudget 효과는 VERIFIED지만 Postings가 더 진행한 뒤 Catalog `managed_install.rs:108`
charge 10,692B가 원512MiB cap에서 거절됐다. 2,403 physical write/3,946,025B,
물리 I/O 실패0이다. source별 시간 차이를 CPU 개선 또는 제품 성공으로 승격하지 않는다.

B03-B2의 Kernel 독립 oracle는 기존 `occurrence_blocks.rs` realstore harness를 재사용해
profile-first 초기 Patch의130개 ASCII lowercase term/260개 canonical selector가
하나의 block ref로 저장되는지, public reader의 첫 Sensitive·마지막 Folded TF/positions,
마지막 term 변경 뒤 old/current pin·재오픈을 검증하도록 추가한다. 기존 같은 파일의
두 source-order lifecycle case가 last-pin GC를 검증하므로 GC harness를 복제하지 않는다.
이 oracle와 codec/producer lane의 8KiB/512-term 정책은 frozen source
`878be6c9…`에 합류했다. f20 compile/body는 아직 NOT_RUN이고 Root가 단일 큐에서
집중 검증한다.

Root 통합 상세: `OccurrenceReader`의 build2/query64 bounded block DATA window,
`read_retained` reply의 registration+payload 동일성, exact `(ref,term)` 선택을 연결한다.
완전 검증된 confirmed block의 파생 count 재사용은 leaf scalar와 같은 DATA memo이며
ref-only scalar/spill에 block count를 쓰지 않는다. prior/query의 cache hit는 원 port의
retained-read charge/cancel 경계를 그대로 지난다. 진단 count의 full-validation 실패와
retained-read 거절을 혼동하지 않는다(후자는 span error로 관측).

B1 시점 policy gold(admission `[31;32]`)는
`eef15862a20af11bf9e7173747df2e155897d75913475bc5eb4a2ee856bdf33e`였다.
B2 현재 policy는 위 `b2c1df18…`이고, 직전 B1·leaf-only 두 policy는
validate_instance의 typed refusal oracle로 남긴다. 새-policy의 이전/current
generation·pin/reopen은 current leaf/block reader로 검증한다.

### 실행 수와 완료 기준

| 실행 | 횟수/목적 | 조건 |
| --- | --- | --- |
| r4 Medium Record | 완료 FAILED | fixed512MiB·실제4축/6계수 prefix 확인 |
| r4 Large Off | 완료 FAILED | 원120초/4,096-source 제품 기준선 |
| r4 Large Record | 완료 FAILED | Large Prepare/Map의4축과 CPU 확인 |
| B1 소유 회귀·정상 cached pair build | focused VERIFIED, build exit0 | 정확 source의 전체 qualification은 별개 |
| mr5 Medium Record | 완료 FAILED | 실제 full ACK 전 accepted536,866,122B +14,133B 거절. Prepare logical bytes 증가 귀속 필요 |
| lo5 Large Off | 완료 FAILED | r5 pair·원120초/cap, `BuildPrepare` deadline, full ACK 없음. PATH-only 별도 runtime cohort; controller 요약 copy bug 구분 |
| lr5 Large Record | 완료 FAILED | r5 pair·원120초 `BuildPrepare` deadline, child trace PARTIAL(4,698 dropped/7 open). 전체 capture·overhead 자격 없음 |
| typed-fee c49/f18 | compile FAILED/body0 | E0716 test-only 임시값 수명. 제품 코드·gold는 그대로 새 a608 source에 고정 |
| typed-fee a608/f19 | 완료 VERIFIED | exact source Native293/293·Kernel lifecycle13/13 통과, 558/65 제외. 제품 결과로 승격 금지 |
| typed-fee r6 pair build | VERIFIED | `a6083fb…` normal cached build exit0; managed-fresh 아님 |
| typed-fee mr6 Medium | FAILED | 536,864,032B+10,692B Catalog 설치 사전 charge, 원512MiB 유지. Prepare fee −54,602,568B 확인 |
| typed-fee a608 Large | NOT_RUN | 기존 d8e Large 실패를 새 source 결과로 승격하지 않음 |
| B03-B2 8KiB/512-term 합류 | NOT_RUN | `878be6c9…` 6변경/15소유 파일 코드 합류. f20 집중 배치 admission 대기·body/terminal 없음; r7 제품 미시작 |
| 정식 Off/Record overhead | 상위 E07의3쌍·동일 작업·비교 가능 호스트 | 위 제품 진단의 성공으로 대체하지 않음. host 조건 부재면 BLOCKED 유지 |

Large Off는 server dispatch120초, seed42, reads/writes/mutations 각각67,108,864,
bytes549,755,813,888을 유지한다. outer watchdog600초는 server 예산을 연장하지 않는다.
prebuilt 관측은 현재 검증한 pair 경로와 실제 SHA를 CLI에 전달하며, 원 build receipt와
source inventory의 일치 여부를 Root가 함께 대조한다. CLI의 UNPROVEN/partial 상태와
제품 lifecycle 자체의 VERIFIED/FAILED를 각각 기록한다.

완료는 다음 모두를 요구한다. Full ACK만 성공하면 끝낸 것으로 판정하지 않는다.

- 전체 SourceAdvance ACK가 원 server dispatch120초 안에 완료한다. 그 뒤 current/old
  검색, delta/noop/delete, drain/reopen과 기존 결과/점수·phrase gold를 별도로 검증한다.
- B03 패치를 선택했다면 해당 비용 단위가 실제로 제거됐고 기존 durability·memory/work·취소·fault
  거절을 보존한다. B03-B이면 증분 locality와 원 query bytes/work/latency bounds를 충족하고
  실제 read amplification을 보고한다.
- 변경한 producer→consumer 실제 UDS 경로와 독립 oracle가 nonzero로 실행된다.
- strict의 이번 소유 지적은 수정·재검증하고, 다른 변경의 전체 gate 실패는 그대로 남긴다.
  이 상태에서 workspace/release qualification을 완료로 승격하지 않는다.
- 정식 tracing overhead·원 ANN4K·XL/mixed·운영/자원·release의 기존 상위 미수용은
  해당 owner/고정 profile의 증거로만 닫는다. Native Large 통과로 함께 닫지 않는다.

검증 실행 순서는 반드시 정적/좁은 회귀를 먼저 하고 긴 normal pair build를 나중에 둔다.
긴 build가 admission을 가진 동안 대기 제한이 짧은 다른 Cargo를 새로 넣지 않는다.

## 필요한 독립 검증

| Scope | 최소 oracle |
| --- | --- |
| 계수 | 신규·prior·중복·RAM/spill·실패를 독립 입력으로 계산, 관측 없음과0을 구분 |
| 저장/검증 | 잘못된 identity·형식·count, confirm 실패, 다른 attempt, 잘못된 backend 결과 거절 |
| 수명 | memory/work 원 한도, retained release, cancel/drop/retry, 부분 성공 settlement |
| Off/Record | Off의 관측 작업 부재와 동일 제품 결과, complete footer·loss/lease/open0 |
| 실제 경로 | UDS 요청에서 현재 canonical packet과 public port/wrapper를 실제로 거침 |
| 성능 | 각 arm의 source/binary 결속, 같은 입력·작업량, 원 deadline, 완료 ACK/search/reopen |

기존74개 fresh-claim·487개 Python 회귀 등은 해당 고정 소스의 근거로 재사용한다.
변경하지 않은 전체 workspace와 별도 플랫폼을 매 패킷마다 실행하지 않는다.
전체 release·Linux 운영·XL/mixed와 정식≤5% overhead는 상위 E07/E08 분모를 유지한다.
