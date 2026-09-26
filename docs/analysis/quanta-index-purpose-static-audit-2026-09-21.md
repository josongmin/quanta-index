# quanta-index 목적 적합성 정적 감사

> Archive classification: historical static audit at the source recorded below. See [SEP-27-001](../adr/SEP-27-001-documentation-authority-and-historical-record-custody.md) and [the documentation archive](../ARCHIVE-INDEX.md). Revalidate every finding against current source.


- 감사일: 2026-09-21
- 감사 방식: 정적 소스/계약/레시피/문서/기존 아티팩트 감사만 수행
- 기준 체크리스트: `docs/analysis/quanta-index-purpose-validation-checklist.md`
- quanta-index HEAD: `3ad279a08879de35fa96a5495a3382af28f095d0`
- 브랜치: `main`
- upstream: `4914156f4191daa3e12998bdb38f2b821a057fdd`
- upstream 대비: ahead 145, behind 0
- 작업 트리: tracked 8개 수정 + 본 감사 문서와 체크리스트 untracked
- 주의: 아래 판정은 현재 dirty working tree를 포함한다. clean HEAD 판정이 아니다.

## 1. 결론

**정적 종합 판정: PURPOSE_RED**

현재 구조는 generation 기반 lexical/semantic 저장, composite activation, snapshot pinning,
fail-closed 계약, UDS 분리 등 핵심 방향은 목적에 부합한다. 그러나 다음 결함 때문에
“프로덕션에서 목적대로 안전하게 동작한다”고 승인할 수 없다.

1. `BatchPublishReceipt`의 wire 및 catalog 저장 형식이 무버전 strict schema인데 필수 필드가
   추가됐다. 구버전 receipt, 구버전 daemon/SDK 조합, 기존 catalog replay가 깨질 수 있다.
2. README는 SIGINT/SIGTERM drain을 주장하지만 daemon 진입점에서 shutdown flag를 설정하는
   signal handler가 보이지 않는다.
3. 요청 취소 뒤 이미 시작된 Lance CPU 작업과 OpenAI provider thread가 계속 실행될 수 있으며,
   process-global residual-work 제한이나 종료 대기가 없다.
4. `quality-all`이라는 이름과 달리 hybrid judged relevance, ANN, concurrency, real provider,
   external comparison을 필수 집계하지 않는다. 기존 benchmark artifacts도 현행 authority가
   요구하는 형식과 맞지 않아 품질/성능 폐쇄 증거로 사용할 수 없다.
5. readiness/doctor/quarantine/운영 runbook이 장애 진단과 복구에 필요한 상태를 충분히 노출하지 않는다.
6. 외부 producer 저장소가 감사 중 이동했으며 clean-head 조합이 고정되지 않았다. 현재 cross-repo
   compatibility는 증명 불가다.

이 감사는 테스트 결과를 판정 근거로 사용하지 않는다. 실행하지 않은 성질은 `RUNTIME_UNVERIFIED`로
남기며 정적 구조만으로 PASS로 승격하지 않는다.

## 2. 판정 규칙

| 판정 | 의미 |
|---|---|
| `PASS_STATIC` | 현재 소스에서 책임자, 계약, 실패 경로가 정적으로 확인됨 |
| `FAIL_STATIC` | 현재 소스만으로 모순, 누락, 호환성 파손 또는 잘못된 주장이 확인됨 |
| `BLOCKED` | 외부 상태 또는 고정되지 않은 조합 때문에 현재 판정 불가 |
| `RUNTIME_UNVERIFIED` | 구조는 있으나 동시성, crash, 성능, 실제 provider 등의 실행 증거가 필요함 |
| `N/A_DECISION_REQUIRED` | 제품 범위상 제외 가능하지만 명시적 범위 결정이 없음 |

## 3. 체크리스트 그룹별 판정

| 그룹 | 정적 판정 | 핵심 근거 |
|---|---|---|
| G0 기준선/증거 | `FAIL_STATIC` | dirty source, moving producer HEAD, 무효 benchmark artifacts |
| G1 목적/경계 | `PASS_STATIC` | daemon/SDK/CLI, lexical/semantic, generation authority의 큰 책임 분리는 명확 |
| G2 producer 계약 | `FAIL_STATIC` | receipt schema의 무버전 breaking change와 clean-head 조합 미고정 |
| G3 ingest/idempotency | `FAIL_STATIC` | request-side 기본값은 있으나 persisted receipt replay upgrade 경로 부재 |
| G4 generation/activation | `PASS_STATIC` + `RUNTIME_UNVERIFIED` | CAS activation, rollback, pair lock, snapshot/GC fencing 구조 존재; fault proof는 실행 필요 |
| G5 query 결과 계약 | `FAIL_STATIC` | symbol identity/language 명세 부족, 일부 surface pagination 정책 불명확 |
| G6 품질 | `FAIL_STATIC` | quality aggregate 누락, semantic threshold 약함, hybrid judged rail 부재 |
| G7 SDK/CLI UX | `FAIL_STATIC` | query-only 연결도 모든 socket 요구, readiness는 connect-only, CLI help/version 문제 |
| G8 daemon lifecycle | `FAIL_STATIC` | signal handler 부재, README 주장과 구현 불일치 |
| G9 cancellation/resource | `FAIL_STATIC` | provider/CPU residual work의 process-global bound와 drain 부재 |
| G10 보안/격리 | `PASS_STATIC` + `RUNTIME_UNVERIFIED` | UDS peer credential 및 state-root 권한 구조는 존재; 실제 배포 권한 검증 필요 |
| G11 성능/확장 | `FAIL_STATIC` | 현행 schema/HEAD 요건을 만족하는 authoritative artifacts 부재 |
| G12 운영/배포 | `FAIL_STATIC` | doctor/readiness/provenance/runbook/upgrade rollback 부족 |
| G13 유지보수/회귀 | `FAIL_STATIC` | recipe가 중요 E2E/quality rail을 빠뜨려 green 의미가 불완전 |

## 4. 우선순위별 결함

### P0-1. Receipt wire 및 persisted replay 호환성 파손

판정: `FAIL_STATIC`

근거:

- `BatchPublishReceipt`에 `accepted_semantic_replace_scopes`와
  `accepted_semantic_tombstone_scopes`가 필수 필드로 존재한다.
  - `crates/quanta-index-contract/src/ipc/ingest.rs:4186-4215`
- serializer는 12개 필드를 모두 송신한다.
  - `crates/quanta-index-contract/src/ipc/ingest.rs:4217-4256`
- decoder는 새 semantic 필드를 누락 시 거부하고 unknown field도 거부한다.
  - `crates/quanta-index-contract/src/ipc/ingest.rs:4363-4392`
- 소스 내 테스트 의도도 구 receipt가 decode되지 않는 fail-closed 정책임을 명시한다.
  - `crates/quanta-index-contract/src/ipc/ingest.rs:5523-5575`
- catalog는 receipt를 raw CBOR blob으로 저장한다.
  - `crates/quanta-index-catalog/src/idempotency.rs:40-52`
- replay는 저장된 blob을 현재 타입으로 직접 decode하고 실패 시 `CATALOG_ROW_CORRUPT`로 바꾼다.
  - `crates/quanta-index-catalog/src/idempotency.rs:245-259`
- finalize는 schema/version envelope 없이 현재 receipt를 encode해 저장한다.
  - `crates/quanta-index-catalog/src/idempotency.rs:276-348`

영향:

- 구 daemon -> 신 SDK: 새 필드 누락으로 decode 실패 가능.
- 신 daemon -> 구 SDK: unknown field 거부로 decode 실패 가능.
- 기존 state root의 구 receipt -> 신 daemon replay: `CATALOG_ROW_CORRUPT` 가능.
- 동일 batch 재시도라는 핵심 idempotency 경로가 배포 업그레이드 뒤 장애 경로가 될 수 있다.

필요 조치:

1. 다음 중 하나를 명시적으로 선택한다.
   - lock-step 배포 + 기존 state root 폐기/재구축
   - versioned receipt envelope + decoder/importer
   - 필드별 bounded legacy default + 명시적 지원 기간
2. 선택한 정책을 wire compatibility 표와 state-root upgrade/cutover runbook에 기록한다.
3. request, response, persisted row를 별개 compatibility surface로 관리한다.

### P0-2. SIGINT/SIGTERM graceful shutdown 주장이 구현과 불일치

판정: `FAIL_STATIC`

근거:

- daemon `run()`은 `AtomicBool(false)`를 만들어 `drive`에 전달하지만, 이 값을 signal에서
  변경하는 handler 등록이 확인되지 않는다.
  - `crates/quanta-index-searchd-runtime/src/lib.rs:269-284`
- `drive`의 drain 진입은 shutdown flag가 true가 되는 것에 의존한다.
  - `crates/quanta-index-searchd/src/app/searchd.rs:20-58`
- README는 SIGINT/SIGTERM drain을 구현된 사실로 주장한다.
  - `README.md:35-39`
  - `README.md:135-138`

영향:

- 정상 종료, 배포 교체, orchestrator termination에서 drain이 실행되지 않을 수 있다.
- 문서 기반 운영 판단이 실제 lifecycle과 다르다.

필요 조치:

- process entry에서 SIGINT/SIGTERM handler가 단일 shutdown state를 설정하도록 한다.
- accept 중단, 신규 admission 차단, in-flight wait, timeout 이후 강제 종료의 순서를 계약화한다.
- 구현 전에는 README의 drain 보장을 제거하거나 `planned`로 표시한다.

### P0-3. 취소 이후 residual work가 process 수준에서 제한되지 않음

판정: `FAIL_STATIC`

근거:

- Lance budget 코드는 이미 dispatch된 CPU 작업이 cancellation 뒤에도 계속될 수 있음을 명시한다.
  - `crates/quanta-index-semantic/src/budget.rs:23-33`
- OpenAI attempt는 OS thread를 spawn하며 caller timeout/cancellation 반환 뒤에도 provider thread가
  자체 timeout까지 남을 수 있다.
  - `crates/quanta-index-embed/src/openai.rs:329-447`
- 이 잔여 작업을 process-global로 계수, 제한, drain 또는 관측하는 authority를 확인하지 못했다.

영향:

- timeout storm에서 실제 CPU/thread/provider request 수가 호출자 관점의 in-flight 수보다 커진다.
- graceful shutdown과 backpressure가 명목상 완료돼도 백그라운드 작업이 지속될 수 있다.

필요 조치:

- residual worker를 포함하는 process-global semaphore/budget을 둔다.
- cancellation 반환과 실제 worker 종료를 분리 측정한다.
- shutdown은 residual worker까지 bounded wait하고 timeout 시 이유와 개수를 기록한다.

### P1-1. `quality-all`이 전체 품질 authority를 대표하지 못함

판정: `FAIL_STATIC`

근거:

- relevance route는 lexical/semantic만 모델링하고 hybrid judged relevance가 없다.
  - `crates/quanta-index-searchd-harness/src/relevance/corpus.rs:25-42`
- semantic MRR/NDCG threshold가 0.0이고 recall만 1.0이다.
  - `crates/quanta-index-searchd-harness/src/relevance/report.rs:72-88`
- Sourcegraph 비교는 provisioning되지 않은 상태를 정상 보고하도록 작성돼 있다.
  - `crates/quanta-index-searchd-harness/src/relevance/report.rs:835-882`
- `quality-all`은 relevance, ambiguity, snippet, scale, tail, ops, ui만 실행한다.
  - `Justfile:475-489`
- 통합 summary도 같은 7개 rail만 집계한다.
  - `tools/benchmark/quality_integration_summary.py:24-34`
- ANN, concurrency, real-provider, external comparison은 전체 품질 집계의 필수 상태가 아니다.

영향:

- recipe가 green이어도 hybrid 품질, ANN 성질, 동시성, 실제 provider 품질은 미검증일 수 있다.
- `quality-all` 명칭이 증거 범위를 과장한다.

필요 조치:

- aggregate에 모든 필수 차원을 `PASS/FAIL/BLOCKED/NOT_RUN/N_A`로 포함한다.
- hybrid judged set과 non-zero semantic ranking floor를 정의한다.
- external comparison이 필수가 아니면 명시적으로 N/A 판정 근거를 남긴다.

### P1-2. 중요 E2E source가 canonical E2E recipe에서 누락

판정: `FAIL_STATIC`

`Justfile:250-275`의 `rust-test-e2e`가 다음 중요 파일을 포함하지 않는다.

- `e2e_explain_score_trace`
- `e2e_history_relevance`
- `e2e_aux_epoch`
- `e2e_keyset_cursors`
- `e2e_read_view`
- `e2e_hybrid_filters`

영향:

- canonical recipe green이 explain, history relevance, auxiliary epoch, cursor, read-view,
  hybrid filter semantics를 포함한다고 해석할 수 없다.

필요 조치:

- test inventory와 recipe selector의 양방향 완전성 guard를 둔다.
- 제외 항목은 owner, 사유, 대체 authority, 만료일을 요구한다.

### P1-3. 외부 producer 호환성 증거가 좁고 source snapshot이 불안정

판정: `BLOCKED`

근거:

- cross-repo recipe는 `Justfile:239-241`에서 한정된 contributor test만 실행한다.
- full/delta, semantic replace/tombstone, activation retry, text-symbol-semantic-hybrid 조합을
  clean-head matrix로 고정하지 않는다.
- 감사 중 sibling producer 저장소 HEAD가 이동했고 dirty 상태였다. 따라서 한 시점의 exact pair를
  authoritative compatibility 조합으로 선언할 수 없다.

필요 조치:

- producer HEAD, consumer HEAD, lockfile digest, dirty fingerprint를 한 receipt에 고정한다.
- 지원 조합과 배포 순서를 compatibility matrix로 관리한다.
- cross-repo rail을 실제 production ingest/activation use case별로 확장한다.

### P1-4. Readiness와 doctor가 운영 준비 상태를 충분히 설명하지 못함

판정: `FAIL_STATIC`

근거:

- `TrackReadinessRecord`는 track, generation, digest만 제공한다.
  - `crates/quanta-index-contract/src/ipc/control.rs:1142-1150`
- aggregate report도 tracks와 semantic content roots 중심이며 proof state와 not-ready reason을
  직접 제공하지 않는다.
  - `crates/quanta-index-contract/src/ipc/control.rs:1232-1249`
- `searchctl doctor`는 catalog listing과 serve-time resolver 일치 여부에 집중한다.
  - `crates/quanta-index-searchctl/src/lib.rs:1721-1831`
- socket 전부, state-root 쓰기/공간, catalog pragma, provider readiness, quarantine backlog,
  residual work를 하나의 진단 모델로 통합하지 않는다.

필요 조치:

- readiness에 `ready`, typed reason, proof/verification state, checked-at를 포함한다.
- doctor는 query/control/ingest socket, state root, catalog durability, active generation,
  provider, quarantine, resource saturation을 분리 진단한다.

### P1-5. Quarantine provenance와 복구 정보 부족

판정: `FAIL_STATIC`

근거:

- quarantined generation entry는 track/path/reason/detail만 가진다.
  - `crates/quanta-index-contract/src/ipc/quarantine.rs:75-99`
  - `crates/quanta-index-contract/src/ipc/quarantine.rs:218-228`
- 명시적 repo/revision/generation, 최초 관측 시각, source manifest digest, suggested remediation이 없다.

영향:

- 운영자가 동일한 path 표현에 의존해 원인과 대상 generation을 재구성해야 한다.
- 보존/삭제 정책과 incident timeline을 자동화하기 어렵다.

### P1-6. 배포·업그레이드·복구 owner 문서 부재

판정: `FAIL_STATIC`

확인하지 못한 항목:

- service owner/support matrix
- start/stop/restart와 signal timeout runbook
- 배포 manifest 또는 외부 platform 문서의 canonical link
- state-root backup/restore 절차와 RPO/RTO
- wire/persisted format upgrade 및 rollback runbook
- quarantine retention/repair 절차

repo 밖 platform가 책임자일 수 있다. 그 경우에도 이 repo의 운영 문서에서 authority 위치와
버전 경계를 링크해야 한다.

### P2-1. Symbol result identity 계약이 모호함

판정: `FAIL_STATIC`

- `SymbolCandidate`는 candidate ID, path, line range, kind를 제공하지만 명시적 symbol name,
  qualified name, language가 없다.
  - `crates/quanta-index-contract/src/results/query_responses.rs:45-72`
- `candidate_id`가 canonical symbol identity인지, display/rename 안정성이 어떤지 계약이 부족하다.

필요 조치:

- stable symbol ID의 생성/수명 계약을 문서화하거나 qualified identity/language를 타입에 포함한다.

### P2-2. SDK query-only 연결이 불필요한 socket까지 요구

판정: `FAIL_STATIC`

- `ConnectOptions::resolve`는 query/control/ingest path를 모두 요구한다.
  - `crates/quanta-index-sdk/src/config.rs:81-123`

영향:

- read-only client도 ingest/control 배치와 구성에 결합된다.
- 최소 권한 배포와 부분 장애 격리가 약해진다.

필요 조치:

- query-only, admin, producer client profile을 분리하고 각 profile이 필요한 endpoint만 resolve한다.

### P2-3. CLI 기본 운영 UX 결함

판정: `FAIL_STATIC`

- `--help`가 정상 help success가 아니라 usage error/exit 2 경로다.
- `--version` surface가 확인되지 않는다.
- usage text 한 줄이 여러 subcommand 설명으로 잘못 합쳐져 있다.
  - `crates/quanta-index-searchctl/src/lib.rs:2644-2668`
- `quarantine discard`가 실제 mutation인데 `Read-only subcommands` 아래에 있다.
  - `crates/quanta-index-searchctl/src/lib.rs:2653-2668`

### P2-4. SQLite durability read-back 설명과 구현 불일치

판정: `FAIL_STATIC`

- 주석은 G0-C pragmas를 각각 read back한다고 주장한다.
- `fullfsync=ON`을 설정하지만 실제 검증은 `journal_mode`와 `synchronous`만 한다.
  - `crates/quanta-index-catalog/src/connection.rs:89-135`

필요 조치:

- fullfsync가 필수 durability invariant면 read-back/unsupported 처리를 추가한다.
- advisory면 주석과 gate 요구를 낮춰 실제 보장과 맞춘다.

### P2-5. 문서가 stale snapshot과 삭제된 구조를 권위로 노출

판정: `FAIL_STATIC`

- README의 code-truth snapshot은 현재 HEAD가 아닌 `526349b`다.
  - `README.md:41`
- README는 구현되지 않은 것으로 보이는 signal drain을 사실로 적는다.
  - `README.md:35-39`, `README.md:135-138`
- canonical SSOT 링크가 삭제된 control crate, 과거 SQLite/socket 구조를 포함한 오래된 문서로 이어진다.
  - `README.md:140-143`
  - `docs/ssot/may-23-storage-architecture-endgame-implementation.md:218-279`
  - `docs/ssot/may-23-storage-architecture-endgame-implementation.md:382-396`

### P2-6. Production request observability가 구조적으로 부족

판정: `FAIL_STATIC`

- daemon runtime에서 명확히 확인되는 operator log는 boot 시점 `eprintln!`이다.
  - `crates/quanta-index-searchd-runtime/src/lib.rs:260-266`
- request ID, route, repo/revision/generation, latency, typed outcome, cancellation, admission refusal을
  일관되게 기록하는 production request logging authority를 확인하지 못했다.

## 5. 정적으로 양호한 구조

다음은 현재 소스에서 목적과 책임 방향이 대체로 일치한다.

- lexical/semantic generation을 분리하면서 composite active authority로 serve head를 고정한다.
- activation CAS, rollback, pair-scoped locking, open/prove/promotion 책임이 구분돼 있다.
- snapshot pinning과 GC fencing 구조가 있다.
- lexical/semantic sealed manifest와 catalog row digest가 있다.
- typed fail-closed 오류 계약을 광범위하게 사용한다.
- query/control/ingest UDS와 peer credential 기반 접근 제어 구조가 있다.
- request-side semantic/clear surface 필드 일부는 missing-to-empty compatibility를 의도적으로 제공한다.

단, crash consistency, race freedom, GC 안전성, 실제 권한, latency, recall은 정적 구조만으로
검증할 수 없으므로 이 감사에서 runtime PASS로 취급하지 않는다.

## 6. 체크리스트 자체의 보완점

### 6.1 감사 프로파일을 분리해야 함

현재 체크리스트는 모든 배포에 모든 항목이 동일하게 필수인 것처럼 읽힐 수 있다. 최소 다음
프로파일을 추가한다.

- `CORE_CORRECTNESS`: wire, ingest, idempotency, generation, query correctness
- `PRODUCTION_DAEMON`: lifecycle, cancellation, security, observability, operations
- `COMPETITIVE_QUALITY`: judged relevance, ANN, scale, external comparison
- `EXTERNAL_PROVIDER`: real embedding provider, secret/rate-limit/failure behavior

각 항목에는 `applies_to`와 N/A 승인자를 둔다.

### 6.2 persisted schema compatibility를 별도 gate로 승격해야 함

wire request/response만 보면 catalog blob replay 파손을 놓친다. 다음을 별도 항목으로 추가한다.

- 이전 binary가 쓴 state root를 현재 binary가 열고 replay할 수 있는가
- schema/version envelope가 있는가
- migration이 restart-safe/idempotent한가
- downgrade/rollback이 가능한가
- clean-state rebuild만 지원한다면 데이터 폐기 조건과 배포 순서가 명시됐는가

### 6.3 recipe completeness guard가 필요함

테스트 파일 존재와 canonical recipe 포함은 다른 사실이다. 체크리스트에 다음을 추가한다.

- inventory의 모든 mandatory rail이 적어도 하나의 canonical recipe에 포함되는가
- recipe 이름이 실제 포함 범위를 과장하지 않는가
- 제외는 owner/reason/alternative authority/expiry를 가지는가

### 6.4 aggregate는 미실행을 숨기면 안 됨

품질/성능 summary는 존재하는 artifact만 모아 PASS하면 안 된다. mandatory dimension 전체를 먼저
선언하고 각 차원을 `PASS/FAIL/BLOCKED/NOT_RUN/N_A`로 기록해야 한다.

### 6.5 cross-repo snapshot 안정성 gate가 필요함

외부 producer/consumer 감사에는 양쪽 HEAD, dirty fingerprint, lockfile digest, dependency resolution,
receipt timestamp를 요구한다. 감사 중 source가 이동하면 결과는 자동 `BLOCKED` 처리한다.

### 6.6 pagination은 surface별 boundedness 결정 뒤 적용해야 함

RepoMap과 cluster membership이 의도적으로 bounded single-response라면 cursor 부재를 결함으로
간주하면 안 된다. 각 surface에 다음 중 하나를 먼저 선언한다.

- bounded response + hard maximum + typed overflow/refusal
- stable cursor pagination
- streaming/chunked response

### 6.7 repo-local 배포 파일만 강제하지 말아야 함

배포 authority가 외부 platform repository에 있을 수 있다. 체크리스트는 “이 repo에 manifest 존재”가
아니라 다음을 요구해야 한다.

- canonical owner와 위치
- 지원 binary/config/state-root version 조합
- 변경 검토 경계
- rollback/restore runbook 접근 경로

### 6.8 `FAIL`과 `정책 결정 부재`를 구분해야 함

compaction/remap, lifecycle CLI, external quality floor처럼 제품 범위 선택이 먼저인 항목은
`N/A_DECISION_REQUIRED`를 허용하되 owner, 근거, 재검토 조건을 필수화한다.

## 7. 수정 순서

1. **배포 차단**: receipt wire/persisted schema 정책을 결정하고 upgrade/cutover 경로를 만든다.
2. **lifecycle 차단**: 실제 signal handler와 bounded drain을 구현하고 README를 일치시킨다.
3. **resource 차단**: provider/CPU residual work를 process-global budget과 shutdown accounting에 넣는다.
4. **증거 authority 수정**: `quality-all`, E2E recipe, artifact schema, mandatory dimension 집계를 정합화한다.
5. **운영성 보완**: readiness/doctor/quarantine provenance와 runbook authority를 확장한다.
6. **계약/UX 보완**: symbol identity, query-only SDK profile, CLI help/version/usage를 수정한다.
7. **문서 정리**: stale snapshot/SSOT를 현재 architecture와 보장 수준에 맞춘다.

P0 세 항목이 해결되기 전에는 release/production-ready 판정을 내리지 않는 것이 타당하다.

## 8. 감사 한계

- 사용자 지시에 따라 실행형 테스트, fault injection, benchmark, daemon 기동은 본 판정에서 제외했다.
- 따라서 race, crash recovery, latency, recall, live provider, OS별 UDS/SQLite 동작은 검증하지 않았다.
- 현재 source는 dirty하며 clean committed revision이 아니다.
- sibling producer source가 이동 중이어서 exact cross-repo compatibility 판정은 고정하지 못했다.
- 본 보고서는 수정 패치가 아니라 현재 구조의 정적 감사 결과다.
