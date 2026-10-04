# Agent 4 — lexical 벤치·하네스 성능 RCA 인수인계

작성일: 2026-10-04, Asia/Seoul. 이 문서는 이 채팅의 작업 상태와 다음 실행 계획이다. 제품 품질·성능·배포 적격성 영수증이 아니다.

## 1. 현재 결론과 담당 범위

- 벤치 계약과 실행 경로의 여러 결함은 수정됐다. 그러나 **벤치 전체가 정식 비교에 적격하다는 판정은 아직 없다.** 최신 소스 통합 검증, 일부 실제 검수·캡처, 동일 시간 경계와 조용한 호스트에서의 성능 실행이 남았다.
- 느린 구간은 하나가 아니다. 선언 oracle/입력 검증, 제품 인덱스 생성, SDK 요청, 평가기 bootstrap, 최종 증거 검증을 분리했다. Python GC가 주원인이라는 증거는 없다.
- 현재 release Gin 진단에서는 coverage/source persistence가 큰 구간이다. macOS Rust의 `sync_all()`이 `F_FULLFSYNC`를 사용하며, 파일별 디렉터리 barrier 반복 비용을 별도 실험으로 확인했다. **엔진의 batch barrier 변경은 아직 구현하지 않았다.**
- SDK active 질의는 현재 resolve와 검색 두 RPC를 수행한다. 단일 원자적 선택·검색 요청으로 합칠 후보가 확인됐지만, activation token/ABA/cursor 불변식을 보존하는 실제 수정은 아직 없다.
- 평가기는 숫자 bootstrap 캐시까지 구현했다. 최초 cold 계산의 Python 반복 비용은 남았다. quality-only에서 불필요한 warmup을 명시적으로 끄는 정책도 실제 parity 실행이 남았다.
- 바로 앞 작업은 `publish_rca`, `search_rca`, `harness_rca`의 세 읽기 전용 병렬 RCA였다. 새 실행은 외부 루트의 파일시스템 syscall 실험뿐이었다. 이 인수인계 요청에서는 소스·영수증·티켓을 읽고 이 문서만 작성한다.

주 담당: lexical 벤치 계약/하네스 비용, 과거 Gin 실행 분해, 평가기 비용, publish·SDK·typo 성능 최적화의 근거와 완료 조건. Semantica 기능 확대나 semantic E2E를 이 lexical RCA의 선행 게이트로 두지 않는다.

## 2. 현재 checkout·소스 결속·공유 소유권

### 2.1 현재 작업 위치

- 저장소: `/Users/songmin/Documents/code-new/quanta-index`, branch `main`.
- 문서 작성 전 재확인 HEAD: `2062fed3ff86287a0914ece99dc2fe0af829c29b`.
- 공유 index에는 benchmark/ingest/search/scale/CI/dependency/vendor 및 다른 handoff 변경이 이미 staged되어 있었다. **이 overlay 전체를 이 채팅의 저작물 또는 검증 완료 소스로 취급하지 않는다.**
- 이 문서 작성은 기존 staged 파일을 수정·스테이징·커밋하지 않는다. `git add -A`, 전체 reset/revert, 오래된 snapshot을 main에 복사하는 방식은 사용하지 않는다.
- 사용자는 구현을 main에 통합하기를 원한다. clean immutable export는 실행 중 source drift 방지용이며, 별도 구현을 방치하는 제품 브랜치가 아니다. 다음 작업자는 main의 기존 구현을 먼저 대조한다.

### 2.2 실행 소스는 서로 다르다

| 증거 | 정확한 소스/역할 | 해석 범위 |
| --- | --- | --- |
| 최초 lexical 300 | Quanta `c64f6af5d5e2817e346336ceee0e57f5fb25d74f` | 원래 실패 5개 RCA. 현재 main과 혼동 금지 |
| `qg15` full pair | `d2740319ca1fcc4ec209cdce80b37e13ca00c25d`, optimized debug | generic lexical/chunk 경로. 현재 release CodeSearch/file 타이머와 합산 금지 |
| `g9` current release ingest | engine `7ff8251e2340063470bd6c58279528df4428dee8`; Python driver `33f8f16d5c712e9ac90a735645cb180a4fee5de9` | Gin 99파일/2질의, schema 9. driver 수정과 binary build source를 분리 |
| ASCII A/B | candidate `7ff8251e`, baseline `18187058` | scanner 분기 한 파일 차이. 20캡처, lane별 arm당 2 fresh roots, diagnostic |
| B09 global12 과거 5제품 | binary source `d7063ac755916d48867416d4f970b6aebc360abd`; gold/native driver `6f3e04856a03291098497c58c9a240ad217d9ee8`; external collector `6f8feb90e7012e79fe78a45c588780eeb59ed779` | 12저장소 OSA1 비교. 위 7ff 성능 진단과 별개 |
| B09 후속 structural capture | `d1a1b7097c5c915afd8fb24467c6d737acdb3b14` | Quanta policy/ranking/NL/parser 수정 후 새 실행. 과거 외부 응답과 동일 시점 실행 아님 |

7ff immutable source 위치는 `/private/tmp/qi-perf-final-source-20261004-2bkgywrr/quanta-index`다. 현재 main의 후속 변경 전체를 이 snapshot의 통과 결과로 승격하지 않는다.

`g9` binding: [CONTROL.json](/private/tmp/g9-7m7inh5r/CONTROL.json).

- SDK context SHA-256: `4a19dde48f2dbebc2b00c6ebf63c84d7a9ffbbc4a925fa9538f5e23dfb7d7a62`.
- Runner SHA-256: `cd594d365b2d3ceb68fd9a2f3c68dc37fd1df56f725465f37901bff9c100dd86`.
- Searchd SHA-256: `daf899f8c493592d333349313a49f5d7efa67b00567a52dc7a0fc23f7752bb6f`.
- Spec SHA-256: `100dc3adeb190bccb9dd364c833f94e59f3611b0b65748651c6e8d41edac1a6c`.
- Binaries: `/private/tmp/qi-perf-release-20261004-7ff8251e-r4u4jix4/sdk/target/release/`.

## 3. 세션 목적의 변화와 유지해야 할 벤치 계약

### 3.1 출발점

원래 범위는 lexical-only 300-query Gin의 5개 gold-file miss였다.

| Task | 질의 | 원래 gold 파일 | 원래 top10에서 놓친 제품 |
| --- | --- | --- | --- |
| S027 | Param | context.go | Quanta, Semble, OpenGrok |
| S040 | mappingByPtr | binding/form_mapping.go | Quanta, Semble |
| S122 | Type | testdata/protoexample/test.pb.go | Quanta, Semble, OpenGrok |
| S155 | Next | context.go | Quanta, OpenGrok |
| S273 | writeContentType | render/render.go | Quanta, Semble, Sourcegraph, OpenGrok |

Gin source: `d3ffc9985281dcf4d3bef604cce4e662b1a327a6`, `code_only` 99파일, file-universe SHA-256 `d4e1ea025c067f344af640568b5bfd0835ed09c2bfbcf9668b8e9782bc68bd67`.

원래 결과는 Q295/S296/SG299/cs300/OG296, 분모 300이었다. 실행 verdict 600/600/600, failed 0이며 Quanta capped 11개 중 5개가 gold 파일을 놓쳤다. `capped` 자체는 실행 실패나 gold 누락이 아니다. Quanta/Semble 10청크와 다른 제품의 10고유파일도 같은 출력 예산이 아니다. 범용 이름의 gold 유효성, 동명 선언·사용처, 후보/순위/단위를 분리해야 한다.

원본 `/Users/songmin/Documents/code-new/qi-smoke-gin-300-20260928`와 `/private/tmp/g3`는 수정·덮어쓰기하지 않는다. 새 재현은 새 외부 출력 루트에 둔다.

### 3.2 현재 평가 범위

- 기본 Gin exact는 **1,196질의**다. 선언 이름 1,296개에서 중복/유사 질의 100개를 제외했다. 기존 300 대비 896개 증가이며, 이 확장 자체는 여전히 같은 99파일이다.
- 최신 Gin release 진단의 exact/insertion/deletion/substitution/transposition은 각각 **1196/1192/1178/1192/1192**, 합계 5,950 tasks다. 원본 질의 가족을 독립 표본으로 중복 계산하지 않는다.
- prefix, infix, components, 네 오타 유형, no-answer, NL, 공식 Gin 20, ARB는 별도 suite·분모·결과표다. 모든 lane을 하나의 순위로 합치지 않는다.
- 내용/청크, 내용/고유파일, 선언/심볼 계약을 분리한다. 반환 문맥 span을 선언 name span의 정답으로 삼지 않는다. 파일 Hit@10은 선언 회수 증거가 아니다.
- 기본 검색과 명시적 OSA1 회복은 별도 요청 정책이다. Sourcegraph 등의 ordinary keyword 요청 결과를 모든 fuzzy UI/API 기능의 성능으로 확대하지 않는다.
- 정상 empty는 유효한 검색 응답이다. error/timeout/partial/unsupported/missing/unknown relevance를 분리한다. 적격 capped는 포함하며, 부족한 partial을 정상 top10으로 취급하지 않는다.
- 조건부 품질은 공통 적격 task ID 집합에서 비교한다. 운영 점수·제외 ID/사유·coverage도 함께 낸다. 제품마다 성공 표본만 골라 평균 내지 않는다.
- 같은 이름의 모든 선언 파일과 첫 대표 파일 정답은 다른 지표다. gold 계약 변경으로 오른 수치를 엔진 개선으로 발표하지 않는다.
- 사용자는 독립 AI 검수로 사람 작업을 상당 부분 대체하는 방향을 승인했다. 실제 reviewer/조정자 호출·근거·unresolved 상태를 보존한다. AI 검수를 human provenance로 바꾸거나 현재 human gate가 통과했다고 쓰지 않는다.
- breaking change가 가능하다. 기존 IR/planner/evaluator를 진화시키며 호환 계층·병렬 v2 IR·두 번째 하네스를 만들지 않는다.

## 4. 구현되어 있는 작업 — 다시 만들지 말 것

| 작업 | 기존 소유 파일/커밋 | 확인된 검증과 남은 경계 |
| --- | --- | --- |
| source oracle 반복 감소 | `source_oracle.py`, declaration census, batch source snapshot | source/digest/parser identity 결속 캐시. SymPy profile 66.234→27.598초는 diagnostic. 당시 oracle/census 90개, caller 130개 통과. 새로운 main 전체의 성능 증거 아님 |
| quality-batch/matrix | `execution_batch.py`, `run.py`; `9a3412c6`, `fca3cd9e` | 저장소·제품당 native union 1회 indexing, 원래 suite/member map/채점 분리. `/private/tmp/qm3`: 8 적격 저장소, 3699 union tasks/product, 7400 projected rows/32 reports parity, 8/8 replay |
| driver closure 재사용 | `run.py`, closure owner; `0aeed2aa` | matrix 첫 verified closure 재사용. 중간 중복 full verification 제거, 최종 full verification/독립 replay 유지. 이미 구현된 재사용을 다시 제안하지 않음 |
| driver 단계 타이머 | `5669b9d2`, 기존 phase/resource owner | source/setup/product/scoring/verdict 구간 분리. 타이머 자체는 verdict authority 아님 |
| stale runner 조기 거절 | `28c883db` 및 runner capability probe | expensive gold/product 작업 전 필요한 schema/capability 확인. old binary를 현 source SHA로 표시하지 않음 |
| debug 해시 profile | Cargo profile의 sha2 opt-level 3 | 같은 소스 118 native rows 비시간 parity; unattributed 8.431→1.006초 diagnostic. release 개선 claim 아님 |
| stale suite/pack 선검증 | `run.py`, `7a046b55` | product 실행 전 frozen suite와 blind pack 검증. old v1 gold는 이전 54.92초 이후 거절, 수정 후 4.35초 내 product 없이 거절 |
| suite/spec route 선검증 | `run.py`, `f80a412e`에 포함 | 잘못된 route 때문에 양 제품 55.70초 실행 후 merge 거절하던 경계 수정. focused negative check 통과 |
| 숫자 bootstrap bounded cache | `evaluator.py`, `203ef446`; inventory `21901f2a` | 16 entries, key당 최대 262144 bytes, canonical key 한도 약 4MiB. 순수 숫자만 캐시; source/evidence 검증은 캐시하지 않음. fixed 1196-row parity/cache-hit unit 통과 |
| 반복 출력 identity | Rust runner/Semble producer, `run.py`, completed-response owner | cold/warmup/measured 모든 phase를 retained normalized row/status에 결속. score는 f64 bits. hash는 타이머 밖. 같은 크기 다른 후보 거절. 과거 timing은 이 추가 증거가 없음 |
| ingest/query 세부 계측 | ingest contract/adapter, runner/SDK/plane diagnostics | 현재 schema 9 / protocol lock 7 / phase 4. child bounds, observation on/off, real SDK seam을 고정 소스에서 확인. 33f Python direct capture 누락 플래그 수정 포함 |
| delta/reuse 구조 | lexical generation/coverage/file/text authority | touched shard, inherited coverage, digest skip/hardlink 경로가 이미 있음. 새 증거 없이 재구현하거나 TOCTOU preflight를 제거하지 않음 |

이 표의 tests는 **이전 소스별 실행 기록**이다. 문서 작성 turn에서 다시 실행한 tests가 아니다. `7a046b55` 같은 공유 파일 커밋에는 동시 작업자의 hunk가 함께 들어간 이력이 있으므로 저작권/소유권도 diff로 확인한다.

과거 C5의 immer/rust-analyzer/tauri/telegraf 4개는 parser census가 바뀌어 예전 exclusion/gold가 stale했다. `/private/tmp/qm3`는 그 예전 12저장소 입력 전체를 재생한 결과가 아니다. **후속 global12의 새 12저장소 실행은 실제 존재한다.** 두 population을 합치거나, 모든 최신 12저장소 실행이 막혔다고 설명하지 않는다.

## 5. 하네스 비용 RCA — 과거 full 1196 pair

증거: [qg15 stage clocks](/private/tmp/qg15/driver-stage-timings.json), [verdict](/private/tmp/qg15/verdict.json), [run manifest](/private/tmp/qg15/run-manifest.json).

`d2740319` optimized-debug pair는 Q/S 각각 1196, selected/executed/passed 2392/2392/2392, failed 0, `PAIR_VALID=pass`였다. `CONTRACT_GREEN`/`SDK_PATH_GREEN`은 이 영수증에서 not_run, PERF/QUALITY claim은 not_applicable다.

| 전체 driver 약 134.75초의 구간 | 초 | 의미 |
| --- | ---: | --- |
| product envelope | 90.775 | Q82.823 / S7.644; 자식 단계와 중복 합산 금지 |
| report scoring | 15.808 | 평가 계산 |
| independent verdict | 19.833 | 원본 재계산·검증 포함 |
| final source closure verify | 5.586 | 별도 final check. 앞의 stage_wall과 전체 wall은 같지 않음 |

Quanta runner 81.379초: publish/seal/activate 41.243초, 측정 warm 질의 18.332초, warmup 18.313초, boot 1.547초, unattributed 1.256초. cold 1 + warmup 1196 + measured 1196 = 2393 observations다. 측정 SDK execute 17.918초, server lexical.search 14.262초, 결과 materialization 0.274초, post-SDK 0.014초. 서로 다른 clock 차이를 IPC로 확정하지 않는다.

Semble process resource 7.511초 대 worker 1.447초(index .602 / warmup .394 / measured .370). **약 6.064초는 아직 process 내부의 귀속 공백**이다. import/env/pip-freeze, 약 32MB 모델 snapshot/digest, corpus copy, worker startup, validation/record assembly 후보를 실제 child clock으로 분해해야 한다.

source closure 1125파일/약 53.58MB의 별도 profile에서 verify 4.783초 중 root resolution 3.725초(Cargo metadata 1.742, Python import 1.924), AST parse .485초, walk .752초였다. SHA primitive .071/.024초가 주비용이라는 근거는 없다. 최종 source check를 삭제하는 최적화는 권한/무결성 경계를 깨뜨린다.

bootstrap profile은 약 6.38억 calls/67.9초(profile overhead 포함), mean_ci 54.639초, bootstrap 54.329초, randrange 51.829초였다. 10,000 draws를 Python 원소 루프로 반복하는 비용이다. bounded cache 후 같은 process diagnostic은 cache-off 20.454초, cold-cache 18.979초, repeat 5.551초였고 verdict bytes는 같았다. OS/file cache와 부하가 달라 전체 차이를 숫자 cache에만 귀속하지 않는다.

qg15 verdict SHA-256: `65277fc45f1e3d7fa5390a50430139af03e858be3da779edd520c6b3f0fdfcd7`. current 203ef replay도 동일했다. full pair는 release speed 증거가 아니다.

## 6. publish/index 비용 RCA — 현재 release

`publish`는 빌드가 아니다. SDK로 source/chunk/symbol을 전달하고 lexical/semantic generation artifact를 생성·검증·seal하는 작업이다. `activate`는 별도 catalog CAS다. Rust compile은 그 전에 runner/daemon executable을 만드는 작업이며 인덱싱 시간에 넣지 않는다.

기존 호출 경로:

- [SDK lexical publish](/Users/songmin/Documents/code-new/quanta-index/crates/quanta-index-sdk/src/lexical.rs:414)
- [plane search_corpus](/Users/songmin/Documents/code-new/quanta-index/crates/quanta-index-search-plane/src/ingest_dispatcher/search_corpus.rs:461)
- [adapter_ingest](/Users/songmin/Documents/code-new/quanta-index/crates/quanta-index-lexical/src/adapter_ingest.rs:299)
- [adapter_open](/Users/songmin/Documents/code-new/quanta-index/crates/quanta-index-lexical/src/adapter_open.rs:82)

`g9`의 실제 release 99파일/2질의 관측:

| 구간 | 초 | 포함 관계 |
| --- | ---: | --- |
| SDK publish | 3.581142 | 최상위 |
| SDK activate | 0.146994 | publish와 별도 |
| lexical build | 3.133278 | publish 안 |
| preparation | 1.020992 | lexical build 안 |
| coverage persistence | 0.975685 | preparation 안 |
| file authority | 1.386603 | lexical build 안 |
| source persistence | 1.370929 | file authority 안 |
| text authority | 0.398059 | lexical build 안 |
| shard construction | 0.331405 | text authority 안 |
| writer mutation / seal | 0.183066 / 0.103245 | lexical build 안 |
| empty semantic-track work | 0.103783 | publish 안 |

coverage+source persistence = 2.346614초, SDK publish의 약 65.5%다. busy-host 1회 관측이므로 실제 write/sync causal 개선은 후속 엔진 A/B가 필요하다. parent와 child 시간을 더하지 않는다. preflight는 .029642초이며 phase 간 source mutation을 잡는 실제 테스트가 있다. 이 비용부터 제거하지 않는다.

### 6.1 새로 확인한 플랫폼 원인

Rust 1.92 Apple 구현에서 `File::sync_all()`은 `fcntl(F_FULLFSYNC)`다. 설치된 [stdlib 구현](/Users/songmin/.rustup/toolchains/1.92.0-aarch64-apple-darwin/lib/rustlib/src/rust/library/std/src/sys/fs/unix.rs:1235)을 직접 읽었다. source SHA-256은 `2357ba5167bea291b690db9d0b75f69fd9bc2be332a4e27c1da213dcd0273c20`.

[index_store.rs](/Users/songmin/Documents/code-new/quanta-index/crates/quanta-index-lexical/src/index_store.rs:127)의 기존 durable write는 파일 full-sync → rename → 부모 디렉터리 full-sync다. `g9` generation의 coverage page 81 + root 1, source blob 99 + manifest 1 = **182 atomic writes / 해당 쓰기만 364 full-sync calls**다.

coverage page logical bytes는 48306, sources/manifest는 700292다. 파일시스템 allocated blocks와 다른 값이다. 아래 실험의 `data_bytes=728925`는 root/manifest publication을 별도 취급한 값이므로 두 inventory 합과 같은 키로 해석하지 않는다.

### 6.2 격리 syscall ABBA 실험 — VERIFIED diagnostic

[REPORT.json](/private/tmp/qi-fullsync-rca-8v3e3mn_/REPORT.json), [PROVENANCE.json](/private/tmp/qi-fullsync-rca-8v3e3mn_/PROVENANCE.json), [상세 액션리스트](/private/tmp/qi-fullsync-rca-8v3e3mn_/ACTION-LIST.md).

실제 182 artifacts를 새 외부 디렉터리에 복사했다. Python `fcntl.fcntl(fd, F_FULLFSYNC)`로 Rust Apple과 같은 flush primitive를 사용했다. B도 모든 182 file full-sync를 유지하고, 그룹의 page/blob rename 후 root/manifest 공개 전 barrier 및 공개 후 barrier로 디렉터리 full-sync를 4회로 줄였다.

| Arm | 두 표본 wall 초 | file full-sync | directory full-sync |
| --- | --- | ---: | ---: |
| A, 파일별 directory barrier | 1.798740 / 1.890440 | 182 | 182 |
| B, 그룹별 directory barrier | 0.906228 / 0.930871 | 182 | 4 |

평균 1.844590→0.918550초(-50.2%). file full-sync .799–.877초는 유지됐고 directory full-sync .905–.910→.0185–.0194초로 줄었다. 네 output inventory 모두 원본 bytes와 같았다.

**확인된 것은 syscall 구조의 비용이다.** 엔진 publish가 2배 빨라졌다는 결과, crash/power-loss proof, 조용한 호스트 추정값은 아니다. 실제 batch writer·publication 순서·오류 주입 tests는 `NOT_RUN`이다.

첫 실험 `/private/tmp/qi-durable-write-rca-vumyf_ow`는 Python `os.fsync`를 사용해 Apple Rust와 동등하지 않았다. `NOT-EQUIVALENT.md`와 raw를 보존했으며 그 작은 시간은 근거에서 제외했다.

## 7. 검색/SDK 비용 RCA와 미확정 항목

### 7.1 active resolve와 검색

[client.rs](/Users/songmin/Documents/code-new/quanta-index/crates/quanta-index-sdk/src/client.rs:302)는 active selector를 resolve한 후 Text query를 보낸다. plane은 이미 [GenerationSelector::Active](/Users/songmin/Documents/code-new/quanta-index/crates/quanta-index-search-plane/src/query_dispatcher/selection.rs:16)를 지원한다. 그러나 응답 pin만으로 activation ABA까지 해결되는 것은 아니다.

같은 release binary의 [i9 VERIFY](/private/tmp/i9-lhyxoxdp/VERIFY.json):

| 2개 representative queries | 1 | 2 |
| --- | ---: | ---: |
| SDK execute ms | 3.855 | 3.384 |
| active resolve RPC ms | 1.643 | 1.530 |
| client read intervals 합계 ms | 3.620 | 3.259 |
| server resolve/text validated→written us | 84 / 707 | 29 / 262 |

zero dropped events와 pack/record/phase/parent bounds는 확인됐다. server accept/frame decode/prevalidation이 이 server clock에서 빠져 있다. resolve 호출은 제거 가능한 작업의 위치를 보여주지만 절감 예상치를 확정하지 않는다. client read는 server 대기와 local decode/read 경로를 포함한다.

### 7.2 typo와 일반 검색

- qg8 release exact 1196 SDK 합계 약 3.575초, server .111초, candidate .0496초다. kernel만 개선해서 전체 호출의 큰 배속을 얻는다는 근거가 없다.
- insertion 1192 SDK 약 4.255초, candidate 1.180초, token scan .884초다. typo candidate에서는 source token scan이 약 75%다. 여러 질의가 같은 source를 다시 tokenize하는 구조가 다음 후보다.
- 현재 gram shortlist는 distinct grams−4를 사용하고 짧은 이름은 full scan fallback이 있다. 단순 length/gram heuristic을 완전한 OSA1 후보 생성으로 주장하지 않는다.
- generation-bound trigram authority와 query distance cache는 이미 있다. 제안하는 **distinct identifier token dictionary/postings**는 이 기존 gram dictionary와 다른 단위다. 동일 기능을 복제하지 않는다.
- qf4a candidate/preview 합계 132.334/119.973ms, sort/page 7.813ms/1196calls. selected page 뒤 preview 생성이 이미 구현되어 있다. 이 자료로 top-k 전체 재작성부터 시작하지 않는다.

### 7.3 ASCII scanner의 혼합 결과

[A/B audit](/private/tmp/a9-21c9z44a/audit-current/REPORT.json): 20 release captures, 5 lanes, arm당 2 roots. 17850/17850 normalized output 비교와 raw 비시간 진단/work counters가 일치했고 총 scored rows는 23800이었다.

| lane | whole completed-call 합계 평균 변화 |
| --- | ---: |
| exact | -1.58% |
| insertion | -1.69% |
| deletion | **+8.75%** |
| substitution | -2.70% |
| transposition | -6.11% |

네 typo lane의 scan stage는 빨라졌지만 deletion 전체 호출은 느려졌다. busy host/2 roots이므로 개선·회귀 원인을 확정하지 않는다. deletion 재측정과 현재 ASCII prepass의 전체 비용을 먼저 판정한다. `is_ascii` 전체 prepass 후 token scan의 이중 순회 가능성을 포함한다. scanner 최적화는 아직 성능 적격이 아니다.

## 8. 남은 구체 액션리스트

### A4-01 — generation 단위 durable write batch [P1, 미구현]

- 소유: `index_store.rs`, `sealed_generation/coverage_pages.rs`, `file_authority.rs`. canonical writer를 진화시킨다.
- staged content 파일의 full-sync·rename은 유지한다. 모든 content rename 뒤 그룹 directory barrier를 완료한 다음 root/manifest를 공개한다. root/manifest 자체 barrier, 오류 전파, sealed identity 최종 공개를 유지한다.
- 독립 unit/fault fixture: write/file-sync/rename/group-barrier/root-publication 각각 실패, old 또는 완전한 new root만 reopen 가능, 참조된 page/blob 전부 존재·digest 일치, barrier 실패 시 seal/activate 불가.
- symlink/partial/source mismatch 거절, fresh/delta/delete/no-op를 independent fresh rebuild와 대조한다. fsync 삭제나 무시를 성공으로 삼지 않는다.
- functional closure 뒤 실제 engine release Gin ingest A/B. 위 syscall 실험을 새 엔진 결과로 대체하지 않는다.
- 조건부 후속: file full-sync 자체가 계속 지배하면 immutable source/coverage pack을 검토한다. digest→offset/length, bounded read, torn pack/GC/inherited reference 검증이 필요한 큰 storage change이므로 batch barrier와 한 번에 묶지 않는다.

### A4-02 — 단일 RPC의 active 선택·검색 [P1, 미구현]

- 소유: SDK `client.rs`, plane `query_dispatcher/selection.rs`·`planning.rs`·route, IPC request/response contract 및 SDK binding.
- 한 catalog snapshot에서 `{generation pin, activation token}` 선택 → 선택한 view 검색 → 그 선택을 응답에 결속 → SDK의 domain/token/row 검증. 기존 pre-resolve를 단순 삭제하지 않는다.
- unit/controlled concurrency fixture: A→B→A, concurrent activate, explicit pin/token conflict, cursor continuation, eviction/stale generation, rev-at-time ancestor domain, credentials/deadline/cancellation/reconnect/malformed response.
- independent atomic snapshot expectation으로 rows/order/count/status/span/identity 확인. active를 fixed-pin으로 바꾸고 같다고 주장하지 않는다.
- 같은 release source/binary/input으로 representative query → full exact lane A/B. 2개 샘플만으로 millisecond 절감 보장하지 않는다.

### A4-03 — quality-only warmup 정책 [P1, API는 존재·실제 검증 미완료]

- `run.py`는 `query_warmup_passes=0`을 이미 허용한다. 기본 spec 경로 일부는 1이다. 새 mode/harness를 만들 필요가 없다.
- quality-only spec producer에서 0을 명시하고 성능 claim을 금지한다. speed qualification의 warmup/repetition/fresh-root 요구는 유지한다.
- 작은 fixed fixture의 0-vs-1 warmup normalized result parity, request/phase ledger, speed-mode zero-warmup 거절 unit을 먼저 실행한다.
- qg15에서 피할 수 있는 warmup phase 합계는 Q18.313+S.394=18.707초였다. 실제 새 wall 절감은 아직 측정하지 않았다.

### A4-04 — cold bootstrap의 bounded vectorization [P1, 미구현]

- 소유: 기존 `evaluator.py`의 `mean_ci`/bootstrap, `test_retrieval_benchmark.py`, 해당 proof inventory. pure numeric cache는 유지한다.
- paired/stratified metric vectors를 bounded batches로 처리하고 stratum별 draw indices를 공유한다. 10000 resamples와 선언된 결정적 통계 방법을 유지한다. source/evidence check는 건너뛰지 않는다.
- 독립 fixed draw-index golden과 별도 scalar/SciPy reference로 mean/difference/CI를 비교한다. task order, strata, finite checks, degenerate group, percentile interpolation, memory bounds를 검증한다.
- RNG 알고리즘 변경은 동일 seed의 결과 bytes를 바꿀 수 있다. 단일 기존 method contract를 명시적으로 진화시키고 과거 byte parity를 주장하지 않는다.
- cold numeric compute를 I/O/replay와 분리해 측정한다. warm cache 결과만으로 first-run 병목을 닫지 않는다.

### A4-05 — typo token 재사용과 ASCII deletion 판정 [P1, 미구현/미판정]

- 소유: `searcher/code_search.rs`, `file_authority.rs`. Agent 5의 declaration evidence/ranking/Unicode 변경과 먼저 파일 소유권 조율.
- 먼저 deletion 전체 호출 반복 측정으로 scanner 유지/수정/철회 판정. substage 개선만으로 유지하지 않는다.
- 다음 설계: source/generation-bound distinct tokens → file memberships + verified byte spans/witness. raw spelling, casefold/Unicode mapping, source digest와 declaration attestation을 보존한다.
- exhaustive independent tokenizer/full-DP OSA1 oracle: 네 edits, short names, case, mixed Unicode, exact-name collision, ambiguous names, 모든 matching files, span/count/order/cursor/budget/cancellation, stale digest.
- build/cold-open/resident memory/delta/delete 비용을 함께 평가한다. shortlist completeness를 증명하기 전 complete fallback을 제거하지 않는다. token index는 검색 비용을 ingest로 옮길 수 있다.

### A4-06 — Semble process gap 및 외부 시간 경계 [P2, 귀속/통합 미완료]

- Semble owner: `tools/benchmark/retrieval/semble.py`와 기존 resource/phase schema. env/import/pip-freeze, corpus/model snapshot+digest, worker startup, native validation/record assembly를 parent-bounded clock으로 나눈다.
- 반복적으로 큰 중복 immutable 작업만 제거한다. model/source 변경 사이 refusal, child bounds, native rows parity를 고정 fixture로 닫는다.
- 외부 HTTP/process의 complete normalized-response clock은 Agent 2 소유다. transport-only `elapsed_ms`를 이름만 바꿔 공통 clock으로 승격하지 않는다.
- 실제 동일 사용자 출력 경계 성능 비교는 B07과 단일 실행권으로 수행한다. Q SDK/IPC, S worker BM25, 외부 service call의 기존 값에 배속 순위를 붙이지 않는다.

### A4-07 — scale 및 최종 성능 실행 [P2, 새 소스 runtime 미완료]

- 기존 B07/J7Q-03/J7Q-04 및 scale/open-loop harness를 사용한다. 새 하네스 금지.
- 현재 defaults: scale history 2 generations, open-loop 8, 16MiB history budget. 300s deadline/256MiB는 별도 diagnostic profile로 기록하며 자동 상향·재시도로 기본 성공을 만들지 않는다.
- 이전 `582cb7a5` 256파일 medium은 full/delta/delete/same-process reopen 진단 통과했다. 4096은 timeout, 별도 300s profile은 retention exhausted, 32768은 posting cap admission refusal였다. 이를 성공 scale timing으로 표시하지 않는다.
- 7ff 이후 변경에 맞는 새 256/4096/32768 실행은 별도 게이트다. same-process reopen과 OS restart/cold-cache를 구분한다.
- resource sampled RSS는 true peak가 아니며 allocated disk size는 physical write I/O가 아니다. probe/parent CPU 경계도 함께 기록한다.
- owner-local tests 뒤 release binaries 한 번 freeze/build하고 무거운 captures는 직렬 실행한다. 기존 계약의 arm별 최소 5 fresh roots/route별 1000 warm observations, randomized paired blocks, 지속 host admission, 사전 effect/uncertainty 판정을 적용한다. 이는 로컬 B07 규약이며 보편적 표본수 법칙이 아니다.

### A4-08 — 벤치 qualification·미실행 셀 [Agent 2/3/5 연계]

- 최신 source/input/request/unit/label contract의 required-cell inventory부터 고정한다. Gin exact1196, prefix/infix/components/오타4/no-answer/NL/워크플로우 별도 scoreboard.
- 실제 검수 실패/unresolved 보존 후 유효 cache만 재사용해 재개한다. 미검수→0점/no-answer 치환 금지. AI reviewer/조정자 실제 호출과 human provenance는 구분.
- five-product fresh cells, Sourcegraph/OpenGrok의 **해당 universe와 query 전후** native index attestation, 새 union 검수와 최종 qrels freeze가 필요하다. 다른 population의 content probe로 과거 캡처를 소급 승격하지 않는다.
- 파일 Hit와 정확한 name span 회수 평가를 각각 닫는다. file hit만 좋으면 오타가 선언을 정확히 찾았다고 쓰지 않는다.
- 이미 튜닝에 노출된 Gin/public/global12를 unseen holdout으로 쓰지 않는다. 새 split/source 가족/저장소 독립성을 사전 고정한다. full CoIR/CORE/CSN 도입 완료 claim은 아직 없다.

## 9. 다른 채팅의 최신 결과와 중복 작업 경계

### 9.1 최신 B09 실행은 존재하나 성능 RCA와 다른 소스

[global12 5제품 결과](/Users/songmin/Documents/code-new/qi-b09-final-20261004-SQoHAA/RESULTS.md)는 12저장소/11695파일/4363 OSA1 queries, 공통 적격 4363, 원본 응답 21815개를 보고한다. 네 edits는 1114/1073/1087/1089다. 일반 파일 검색 요청 비교이며 fuzzy UI/명시적 typo 기능 전체 비교가 아니다.

과거 global12 의도 선언 파일 Hit@10: Q4289, S3037, SG140, cs138, OG0 /4363. 호출 합계는 Q82.012/S120.571/SG647.702/cs587.764/OG75.104초다. 부하·타이머·제품 topology가 달라 qualified latency 순위는 없다.

같은 report의 Q lexical build 합계593.223초는 publish/seal/activate727.975초 안에 포함된다. S from_path188.792초는 BM25+vector이며 청크 수도 다르다. pair wall5500.863초와 external wall6527.366초는 병렬·중첩했으므로 합쳐 경과시간으로 쓰지 않는다. gold1996.263초/matrix3484.876초/compile3449초도 각 측정 경계다.

[후속 structural 결과](/Users/songmin/Documents/code-new/qi-b09-structural-fix-20261004-ji1PLR/RESULTS.md)는 새 Quanta default OSA4340/4363, explicit OSA4363/4363, Gin exact1192/1196을 보고한다. 이전 external 결과와 같은 engine capture가 아니다. 여러 engine revisions가 바뀌었으므로 단일 인자의 causal gain으로 발표하지 않는다.

후속 NL은 common CLARC425에서 original96/renamed84, 새 admitted444에서는98/85, CSN positive-known408에서345다. 과거63/41/307과 source/profile/분모를 대조해야 한다. CSN submitted462 중54 quality unknown, 전체573 중111 source-blocked다. unseen/human/precise-span/performance qualification은 여전히 별도다.

이 문서 turn은 위 report를 읽고 source 역할을 대조했다. 이 turn에서 21815개 native responses를 모두 재생하거나 새 제품을 실행하지 않았다.

### 9.2 인수인계 간 소유권

| 담당 | 영역/현황 | Agent 4의 경계 |
| --- | --- | --- |
| [Agent 1](/Users/songmin/Documents/code-new/quanta-index/docs/handoff/oct-4/agent-1.md) | semantic 원래300 RCA/제품 문서 정리 | semantic miss/ANN/encoder 결론과 lexical performance를 혼합하지 않음 |
| [Agent 2](/Users/songmin/Documents/code-new/quanta-index/docs/handoff/oct-4/agent-2.md) | 검수·C3 admission·다제품 capture/join·외부 complete timer | 새 검수/외부 collector를 중복 시작하지 않음. 성능 timer/실행권 조율 |
| [Agent 3](/Users/songmin/Documents/code-new/quanta-index/docs/handoff/oct-4/agent-3.md), 코퍼스/하네스 | `holdout_review.py`, `execution_batch.py`, scheduler, ingest/SDK/scale 접점 | batch/review 소스를 재구현하지 않음. A4-01/02/07 착수 전 실제 파일 소유권 협의 |
| [Agent 5](/Users/songmin/Documents/code-new/quanta-index/docs/handoff/oct-4/agent-5.md), 엔진/벤치 계약 | parser/Unicode/IR/planner/default/explicit typo/ranking/source-lock/fresh join | `code_search.rs`/file authority 및 request contract 동시 편집 금지. 기능 수정과 성능 실험 분리 |
| commit/push 담당 | 공유 변경 publication | 이 문서 작성이 공유 staged overlay의 일괄 commit/push를 의미하지 않음 |

Agent 2/3/5 handoff를 이번 turn에서 읽었다. Agent 2 기록은 원본 C3 7저장소×20=140/240 suite 발행, 실패5저장소와 supplemental375쌍 미완료를 명시한다. 이전 진행 중 설명 대신 actual terminal을 다시 확인해야 한다. Agent 3의214쌍 payload preflight 통과는 실제 모델 검수 완료가 아니다. Agent 5의 후속 hardening은 runtime join/AST-source/Unicode 요청 결속을 보강했으며, 이전 7ff 성능 실행에 소급 반영하지 않는다. 현재 PID 생존도 완료 증거가 아니므로 재개 전에 owner/terminal/생성 셀을 확인한다.

## 10. 병렬 진행 순서와 중지 조건

1. main HEAD/dirty, Agent 2/3/5의 최신 소유 파일과 실행 중 jobs를 확인한다. old export를 main에 덮어쓰지 않는다.
2. **durability**, **SDK atomic selection**, **harness/evaluator**를 독립 owner로 진행할 수 있다. 공통 schema/driver/proof inventory는 단일 통합 owner가 반영한다.
3. typo dictionary/ASCII는 engine owner의 ranking/Unicode 변경을 먼저 소비하고 한 파일 소유자가 작업한다. storage pack은 batch barrier 결과 뒤 조건부다.
4. 각 변경은 independent fixed expectations의 positive/negative unit부터 닫는다. 느린 전체 회귀를 반복하는 대신 실제 바뀐 authority와 실패 경계의 좁은 test를 실행한다.
5. 통합 source를 freeze하고 release runner/daemon 한 쌍을 build/verify한다. 제품 타이밍 A/B를 병렬로 돌려 CPU/I/O 간섭을 만들지 않는다.
6. representative2queries 및 ingest full/delta/delete/no-op/reopen parity → 영향 lane full1196+네 typo → 필요한 multirepo cells 순서다. prefix/NL/no-answer는 별도 계약/실행.
7. source/binary/input/request/gold가 변하면 영향 증거를 새 루트에서 다시 만든다. original300/1196/oldC5 결과와 새 population 집계를 섞지 않는다.
8. 결과/status/span/identity 손실, sync 오류 무시, underfilled partial 정상화, host admission 누락, whole-call 악화가 확인되면 최적화 patch를 수정/거절한다. substage 개선이나 compile 성공만으로 완료하지 않는다.

## 11. 검증 상태와 실행 명령

### 11.1 완료 범위

| 범위 | 판정 | 실제 의미 |
| --- | --- | --- |
| 앞선 세 병렬 source/capture RCA | VERIFIED | 읽기 전용 publish/search/harness 분석 |
| installed Rust sync primitive와182 artifacts | VERIFIED | source/원본 inventory 확인 |
| isolated F_FULLFSYNC ABBA | VERIFIED | 4표본, bytes parity. engine/crash proof 아님 |
| qg15 exact1196 pair·독립 verdict | VERIFIED diagnostic | 2392응답, frozen optimized-debug 계약 |
| frozen7ff/181 fresh release SDK | VERIFIED | 각각25/25, portable context 검증. 현재 overlay 전체가 아님 |
| ASCII20captures의 raw/phase/출력 parity | VERIFIED diagnostic | 성능 개선 판정 아님 |
| 새 durability/단일RPC/vector bootstrap/token dictionary | NOT_RUN | 제안이며 구현/단위테스트/actual engine A/B 미완료 |
| 최종 현재 main 전체 tests·quiet-host speed·new-scale qualification | NOT_RUN | 개별 owner의 과거focused tests와 다름 |
| 모든 준비 lane의 새5제품 capture·precise declaration span·unseen holdout | NOT_RUN | 일부 실제 실행/검수는 존재하지만 전체 완료 아님 |
| 배포/정식 release 적격성 | NOT_RUN | 본 RCA 목적 밖. 벤치 binary 복사 배포 금지 |

당시 B07 owner 기록의597 Python, ASCII6, ingest8, scale29, open-loop20 및4fuzz targets×60s 통과는 개별 frozen/overlay 범위다. 6194-document test는 interrupted이며 통과가 아니다. Rust/Python inventory188/738은 collection 수이며 전체실행 수가 아니다. 후속 소스별 실패·repair·partial/full tests도 B07 원문을 따른다. 수를 합쳐 현재-main all-green으로 만들지 않는다.

### 11.2 이전에 실행한 좁은 체크

아래는 이 세션의 이전 실행 명령이며 이번 문서 turn에서는 다시 실행하지 않았다.

```sh
.venv/bin/python -m pytest -q \
  tools/ci/tests/test_retrieval_benchmark.py::test_pair_staging_atomicity \
  tools/ci/tests/test_retrieval_benchmark.py::test_large_paired_bootstrap_reuses_bounded_numeric_cache
```

해당2개 focused check는19.89초에 통과했다. bootstrapping cache와 pair atomicity 범위이며 제품 speed/E2E가 아니다.

기존 qg15 독립 replay에 사용한 CLI 형태(출력은 반드시 새 외부 경로):

```sh
.venv/bin/python tools/benchmark/retrieval/run.py verdict \
  --repo /Users/songmin/Documents/code-new/qi-large-scale-rerun-20260927/full-checkouts/gin \
  --suite /private/tmp/qg15/evaluator-only/suite.json \
  --run-manifest /private/tmp/qg15/run-manifest.json \
  --out /private/tmp/<fresh-output>.json
```

`<fresh-output>`은 설명용 placeholder다. `--repo gin`은 잘못된 invocation이며 거절됐다. old/staging 실패 결과를 scored output으로 사용하지 않는다. 다음 실행자는 current CLI `--help`와 source contract를 먼저 확인한다. Rust tests/build는 `Justfile`/`./scripts/cargow`의 기존 owner selector를 사용한다.

이번 문서 저장 요청에서 실행한 범위: `git rev-parse/status/log/branch`, `rg`, 관련 ticket/source/JSON/handoff 읽기, 이 문서 작성과 파일/링크/diff 확인. 제품 검색·모델 호출·Rust build·새 unit test·commit/push는 실행하지 않는다.

## 12. 증거·티켓 위치와 다음 작업자의 첫 체크

| 위치 | 용도 |
| --- | --- |
| [B07](/Users/songmin/Documents/code-new/quanta-index/docs/plans/sep-30-code-search-benchmark-trust/tickets/S30-B07-performance-and-indexing.md) | source별 계측·실행·실패·최신 성능/scale 게이트의 기존 owner |
| [B08](/Users/songmin/Documents/code-new/quanta-index/docs/plans/sep-30-code-search-benchmark-trust/tickets/S30-B08-fresh-multirepo-holdout.md) | corpus/holdout/C0–C5와 독립성/검수 계약 |
| [B09](/Users/songmin/Documents/code-new/quanta-index/docs/plans/sep-30-code-search-benchmark-trust/tickets/S30-B09-external-robustness-adoption.md) | OSA/NL/외부 데이터 도입·structural fix와 실제 분모 |
| [qg8 MEASUREMENTS](/private/tmp/qg8-zzx504gf/MEASUREMENTS.md) | release5lane의 query/ingest 비용; schema8 historical |
| [qg15 binding](/private/tmp/qi-gin1196-pair-v3-20261004-d274-lexical-only/binding.json) | v3 mechanical exact1196 입력/optimized-debug source |
| [qm3 manifest](/private/tmp/qm3/matrix-manifest.json) | 예전8repo native union/replay parity |
| [g9 diagnostic](/private/tmp/g9-7m7inh5r/capture/strategy-00-fw_strict/retrieval-diagnostic.json) | release schema9 ingest child clocks |
| [g9 phases](/private/tmp/g9-7m7inh5r/capture/strategy-00-fw_strict/phase-metrics.json) | publish/activate/query ledger |
| [fullsync ACTION-LIST](/private/tmp/qi-fullsync-rca-8v3e3mn_/ACTION-LIST.md) | 세 병렬 RCA와 원자적 durable batching 설계/제약 |
| [i9 request events](/private/tmp/i9-lhyxoxdp/request-events.json) | resolve/text 실제 RPC 귀속, prevalidation 공백 |
| [ASCII current audit](/private/tmp/a9-21c9z44a/audit-current/REPORT.json) |20captures의 independently audited parity·timing |
| [B09 final](/Users/songmin/Documents/code-new/qi-b09-final-20261004-SQoHAA/RESULTS.md) | 과거global12 five-product diagnostic |
| [B09 structural](/Users/songmin/Documents/code-new/qi-b09-structural-fix-20261004-ji1PLR/RESULTS.md) | 후속Quanta source/profile별 새 diagnostic |

`/private/tmp` 산출물은 재부팅/정리로 없어질 수 있다. 이 문서에 핵심 표본값과 source/binary identity를 남겼지만 raw를 대체하지 않는다. 실제 작업 재개 때 존재·digest를 재검증하고 필요한 raw는 새 외부 보존 루트에 복사해 binding을 기록한다. 원본은 덮어쓰지 않는다. repo에 one-off 실행 dump를 추가하지 않는다.

다음 작업자는 **현재 main/dirty 및 담당자 → 남은 authority별 unit → source/binary freeze → 작은 실제 parity → 해당 lane의 필요한 캡처** 순서로 진행한다. 최종 보고서는 완료 구현, functional proof, diagnostic 품질, qualified 성능, publication을 각각 분리한다.
