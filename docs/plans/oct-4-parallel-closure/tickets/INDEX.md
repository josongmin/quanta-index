# 잔여 작업 인덱스

Status: `ACTIVE_RESIDUAL`

완료된 결정은 [ADR](../../../adr/README.md), 정확한 과거 문서·실행은
[복구 인덱스](../../../ARCHIVE-INDEX.md#historical-record-recovery)가 소유한다. 이 인덱스에는 **미완료 조건만** 남긴다.
완료·비적용 scope와 중복 RFC/플랜/티켓은 제거했다. B01–B09/J7Q/QIT/SEP-21/MISC의
미완료 작업은 이 목록에만 유지하고, 영구 수용 계약은 기존 ADR가 소유한다.
파일 경로는 동시 작업의 참조를 보존하기 위해 유지한다.

## Owners

| Owner | 소유 경계 |
| --- | --- |
| E1 | review/admission/evaluator/source oracle/split/license/gold/scoring |
| E2 | native collector/index scope/Semble phases/required cells |
| E4 | lexical lifetime/cost/scanner/scale/open-loop/측정 후 정책 판정 |
| I0 | shared DTO/schema/registry/CI/dependency/영향 source/설치·pair·release |

Cargo/Justfile/CI/shared schema는 한 integration owner가 반영한다.
E1→E2는 immutable admitted inputs, E2→E1는 raw/index/clock/outcomes/unjudged keys,
E1→E4/I0는 final qrels/report/denominators/정책 판정이다.
Semantica는 외부 producer이며 fact resolution/join/completion은 그 저장소가 소유한다.

## 현재 코드 잔여

2026-10-08 코드·실행 대조는 main의 packed-source planner·publication typed proof·공식 ARB/BCY scorer·paired R5 owner recipe 수리를 포함한다. 기존 `b9c058e1`의 완료된 F15/query/restart·paged directory,
SDK lifecycle/cache 및 hosted checkpoint의 source·명령·범위는 아래 ADR가 소유한다.
Staged upload, EOF cancellation, checked memory/deadline, scanner custody와 P11 공통
producer/parser/checker/recipes도 구현돼 있다. 추가 수리는 실제 비용·반례 또는 target 계약으로 결정한다.

### Native corpus decode의 외부 DATA 계약 — 2026-10-09

**Index producer 구현 완료, Semantica recursive receiver co-cut/Original Source 수용은
`NOT_RUN`.** `4729a1a1`에 기록했던 제안 ABI를 아래 실제 구현으로 대체했다. 변경은 아직
`codex/native-corpus-external-data` 검토 브랜치로 보존하며, 공유 작업트리에도 변경이
남아 있다. 기존 native owned 반환 trait/seed와 하위 호환하지 않는다. receiver가
이 계약으로 전환되기 전에는 이 breaking 묶음을 단독 main commit으로 반영하지 않는다.

물리 owner와 canonical 경계:

- `contract-base/src/ids/native_decode_data_v1.rs`의
  `NativeIdentityDecodeDataV1<T, E, F>`는 실제 wire/copy String, canonical typed output,
  기존 `NativeNormalizationDataV1<E>`, full `NativeIdentityConstructionErrorV1<E>`,
  one-attempt phase와 `Option<F>`를 보유한다. `F`는 source/control/callback이 없는 실제
  funding bank다. declaration/drop 순서는 실제 backing/전체 오류가 bank보다 앞선다.
- canonical identity visitor만 transient `NativeIdentityDecodeLoanV1<'data, 'input, T, E, F>`를
  만든다. owned String을 DATA에 먼저 넣고, borrowed input은 loan/runner에서만 빌린다.
  `with_funding_v1`의 runner에는 입력 교체, typed output setter 또는 NFC verdict 주입
  API가 없다. `try_fill_v1`은 기존 canonical raw validator/NFC/into-slot producer를 호출한다.
  `refuse_admission_v1(&mut Option<E>)`는 pre-construction 원본 오류를 순수 이동으로 보관한다.
- `control/native_decode_v1.rs`의 하나의 map visitor/Node body가 ordinary와 native를
  처리한다. native recursive seed의 `Value`는 unit이다. 각 schema subtree의 seen 상태,
  owned key/duplicate pending key, String/identity/token 부분 상태와 완성 candidate를
  외부 DATA에 채운다. aggregate work/validation 전에 candidate를 DATA에 먼저 저장한다.
- `activation_token.rs`의 같은 canonical token visitor도 unit fill을 사용한다. owned key,
  잘못된 owned scalar/array-element 문자열, 원본 token work 오류를 외부 DATA에 보관한다.
  numeric payload는 기존 Serde scalar/array visitor를 사용하며 새 token issuer를 만들지 않는다.
- `ids/native_manifest_decode_v1.rs`는 기존 ManifestGeneration numeric visitor의 unit
  결과를 보관한다. native scalar dispatch는 self-describing `deserialize_any`를 요구하여
  잘못된 owned 문자열도 기존 numeric visitor가 거부하기 전에 DATA에 넣는다. ordinary
  token의 typed deserializer dispatch와 기존 JSON/CBOR wire shape는 유지한다.

실제 exported ABI (`quanta-native-identity-v1` feature):

```rust
trait NativeIdentityDecodeAdmissionV1 {
    type OriginalError;
    type Funding;
    type Error: core::fmt::Display + Copy; // finite Serde-facing marker
    fn repo_id_from_owned_v1(
        &mut self,
        loan: &mut NativeIdentityDecodeLoanV1<'_, '_, RepoId, Self::OriginalError, Self::Funding>,
    ) -> Result<(), Self::Error>;
    fn repo_id_from_borrowed_v1(
        &mut self,
        loan: &mut NativeIdentityDecodeLoanV1<'_, '_, RepoId, Self::OriginalError, Self::Funding>,
    ) -> Result<(), Self::Error>;
    // RevisionId has the corresponding two unit callbacks.
}

// RepoId and RevisionId expose the same signatures with their own T.
RepoId::native_decode_seed_v1(&mut admission, &mut identity_data) // Value = ()
RepoId::try_decode_into_v1(deserializer, &mut admission, &mut identity_data, &mut full_decoder_error)

// Token policy uses ControlError = OriginalError; consume_token_work_v1
// returns the complete ControlError, token_work_refusal_v1 returns a finite marker.
SearchCorpusActivationTokenV1::native_decode_seed_v1(&mut admission, &mut token_data) // Value = ()
SearchCorpusActivationTokenV1::try_decode_into_v1(deserializer, &mut admission, &mut token_data, &mut full_decoder_error)

// NativeCorpusDecodeAdmissionV1 extends both native traits above; its
// OriginalError/Funding are the identity policy's associated types.
// consume_corpus_work_v1(&mut self, u64) -> Result<(), Self::OriginalError>
// refuse_corpus_work_arithmetic_v1(&mut self) -> Self::OriginalError
// admit_corpus_string_birth_v1(&mut self, usize, &mut dyn FnMut()->bool)
//     -> Result<bool, Self::OriginalError>
// corpus_invalid_data_v1(&self, finite_schema_cause, Option<&'static str>) -> finite_marker
let mut corpus_data = NativeCorpusDecodeDataV1::<E, F>::new_v1();
SearchCorpusActiveHeadV1::native_decode_seed_v1(&mut admission, &mut corpus_data) // Value = ()
SearchCorpusActiveHeadV1::try_decode_into_v1(
    deserializer, &mut admission, &mut corpus_data, &mut full_decoder_error,
) // Result<(), NativeIdentityDecodeDataRefusalV1>

// Pure transfer into another EXTERNAL slot, or publication after finishing.
corpus_data.complete_into_slot_v1(&mut external_head)
// failure_v1() returns a borrowed view of full Work(E), Copy(error<E>),
// or Identity(construction_error<E>); it does not format/clone the original.
```

각 occurrence에 별도 DATA가 필요하다. 한 head의 네 identity는 schema의 네 필드이며
전체 replay/collection을 네 identity로 제한하는 전역 슬롯이 아니다. Runtime의 기존
admitted collection owner가 임의 개수의 head DATA와 container backing을 보유해야 한다.
DATA에는 입력 참조/Current/Source/control/admission closure를 저장하지 않는다. policy의
`Funding`은 실제 normalization bank이며, 예를 들어 Semantica SourceWire의
`OriginalNativeNormalizationFundingV3`가 해당한다. transient admission은 이 bank를 빌린다.
borrowed String copy와 deserializer backing의 funding은 실제 Runtime 외부 owner가 별도로
유지해야 한다. byte counter만으로 custody를 충족하지 않는다.

가장 바깥 `try_decode_into_v1`는 반환된 실제 `D::Error`를 별도 **외부** error 슬롯에
그대로 저장한 뒤 finite unit status를 반환한다. 중첩 unit seed를 직접 쓰는 Runtime은
자신의 상위 driver에서 같은 오류 보관을 수행해야 한다. occupied error/output 슬롯은
보존하며, 사용된 DATA의 재시도는 deserializer/admission poll 전에 거부한다. candidate가
있더라도 전체 decode가 실패한 DATA에서는 publication transfer를 거부한다. parent로
순수 이동한 typed payload가 살아 있는 동안 funding DATA도 terminal까지 유지한다.

범위 제한: generic Serde implementation 내부에서 방문자 호출 전에 만들어지는 owned
container/escaped scratch 및 아직 읽지 않은 input backing은 caller-controlled deserializer가
별도로 admit/retain해야 한다. 이 Index visitor만으로 그 allocation을 통제했다고 주장하지
않는다. native scalar는 self-describing format을 요구한다. ordinary DTO decode는 같은
canonical visitor를 사용하지만 Native Source qualification을 제공하지 않는다.

실제 수신 co-cut 대상 (Semantica, 다른 owner의 working tree):

| Owner | 경로와 역할 |
| --- | --- |
| Contract | `quanta-contract-retrieval/src/indexing_writer_ports/admitted_decode_v3.rs`: replay recursive partial DATA/seed; 기존 native head owned 반환 호출과 dyn policy seam 교체 |
| Contract 테스트 | `admitted_decode_v3/native_corpus_policy_tests_v3.rs`, `policy_tests_v3.rs`: associated OriginalError/Funding, mutable transient policy, unit result |
| Runtime | `quanta-runtime/src/indexing_machine_v2/store/source_bound_replay_v2/canonical_json_v2/native_corpus_admission_v3.rs`: four wire-bound identity loans, per-occurrence actual funding bank, complete work/copy errors |
| Runtime | 같은 디렉터리 `decoder_scratch_v3.rs`: recursive DATA/reader scratch/full decoder error를 highest Source 밖에 보관 |

Index 검증 명령 (lane `test-canonical-identity-lane`, 로컬 실행):

- `./scripts/cargow --lane test-canonical-identity-lane test -p quanta-index-contract-base -p quanta-index-contract --all-features --locked --quiet`
- `./scripts/cargow --lane test-canonical-identity-lane test -p quanta-index-contract-base -p quanta-index-contract --locked --quiet`
- `./scripts/cargow --lane test-canonical-identity-lane clippy -p quanta-index-contract-base -p quanta-index-contract --all-targets --all-features --locked -- -D warnings`

집중 oracle은 borrowed/owned 입력 및 pointer-preserving owned move, escaped/field-order
parity, 계층별 missing/unknown 및 owned duplicate 키 보존, late malformed/semantic/work/
copy/NFC refusal, full non-Copy Box 원본 주소 보존, 실제 NFC funding liveness, 무-poll 재진입,
occupied transfer 보존이다. 37 head/148 identity의 동시 DATA 보존 회귀도 포함한다.
최종 로컬 결과: all-features Rust **430 PASS**, default-features Rust **401 PASS**,
Clippy(all-targets/all-features) **VERIFIED**. `just rust-public-api`,
`just rust-hexagonal`, `just rust-cargo-modules`, scoped fmt/diff check도 **VERIFIED**다.
public-api snapshot은 default feature 표면이며 native ABI는 compile/test/Clippy로 검사했다.
Hexagonal 검사는 DTO canonical construction protocol의 기존 정확한 signature guard를
새 unit/full-error/native-String-birth 계약에 맞췄다. Core로 옮기면 contract→core 의존
cycle가 생기는 경계이며, storage/transport/service port는 추가하지 않았다. guard를
느슨하게 만들지 않았고 service method/별도 port 주입 거부를 포함한 Python 회귀 **8 PASS**를
확인했다. 이 테스트는 Semantica actual Source,
collection admission, daemon E2E, remote CI 또는 paired publication proof를 대신하지 않는다.

### Borrowed scope validation의 외부 NFC DATA 확장 — 2026-10-09

`2bf2353d` corpus candidate 이후 발견한 별도 producer delta다. RepoId와 RevisionId에
`validate_str_into_with_native_admission_v1(value, &mut NativeNormalizationDataV1<E>, &mut P)`를
공개했다. 반환형은 `Result<(), NativeIdentityConstructionErrorV1<E>>`이고 full non-Copy E를
그대로 이동한다. 기존 `validate_native_identity_into_v1`/동일 predicate/NFC producer만
호출하며 identity String/typed identity를 만들거나 normalize/revalidate하지 않는다.
이 경로는 scratch/grant를 해제하지 않는다. 부모가 full result와 실제 DATA/funding bank를
highest Source finisher까지 보유하고 DATA를 bank보다 먼저 drop해야 한다.

같은 private validator에 freshness guard를 먼저 두었다. 이미 시도/retire된 normalization
DATA는 raw predicate나 admission을 poll하기 전에 거절하며 원래 결과·backing을 보존한다.
**NFC 이전 raw predicate/work 거절에서는 normalization DATA가 fresh로 남는다.**
이 타입은 normalization attempt storage이며 전체 scope validation retry guard가 아니다.
Semantica 부모는 별도의 scope attempt 상태와 complete returned error를 보존해야 한다.
이를 새 normalizer/issuer나 vendor DATA state 확장으로 대체하지 않았다.

새 oracle은 양 ID의 기존 error order, 실제 long NFC scratch의 success/retire/reuse no-poll,
late normalization refusal의 동일 Box 원본 주소 및 transient policy 종료 후 실제 funding
객체의 생존이다. 기존 `2bf2353d`의 430/401 결과와 새 delta 실행 결과는 별도로 기록한다.
Semantica Scope/recursive receiver 채택과 Original Source 수용은 계속 `NOT_RUN`이다.

이번 delta의 별도 로컬 실행 결과는 all-features **433 PASS**(새 회귀 3개 포함),
default-features **401 PASS**다. 두 contract crate의 all-targets/all-features Clippy
`-D warnings`, scoped fmt, default public-api, hexagonal 및 diff check는 **VERIFIED**다.
실행 명령은 앞 절과 같은 `./scripts/cargow --lane test-canonical-identity-lane`
`test -p quanta-index-contract-base -p quanta-index-contract [--all-features] --locked --quiet`와
동일 package의 `clippy --all-targets --all-features --locked -- -D warnings`다.
이 결과는 앞선 430/401 결과에 합산하지 않으며 Native 공개 API는 feature compile/test로
검사했다. default public-api snapshot에는 이 feature 전용 메서드가 포함되지 않는다.

### SDK connect native 경계의 추가 대조 — 2026-10-09

**정적 대조 완료, 새 connect producer/receiver ABI와 Native 실행은 `NOT_RUN`.**
`ConnectOptions::from_state_root` (`sdk/src/config.rs:56`)는 state-root PathBuf 하나를
만든다. `resolve_profile`은 deadline/timeout policy를 먼저 검사하고 state-root를 resolve하며,
root가 있으면 각 socket을 `root.join("search-plane").join(name)`으로 만든다. explicit socket/
environment precedence, Full/QueryOnly 분기를 보존해야 한다. `resolve_state_root_with`의
explicit root clone, 두 join의 중간 PathBuf, environment String/error도 custody 대상이다.

`QuantaIndex::connect`는 `resolve` 후 `from_resolved`를 호출한다. `from_resolved`의 actual
shared births는 query/control/ingest transport의 std Arc 세 개와 `QuantaIndexInner`의 Arc다.
QueryOnly에서는 query와 inner만 만든다. transport constructor는 PathBuf와 `ClientIoPolicy`를
보관할 뿐 실제 UDS dial을 하지 않는다. dial/request는 transport `send` → IPC `send_request`
경로에 있다. connect 성공만으로 socket 연결이나 daemon 수용을 주장할 수 없다.

실제 Semantica caller는 `quanta-runtime-retrieval-kernel/src/index_sdk_ingress/connect.rs`의
default/relative-timeout(Duration)/absolute-deadline(Instant) 세 함수다. ingress consumer의
late Source classification/diagnostic은 complete SdkError와 options/path/client partial DATA를
함께 보유해야 한다. elapsed absolute deadline을 상대 timeout으로 재시작하면 안 된다.

현재 SDK/IPC에는 authentic native shared-header owner를 호출하는 connect port가 없다.
단순히 ordinary `connect` 결과를 외부 slot에 넣거나 std Arc allocation을 새 callback으로
감싸는 것은 기존 Core original shared producer에 도달했다는 증거가 아니다. Core owner의
실제 typed shared handle과 SDK client의 현재 std Arc fields를 연결하는 producer/receiver
계약을 먼저 확정해야 한다. 새 allocator/parallel connector/ordinary-to-Native adopter는
추가하지 않았다. 이 불확정 경계는 위 corpus producer 구현 완료와 별개다.

### SDK 구현 후보 통합 재감사 — 2026-10-08

Updated: 2026-10-09 (KST).

이 절은 preparation SDK와 publication/V5 소비자의 남은 통합을 소유한다.
**독립 단위는 local main에 반영했고, breaking publication/V5 전체 merge는 보류한다.**
아래 과거 candidate proof와 이번 main 실행을 합산하지 않는다.

#### 현재 위치와 변경 소유권

| 저장소·위치 | 현재 소스·판정 |
| --- | --- |
| Quanta local main | `433a9363`: consumer backport `c75668c7`, 독립 tooling `23b5659f`, preparation `433a9363` 통합. 기존 `ActivationAfterPublish` 유지 |
| Quanta `codex/sdk-preparation-final` | HEAD `36e16d5d`, base `09387a9a` 대비38경로. SDK/API·전체 daemon·실제 L2 검증 완료; main에 남은 breaking patch는25경로 |
| Semantica 원 후보 `codex/semantic-publication-v5` | HEAD `6c3922fb219`: owned28 보존. Foreign parser/native dirty는 feature가 아님 |
| Semantica 최종 기능 후보 `codex/sdk-publication-complete` | HEAD `34cc31cab7c`; implementation `2ce4f205ffa`, committed main `3b8f86dd` 기반36경로. 기존28 + typed rebaseline8경로. 정확한 V4 거부 oracle·hooks 통과; Runtime acceptance 미완료 |
| 최신 canonical Semantica pair | `/Users/songmin/.codex/worktrees/sdk-canonical-final/semantica-sdk`: main `3b8f86dd9ea` + 시점 고정한 dirty3,976경로 + 최종 feature36경로의 의미 보존 결합 + private dependency closure 수리. Sibling Quanta는 아래 physical snapshot, QGLang은 clean `9a9212da9526d50f9b264d7ee34dbe4cd0b03fd9` |
| Semantica shared main | 다른 owner가 진행 중. 이 작업의 production 변경은 아직 미적용. Kernel4 경로 claim은 검증 중 확보 후 미적용 상태로 해제. 재통합 시 lock·겹치는 test owner 조정과 claim 재획득 필요 |

Canonical capture는 path 집합과 실제 bytes를 두 번 대조해 고정했다. Capture digest는
`0f899aa554a8775cec3afd224997fde670aacf43c718de52152eb159d7d9f84b`이며
per-run 입력은 checkout 밖 `/tmp/sem-sdk-canonical-final-capture.json`에만 뒀다.
옛 foreign capture의 `217 composable / 50 conflicts`는 owned27 conflict 수가 아니다.
그50수리나 옛 Runtime19오류를 최신 main에 적용·재사용하지 않았다.

#### 최종 채택 판정: 전체 merge 대신 계약 단위로 분리

| 단위 | 현재 처분 | 정확한 경계·남은 조건 |
| --- | --- | --- |
| A · Nextest reporter2 | **main 통합 `23b5659f`** | `run-local-test-scope.py`와 테스트. `--status-level leak`으로 이름을 보존; timeout/selection/native 실행정책 유지 |
| B · 운영 경로2 | **main 통합 `23b5659f`** | `proof_operational_result.py`와 테스트. 정규 절대 경로, root/별칭/install-state overlap를 actor 전에 거부. 실제 symlink/권한/설치 proof는 아님 |
| C · preparation9경로/shared hunk | **main 통합 `433a9363`** | `src/preparation/{mod.rs,tests.rs}`, external `preparation_public.rs`, SDK Cargo.toml·lock, lib.rs·lexical.rs·API baseline·L2 scenario. 기존 batch/publication API로 독립 검증 |
| D · publication25경로/shared hunk + Sem kernel4 | **보존; 실제 aggregate caller acceptance 후 결합** | `AfterPublish`/explicit API/committed receipt·CLI/benchmark/SDK/runtime/API를 함께 반영. Kernel28만으로 수용하지 않음. 현재 main의 새 consumer regression은 새 enum 양 stage로 전환해 유지 |
| E · V5 contract5 + Runtime23 | **28 Rust 전체 보존; 별도 수용** | writer/reader/snapshot/issuer/aggregate/cleanup/test와 typed intent 전달 단일 계약. V4 전환·feature·Runtime·durable6·process3 필요 |
| F · native test1 | **독립 보존** | `native_corpus_policy_tests_v3.rs`의 import/visibility만. 선택된 lib(test)에 필요하면 별도 선행 반영 |
| F2 · manifest fixture1 | **후보 수리·검증 완료 `6c3922fb219`** | `indexing_tests/commit_and_lane_status.rs`. 선언된 정확한 Risk slot 재사용; duplicate-lane 첫 거부 assertion 유지. Artifact 선택298passed |
| Sem lock2·foreign source | **옛 snapshot 복사 금지** | 현재 root/nested lock의 다른 dependency edge를 유지하며 SDK `ciborium`/`sha2`만 결합. Foreign dirty3,976경로는 소유 feature commit에 포함 금지 |
| 추가 플랫폼 설계 | **범위 제외** | daemon plugin registry·새 wire IR·native media engine·SDK durable store·V4/V5 병렬 내부 IR 필요 없음 |

#### 이번 main 수리와 실행

- **기존 main consumer compile 수리:** base `09387a9a` CLI/benchmark의
  `ActivationAfterPublish` 누락 E0004를 `c75668c7`로 수리. 원 cause exit/status와 원
  publication/receipt를 보존하며 benchmark diagnostic은 payload를 노출하지 않는다.
  `./scripts/cargow test -p quanta-index-searchctl -p quanta-index-retrieval-bench --lib --locked`:
  `VERIFIED`, CLI46 + benchmark125passed. 두 package strict Clippy도 통과.
- **Membership no-op 수리:** batch builder와 preparation이
  `canonicalize_semantic_cluster_memberships_v1`을 공유한다. 순서만 바뀐 public regression을
  실제 실패시킨 뒤 hash 전에 동일 정렬로 수리했다. 동일 wire/prior, CBOR resume, no-op과
  실제 mutation replace를 함께 검사한다.
- **Budget 계약:** `max_emitted_text_bytes`는 chunk+semantic text 합계다.
  원본은 input limit, symbol/semantic metadata와 전체 serialized contribution·wire는 batch
  limit이다. 정확한 경계와1byte 초과, input>text limit, combined text, metadata를 검사한다.
  RSS/allocator 또는 O(changed-files) 보장으로 해석하지 않는다.
- **SDK:** `./scripts/cargow test -p quanta-index-sdk --lib --test preparation_public --locked -- --quiet`:
  `VERIFIED`,145unit +7external. Strict Clippy, `just rust-public-api`,
  `just rust-cargo-modules`, `just rust-hexagonal`: 모두 `VERIFIED`.
- **Shared:** `just rust-profile validate-shared-surface`: `VERIFIED`, 선택 compile,
  1,079shared +248integration(8skip) +25CLIpassed. 역사적 후보1,087과 다른 현재 split이다.
- **실제 main daemon:** `./scripts/cargow --lane daemon-lane build -p quanta-index-searchd-runtime
  --bin quanta-index-searchd --all-features --locked --message-format=json-render-diagnostics` 성공.
  실제 artifact SHA256 `1005ff0b3034dd881c3d89875573d382530105db5e6ffb741b20101acca0fbdd`를
  사용해 `QUANTA_INDEX_L2_TEST_BINARY=<artifact> ./scripts/cargow test -p quanta-index-sdk
  --test l2_daemon_publication --locked prepared_text_and_markdown_move_delete_noop_survive_restart
  -- --ignored --nocapture --test-threads=1`: `VERIFIED`,1passed/4filtered.
  현재 main의 full/move/delete/no-op/prior resume/restart query이며 전체5개·8cuts 또는
  전체 daemon300을 이번에 다시 실행했다는 뜻은 아니다.
- **독립 Python4파일:** `uv run --frozen --extra dev python -m pytest
  tools/ci/tests/test_run_local_test_scope.py tools/ci/tests/test_proof_operational_result.py
  -q -o cache_dir=/tmp/qi-final-integration-pytest`:75passed와 Ruff clean, `VERIFIED`.
- **게시 경계:** 이번 작업에서 push 명령은 실행하지 않았다. 후속 `git ls-remote origin refs/heads/main`에서
  원격 main `433a9363ed7ce52cd60a7c33842e9a391a815481`과 코드3커밋의 존재를 직접 확인했다.
  최신 문서 커밋은 로컬이다. Remote CI·installed Linux·scale/performance는 `NOT_RUN`.
  실제 host/destination/backup/observer 입력이 필요한 운영 수용은 계속 `BLOCKED`.

#### 현재 pair 실행과 남은 수용

**Quanta 후보 최종 실행:** `36e16d5d`의 production source에서 SDK156unit +7external,
CLI46 +benchmark125, 세 package strict Clippy, SDK/contract API 검사는 `VERIFIED`다.
이번에 같은 후보로 daemon을 새로 빌드했다. 실제 binary SHA256은
`0bbd47275904d0cf595a4bc195cd412af3092dbe0e503412530d85da0982bcdb`다.
이 binary로 `l2_daemon_publication -- --ignored --nocapture --test-threads=1`을 실행해
5passed와8개 named crash cut을 확인했다. Preparation move/delete/no-op/prior resume/restart,
원 publication 복구, retargeted replay를 포함한다. `just rust-profile test-daemon`도
300passed/10skipped,2slow,375.138s,exit0이며 이번 실행에는 leak 보고가 없다.
이 시간은 해당 correctness rail의 실행 시간이며 성능 benchmark 결과가 아니다.
Candidate 결과를 기존 API를 유지한 main의145unit/선택 L2 결과와 합산하지 않는다.

**Semantica의 두 입력을 분리한다.** 기존 canonical working-tree capture는 main `3b8f86dd`
+foreign dirty3,976경로였다. Kernel28과 contract `prepared_commit_receipt`9,
`indexing::indexing_tests`298, exact V4-baseline 거부1/V5 digest-bound snapshot1은
각 owning QBC GREEN 및 publication-complete를 확인한 해당 snapshot 결과다.
Contract의 첫297pass/1fail은 manifest의 정확한 Risk coverage slot으로 fixture를 수리한 뒤
같은 선택298pass로 종료했다. Duplicate-lane 첫 거부 assertion을 유지했다.

**최신 canonical Runtime 실행:** owning QBC `quanta-runtime`, `test/lib`,
`shadow_delta_orchestration`, `feature-isolation:quanta-runtime.no-default.9c3270892708`,
`--ignored-policy exclude --max-test-threads 1`, 기존 `sdk-publish-recovery` lane:
`FAILED`, finished owner `RED`, exit101, passed/failed/ignored 모두0. 실제 feature closure는
`--no-default-features --features search-plane-sdk-ingress-proof`다.
Run `20261008T152558.067342Z-f3564a380bae`, source digest
`816a260c609bcc63dc2a67289ad098adc3cdec4150b550f9f830589b770891af`,
결과 위치 `/tmp/sem-sdk-canonical-final-runtime-v10.json`.
`quanta-adapters-parser`126개 primary source의334진단으로 컴파일이 중단됐다.
이는 private captured pair의 진단 수이며 current shared-main 오류 수가 아니다.
R3·typed intent·durable6·ignored process3 본문은 계속 `NOT_RUN`이다.

Private closure에서는 BuildContext checked carrier/read, symbol·foundation·import-graph,
relation authority의 fallible ordinary copy/identity, execution의 source custody 검사를 보완했다.
Parser 모듈 연결·필요한 owner export·정확한 trait import도 결합해 raw type/lifetime 검사까지
도달했다. 기존 raw API/Clone 복원, feature 제거, 실패의 기본값 전환은 하지 않았다.
이 변경들은 foreign migration 수리이며 SDK36경로 feature commit에 포함하지 않는다.
정확한 preimage가 있는137경로의 검토용 delta는 checkout 밖
`/tmp/sem-sdk-private-closure-repairs.patch`와
`/tmp/sem-sdk-private-closure-repairs-paths.json`에 보존했다. 이전 foundation·
BuildContext·source-authority 수리는 그 patch의 포함 범위가 아니며 private source와 별도 메모를 확인한다.

**실제 선행 경계:** 단순 missing-module 단계는 종료됐고 다음 producer 계약 결합이 필요하다.

- `captured_semantic_workspace_generation_v1/{python_projection_v1,python_frontend_authority_v1}.rs`,
  `product_callable_resolution_authority_v1/go_retained_parse_hir_v1.rs`: 제거된 raw tree 접근과
  Clone 소비자를 tree owner의 실제 current/loan으로 전환한다. 보호된 tree를 ordinary copy로 우회하지 않는다.
- `language_host_adapter/java_callable_lowering_view_v1/checked_read_v3/ledger_projection_v3.rs`,
  `java_workspace_resolution_syntax_v1/{part_1,part_3,workspace_batch_members_v3}.rs`,
  `adapter_impl/{augment.rs,augment/native_import_transaction_v3.rs}`: native admission trait,
  오류 타입, HRTB callback·borrow lifetime을 producer와 소비자가 함께 맞춘다.
  trait import와 실제 borrowed-data escape·불충분한 FnOnce/FnMut 수명 진단을 구분한다.
- 121경로 scoped boundary closeout에서 클래스 초기화자 상수·DIP·Python-min·query frontier 등은
  통과했다. `quanta-runtime-analysis.interprocedural-source-contracts.v1`은 imported seal,
  common-truth owner path, retained projections, frontier/cache/telemetry, PTA FileText custody의
  9개 조건으로 `FAILED`다. 새 primitive나 검사 완화로 이를 대신하지 않는다.
- Shared main과 parser 수리 preimage111경로를 직접 대조했을 때101동일/10변경/누락0이었다.
  따라서 private migration 전체 파일 복사로 최신 main을 덮어쓰지 않는다. Feature36경로와
  foreign source/lock owner의 완결된 source를 조정한 뒤 동일 QBC를 재실행한다.

**Committed-main 검증 입력:** original owned Rust26개와 committed main `3b8f86dd`를
직접 대조해24개 preimage 동일·2개 양쪽 absent·mismatch0을 확인했다. 따라서 foreign
migration을 포함하지 않는 `/Users/songmin/.codex/worktrees/sdk-committed-final/semantica-sdk`,
branch `codex/sdk-publication-complete`에서 `3b8f86dd` +owned26Rust +현재 lock의 SDK
`ciborium`/`sha2`2edges로 별도 검증한다. Sibling Quanta는 실제 detached `36e16d5d`,
QGLang은 실제 detached `9a9212d`다. Committed-base 결과는 dirty shared-main의 통과를 뜻하지 않는다.
명시적 rebaseline의 실제 SDK 전달·artifact retry 계약·V4→V5 process 전환 구현은 `2ce4f205ffa`에, 정확한 V4 baseline 거부 oracle은 `f60a6bd5a81`, artifact-ref 거부 oracle은 `34cc31cab7c`에 보존했다. 같은 frozen attempt의 artifact-ref 대체 거부 unit은 선언된 ref를 사용하며, 실제 materialized full/delta 두 artifact의 retry 검증으로 확대 해석하지 않는다.
Committed-base Runtime owning QBC는 기존 `quanta-adapters-parser`101diagnostics로 `FAILED`,exit101/0test body였다. Source digest `42f301a8c09a43c16b74c881cb3000bea5b287e3f41865c2b34d16365aaa3b93`다. 따라서 clean base도 Runtime acceptance를 대체하지 못했다. Canonical pair에서 private closure 수리와 feature36의 의미 보존 결합 후 같은 Runtime rail을 재실행한다. Durable·process body는 아직 `NOT_RUN`이다.

**다음 수용 조건:**

- Artifact 계약의 fixture 실패는 위298passed로 종료했다. Runtime compile과 process acceptance는
  이 계약 결과와 별도이며 계속 미완료다.

1. Committed-base와 canonical working-tree의 실패를 분리한다. 현재 canonical pair의
   output-owner closure를 의미 보존해 복구한 뒤 `shadow_delta_orchestration`, durable6,
   ignored process와 V4→V5 전환을 실제 실행한다. Private migration 수리는 SDK36경로
   commit에 포함하지 않는다. 다른 입력의 성공으로 dirty main이나 이 caller를 수용하지 않는다.
2. 새 test3파일은 V4 artifact의 V5 baseline 거부, shadow on/off lexical full/delta,
   실제 G1→G2 predecessor 누락 시 activation0/원 active 유지→artifact 복구 후 retry를
   검사한다. G1/G2→prune/restart→G3도 유지한다. V4 artifact 거부는 위 exact 실행으로
   `VERIFIED`; feature/Runtime/process는 `NOT_RUN`. Completed V4→명시적
   V5 full→delta 전체 process 전환은 아래 clean pair의 새 회귀로 검증한다. Shared main은
   active base가 존재하면 planned delta를 유지하며 explicit rebaseline 선택이 아직 없다.
   SDK의 replace-generation primitive 존재만으로 Runtime upgrade 경로 구현을 선언하지 않는다.
   Clean pair의 보완은 `SearchPlanePublicationIntentV1::RebaselineExistingPredecessor`를
   host-issued authority→sealed ingress→projection controls→canonical full plan으로 전달한다.
   Planned predecessor와 frozen active가 일치해야 하며, source-event ancestry를 유지한다.
   Artifact-ref 재시도 계약과 실제 legacy V4 issuer→V5 full→V5 delta process 회귀는
   소스에 추가했고 owning 실행을 기다린다. initial G1에 이 intent를 적용하면 거부한다.
3. 현재 main의 aggregate authority도 이미 `sqlite-store`와 retrieval/workspace bundle을
   요구한다(`authority_assembly.rs`). No-SQLite aggregate 거부만으로 새 회귀라고 하지 않는다.
   실제 지원 feature 조합과 shadow on/off를 owning rail로 확정한다.
4. D의 기존 V4 aggregate publication/replay/CAS/restart acceptance를 확보하거나 D/E를
   완결된 pair로 수용한 뒤 반영. API와 main에 추가한 consumer test를 함께 결합하고 영향
   SDK/API/shared/daemon rail을 최종 source에서 재검증한다.
5. Sem shared main의 test/lock owner·barrier를 조정하고 정확한 owned 경로만 commit.
   Foreign source·선행 unpublished ancestor를 이 작업으로 stage/push하지 않는다.
6. Projection-root 전체 cold restart와 Linux 운영은 현재 fixture의 수용 범위 밖이다.
   Manifest로 root를 추정하거나 source-preparation에 두 번째 durable protocol을 만들지 않는다.
7. 과거 Nextest300pass 중 leaky1는 원인 미확인 P2 triage다. Reporter 통합으로 다음
   재현의 test 이름을 보존한다. 실제 영향 cleanup 실패가 재현되면 해당 owner scope를 막는다.

B1–B3는 아래 canonical dependency의 소유 경계이며 이번 feature commit 목록이 아니다.

- **B1 · Runtime identity/controlled output** — `quanta-runtime-kernel-contract/src/domain/contracts/`
  의 `query_{pre_key_admission_v1,source_execution_authority_v1,output_read_context_v3}`와
  `shared_query_source_v3.rs`; `src/ports/{input_cell_store.rs,query_executor_port.rs,
  query_executor_port/,pre_native_function_build_authority_port_v1/,
  runtime_ctx/runtime_services/telemetry/}`. 도달하는 `quanta-runtime-query-executor-owner`,
  `quanta-runtime-query-surface-foundation`, `quanta-contract-error-shared` consumer를 같이 확인.
  `canonical_verified_operations_v3.rs`는 현재 존재하므로 옛 missing-file blocker는 종료;
  전체 source authority/copy 계약의 컴파일은 별도 미검증이다.
- **B2 · Parser/Java retained authority** — `quanta-adapters-parser/src/language_host_adapter/`
  의 `java_workspace_resolution_syntax_v1/`, `java_callable_lowering_view_v1/`,
  `java_resolved_type_identity_catalog_v1/`,
  `java_workspace_generation_authority/ordinary_generation_materialization_v1.rs`,
  `semantic_expression_identity_ledger_v1/`, `semantic_module_v3/`와 `src/` 바로 아래의
  `semantic_attribution_java_workspace_production_owner_v1/`. `packages/core/`의
  `codegraph-lang-{common,java}`, `codegraph-cfg-dfg-kernel`, `codegraph-defuse-extractor-java`
  계약까지 actual enabled closure로 확인. 예전 격리 parser 수리를 최신 producer에 덮어쓰지 않음.
- **B3 · PTA/native carrier** — `quanta-analysis-interproc-contract/src/ports/pta_solver/`의
  `delta_result_{carrier,substrate}_v1`, `incidence_index_v1`, `pair_delta_v1`, `state_v2.rs`;
  `quanta-contract-pta-result-shared`와 `quanta-adapters-pta/src/`의 production workspace,
  prepared native payload, constraint extraction/handoff 소비자. `packages/core/codegraph-native-allocation-core`
  의 admitted storage/copy와 `quanta-taint-solver` 도달 edge도 유지.

**Coverage 수리 구조:** 새 digest나 중복 metadata 필드 대신 기존 `SourceFileCoverage`를
`PriorSourceEntry`의 canonical owner로 보관한다. source/profile/unit-set getter는 그 값에서
파생하고 no-op은 coverage 전체와 semantic stamp를 비교한다. 아직 미게시된 새 prior tuple은
한 형식으로 수정하고 이전 incomplete tuple은 거부한다. 새 wire protocol·legacy reader 없음.

#### Coupled D/E의 소유 코드 결합 순서

아래 순서는 D/E 전체를 통합하는 경우의 절차다. A/B/C에 V5 선행 조건을 부과하지 않는다.

1. 현재 canonical producer의 완결된 source와 실제 enabled dependency closure를 먼저 확보한다.
   stale foreign proof 수리50개를 재생하기보다 새 격리 producer 기반에 현재 feature36경로를
   적용하는 것을 우선 검토한다. 어떤 선택이든 기존 dirty 작업은 보존하고 동일 invariant
   충돌은 해당 owner와 결합한다. 단순 파일 수나 source 정적 검사로 compile 완료 판정 금지.
2. Quanta 후보의 coverage 수리와 benchmark consumer 변경을 공개 생성자·serde·API baseline과 함께 결합한다.
   두 결함의 집중 회귀 및 공개 API 검사는 완료했다. 최종 pair 검증은 별도다.
   canonical source가 같아도 global revision 변경은 chunk identity/replacement를 바꾼다.
   이 구현을 O(changed-files) 또는 embedding reuse 보장으로 홍보하지 않는다.
3. Semantica owned 목록은 `git diff --name-only 3b8f86dd9ea 34cc31cab7c`, Quanta 목록은
   `git diff --name-only 09387a9a <final-sdk-head>`로 확인한다. 초기37 inventory에 CLI
   `tests/control.rs` 회귀가 추가되어 현재 후보 경로는38개다. Semantica 두 lock의 SDK
   `ciborium`/`sha2` edge와 새 canonical path dependency edge를 결합한다. 기존 registry 버전
   upgrade는 이 작업의 요구가 아니며 자동 lock 재생성 부수 변경을 그대로 채택하지 않는다.
   이전 snapshot의 root lock 차이는7 package row, nested는3 row였다. 현재 lock은 다시 대조한다. Root의 indexing-control-plane,
   runtime-pta-consult-shared, runtime-test-support-pta, sdk-runtime-executor,
   sdk-session-backend-owner, taint-solver edge와 nested의 contract-error-shared,
   taint-solver edge를 보존하고 양쪽 index-sdk의 `ciborium`/`sha2`를 결합한다.
4. 실제 Semantica manifest가 선택하는 sibling Quanta를 최종 SDK 소스와 일치시키고 QBC
   dependency resolution으로 확인한다. 이번 canonical pair의 actual sibling은 preparation canonicalization/budget 보완을 포함한
   SDK 후보 checkout이다. 예전 `e3499fce` bytes와 동일하다고 판정하지 않는다.
   다른 checkout의 성공·옛 바이너리·서로 다른
   source receipt를 합쳐 pair 성공으로 처리하지 않는다.
5. 아래 검증을 기존 warm lane에서 직렬 실행한다. 실패 시 owner 원인을 수리하고 같은 rail로
   재검증한다. 소스 변경이 없는 기존 Quanta proof는 범위를 비교해 재사용하고 무관한 XL
   benchmark를 반복하지 않는다. API 변경 이후 baseline과 영향 SDK 테스트는 다시 실행한다.
6. pair 검증 후 **shared main에 적용하기 전에** Semantica의 정확한 owned 경로 claim,
   두 lock owner 조정, 현재 HEAD와 integration barrier를 확보한다. 적용 후 실제 main의
   source/lock·sibling dependency가 검증한 pair와 같은지 대조하고 달라진 영향 proof를
   재실행한 다음 `tools/shared-main/shared-main` coordinator로 commit/push한다. Quanta main의
   owned dirty 문서는 후보와 대조·결합하고 기존 내용을 버리는 reset/checkout으로 merge를
   통과시키지 않는다. 필요한 CI와 두 repository의 실제 게시 SHA는 별도로 확인한다.

#### 실제 결합 파일

아래는 owned bundle의 정확한 경로다. B1–B3 foreign dependency owner 목록과 구분한다.
Quanta의 main 잔여25경로 patch는 `git apply --check`를 통과했지만 아직 적용하지 않았다.
Quanta 경로는 repository root 기준이고, Semantica Rust 경로는
`packages/analysis/quanta-v2/crates/` 기준이다. 두 lock은 Semantica root 기준이다.
최종 적용 직전 위 `git diff --name-only`로 추가 변경이 없는지 다시 대조한다.

**Quanta38 (기존37 + main backport에서 확장한 CLI regression1경로):**

```text
Cargo.lock
benchmarks/retrieval/src/sdk.rs
crates/quanta-index-contract/src/ipc/ingest_observation.rs
crates/quanta-index-sdk/Cargo.toml
crates/quanta-index-sdk/src/binding.rs
crates/quanta-index-sdk/src/client.rs
crates/quanta-index-sdk/src/error.rs
crates/quanta-index-sdk/src/generations.rs
crates/quanta-index-sdk/src/lexical.rs
crates/quanta-index-sdk/src/lib.rs
crates/quanta-index-sdk/src/preparation/mod.rs
crates/quanta-index-sdk/src/preparation/tests.rs
crates/quanta-index-sdk/src/tests.rs
crates/quanta-index-sdk/src/tests/control_tests.rs
crates/quanta-index-sdk/src/tests/corpus_ingest_tests.rs
crates/quanta-index-sdk/src/tests/explicit_activation_adversarial_tests.rs
crates/quanta-index-sdk/src/tests/published_activation_tests.rs
crates/quanta-index-sdk/src/tests/replay_tests.rs
crates/quanta-index-sdk/tests/l2_daemon_publication.rs
crates/quanta-index-sdk/tests/preparation_public.rs
crates/quanta-index-searchctl/src/lib.rs
crates/quanta-index-searchctl/src/render.rs
crates/quanta-index-searchctl/src/tests/control.rs
crates/quanta-index-searchd-runtime/tests/e2e_ingest_preflight.rs
crates/quanta-index-searchd-runtime/tests/e2e_lifecycle_history.rs
crates/quanta-index-searchd-runtime/tests/sdk_frontdoor.rs
crates/quanta-index-searchd-runtime/tests/sdk_frontdoor/failure_tests.rs
crates/quanta-index-searchd-runtime/tests/sdk_frontdoor/matrix_tests.rs
docs/adr/MAY-27-002-sdk-ingress-and-public-surface-boundary.md
docs/adr/OCT-04-003-source-preparation-sdk.md
docs/adr/OCT-05-003-active-query-and-runtime-lifecycle.md
docs/plans/oct-4-parallel-closure/tickets/INDEX.md
tools/ci/lint/baselines/public-api/quanta-index-contract.txt
tools/ci/lint/baselines/public-api/quanta-index-sdk.txt
tools/ci/proof_operational_result.py
tools/ci/run-local-test-scope.py
tools/ci/tests/test_proof_operational_result.py
tools/ci/tests/test_run_local_test_scope.py
```

**Semantica 최종36 (기존28 + 실제 typed rebaseline 전달8):**

```text
Cargo.lock
packages/analysis/quanta-v2/Cargo.lock
quanta-contract-retrieval/src/indexing_tests/prepared_commit_receipt.rs
quanta-contract-retrieval/src/indexing_tests/commit_and_lane_status.rs
quanta-contract-retrieval/src/indexing_tests/prepared_commit_receipt/handoff_cleanup_fixtures_v3.rs
quanta-contract-retrieval/src/indexing_writer_ports/admitted_decode_v3/native_corpus_policy_tests_v3.rs
quanta-contract-retrieval/src/indexing_writer_ports/part_2.rs
quanta-contract-retrieval/src/indexing_writer_ports/prepared_identity_v3.rs
quanta-contract-retrieval/src/indexing_writer_ports_serde/part_1.rs
quanta-runtime-retrieval-kernel/src/index_sdk_ingress/error.rs
quanta-runtime-retrieval-kernel/src/index_sdk_ingress/publish.rs
quanta-runtime-retrieval-kernel/src/index_sdk_ingress/publish/frozen_publication_tests_v1.rs
quanta-runtime-retrieval-kernel/src/index_sdk_ingress/publish/sdk_error_classification_v1.rs
quanta-runtime/src/retrieval/port_impls/index_projection_writer/mod.rs
quanta-runtime/src/retrieval/port_impls/index_projection_writer/prepared_commit_artifact_v2.rs
quanta-runtime/src/retrieval/port_impls/index_projection_writer/prepared_commit_artifact_v2/orphan_inventory_v1.rs
quanta-runtime/src/retrieval/port_impls/index_projection_writer/prepared_commit_artifact_v2/orphan_inventory_v1_tests.rs
quanta-runtime/src/retrieval/port_impls/index_projection_writer/prepared_commit_artifact_v2/source_bound_cleanup_unlink_v2.rs
quanta-runtime/src/retrieval/port_impls/index_projection_writer/source_bound_projection_assembly/authority_assembly/prepared_aggregate_issuer_v1.rs
quanta-runtime/src/retrieval/port_impls/index_projection_writer/source_bound_projection_assembly/authority_assembly/prepared_aggregate_owner_pipeline_v1.rs
quanta-runtime/src/retrieval/port_impls/index_projection_writer/source_bound_projection_assembly/authority_assembly/search_plane_handoff_dispatch.rs
quanta-runtime/src/retrieval/port_impls/index_projection_writer/source_bound_projection_assembly/authority_assembly/search_plane_handoff_dispatch/aggregate_prepare.rs
quanta-runtime/src/retrieval/port_impls/index_projection_writer/source_bound_projection_assembly/authority_assembly/search_plane_handoff_dispatch/semantic_plan.rs
quanta-runtime/src/retrieval/port_impls/index_projection_writer/source_bound_projection_assembly/authority_assembly/search_plane_handoff_dispatch/semantic_state.rs
quanta-runtime/src/retrieval/port_impls/index_projection_writer/source_bound_projection_assembly/authority_assembly/search_plane_handoff_dispatch/tests/shadow_delta_orchestration.rs
quanta-runtime/src/retrieval/port_impls/index_projection_writer/source_bound_projection_assembly/authority_assembly/search_plane_handoff_dispatch/tests/shadow_delta_orchestration/completed_v5_publication.rs
quanta-runtime/src/retrieval/port_impls/index_projection_writer/tests.rs
quanta-runtime/src/sdk/search_builder/source_bound/snapshot_collector/source_closure_witness.rs
quanta-runtime/src/retrieval/assembly_data/source_bound_aggregate_coordinator_v2.rs
quanta-runtime/src/retrieval/port_impls/index_projection_writer/source_bound_final_assembly.rs
quanta-runtime/src/retrieval/port_impls/index_projection_writer/source_bound_final_assembly/aggregate_publication_ingress_v2.rs
quanta-runtime/src/retrieval/port_impls/index_projection_writer/source_bound_final_assembly/projection_controls_v1.rs
quanta-runtime/src/retrieval/port_impls/index_projection_writer/source_bound_projection_assembly/authority_assembly/ordinary_aggregate_prepare_v2.rs
quanta-runtime/src/sdk/search_builder.rs
quanta-runtime/src/sdk/search_builder/index_owner_env_authority/aggregate_publication_authority.rs
quanta-runtime/src/sdk/search_builder/source_bound/snapshot_collector/lane_assembly.rs
```

#### 과거 격리 Runtime owner 결과와 선행 수리

현재 canonical 실행은 위 절이 소유한다. 다음은 그 이전 격리 source 결과다.

당시 최종 SDK code `e3499fce`를 actual sibling에 결합하고 R3 고정 oracle을 포함해 아래 Runtime
module owner를 재실행했다. **`FAILED`**: finished owner verdict `RED`, exit101,
`quanta-runtime-kernel-contract`의19진단(typed15 + unused-import4), 테스트 본문0개.
즉 R3 테스트·Runtime aggregate 동작은 `NOT_RUN`이며 oracle 구현만으로 종료하지 않는다.
새 결과는 `/tmp/sem-sdk-r3-audit-20261008-result.json`, run ID
`20261008T093148.297111Z-f43087e4a43c`, source digest
`2d2669674eef6208f5ff61a93f046e43b15cb02cc3b49df86db6d0ca9eda1399`에 묶였다.
R3의 후속 local commit `6f5dc8fc13f`는 실행한 test source와 같은 바이트다.

19진단의 primary14경로는 다음과 같으며 **전부 현재 shared main과 바이트가 다르다**.
접두 경로는 `packages/analysis/quanta-v2/crates/quanta-runtime-kernel-contract/src/`다.

```text
domain/contracts/query_output_read_context_v3.rs
domain/contracts/query_pre_key_admission_v1.rs
domain/contracts/query_pre_key_admission_v1/source_paths_v1/exact_member_v3.rs
domain/contracts/query_source_execution_authority_v1.rs
domain/contracts/query_source_execution_authority_v1/original_identity_v3.rs
domain/contracts/query_source_execution_authority_v1/serving_issuance_v1.rs
domain/contracts/shared_query_source_v3.rs
ports/input_cell_store.rs
ports/pre_native_function_build_authority_port_v1/original_result_resource_v3.rs
ports/query_executor_port.rs
ports/query_executor_port/typed_input_v3.rs
ports/query_executor_port/validation.rs
ports/runtime_ctx/runtime_services/telemetry/query_ops.rs
ports/runtime_ctx/runtime_services/telemetry/read_ops.rs
```

- Source identity/admission: 옛 authority exports와 controlled identity type, generic error
  변환, `CanonicalIdentityMaterialErrorV3`/serving error 전달, `Cow` 오류 인자 계약의 불일치.
- Telemetry/read: `RuntimeCtxPayloadV1`과 `RuntimeCtx` 경계, `SharedSourceTextV1`의
  소유 read/copy 계약이 caller와 어긋난다.
- First-refusal/output: fallible observer를 `Result`에서 꺼내지 않은 소비,
  `FallibleTypedOutputCopyV3` bound, RuntimeCtx 복사 계약, 이동된 envelope의 재사용.

이 경로만 옛 candidate에서 수정하는 것은 수용 조건이 아니다. 최신 B1–B3 canonical source
전체와 owned27파일을 결합한 fresh pair를 만들고 이 owner부터 재실행한다. 현재 main 자체의
compile은 여전히 `NOT_RUN`; 이번19는 **격리 pair**의 실제 결과이며 main 오류 수가 아니다.

#### Owner-local / integration test plan

아래 실행 수치는 이전 full-candidate 기록이다. 이번 main/후보 보완 실행은 위 절이
소유하며, command recipe는 D/E 최종 source의 재검증에 사용한다.

- **Quanta 공개 coverage RED→GREEN:**
  `./scripts/cargow test -p quanta-index-sdk --test preparation_public --locked public_reconcile_replaces -- --nocapture`.
  수리 전2실패/exit101을 실제 확인했다. 잘못된 selector로 실행된0tests는 검증이 아니다.
  수리 후 아래 SDK 명령은152 unit+5 external 성공, SDK/CLI lib+tests strict Clippy도 성공했다.
  이어 `./scripts/cargow test -p quanta-index-sdk --lib --test preparation_public --locked`,
  SDK/CLI strict Clippy와 `python3 tools/ci/lint/check-public-api.py --packages quanta-index-contract quanta-index-sdk`.
- **Quanta benchmark consumer:**
  `./scripts/cargow test -p quanta-index-retrieval-bench --lib --locked classification -- --nocapture`.
  수정 전 E0004로 테스트 본문0개, 수정 후2passed/123filtered다. 두 failure stage에서
  원 timeout/unavailable/error·code와 stage를 보존하고 publication payload는 출력하지 않는다.
  `./scripts/cargow clippy -p quanta-index-retrieval-bench --lib --tests --locked -- -D warnings`의
  결과도 `VERIFIED`다. 실제 retrieval benchmark 실행·속도 판정은 `NOT_RUN`.
- **이전 full-candidate Quanta 실행 결과:** SDK152+external5, 공개 API 양쪽 baseline 검사 `VERIFIED`.
  fresh daemon build와 SDK L2의5process/8crash cuts도 `VERIFIED`.
  compiler artifact는 `/Users/songmin/Library/Caches/quanta-index/target/b83f409f3af32ff0/daemon-lane/debug/quanta-index-searchd`,
  실행 직전 SHA256은 `0bbd47275904d0cf595a4bc195cd412af3092dbe0e503412530d85da0982bcdb`였다.
  경로만으로 같은 바이너리라고 판단하지 말고 Semantica process 실행 시 다시 검증한다.
- **Lifecycle consumer RED→GREEN:** 첫 `just rust-profile test-daemon`은 history CAS 오류
  분류 누락으로 `FAILED`: 52/300실행,51pass/1fail/10skipped,248미실행.
  `./scripts/cargow --lane test-daemon-lane test -p quanta-index-searchd-runtime --test runtime_extended_suite --all-features --locked e2e_lifecycle_history::sdk_source_delete_append_pin_cas_duplicate_reorder_rollback_restart_history -- --nocapture --test-threads=1`
  은 수리 후1passed/94filtered. 원본 증거의 binding·durability를 검증한 Activation CAS 충돌만
  history의 Conflict로 취급하며, fail-fast 미실행248개를 성공으로 합산하지 않는다.
- **최종 daemon rail:** `just rust-profile test-daemon` 재실행은 `VERIFIED`:
  25catalog rows/5binaries,300passed/10skipped,2slow/1leaky,301.377s, exit0.
  test result 성공과 child/FD cleanup 완결을 구분한다. 기본 fail-only reporter로는 leaky
  테스트 이름을 알 수 없어 같은 command의 status/final-status만 all로 바꿔 추적한다.
  `./scripts/cargow --lane daemon-lane clippy -p quanta-index-searchd-runtime --test runtime_extended_suite --all-features --locked -- -D warnings`도 `VERIFIED`.
  같은 generated command의 reporter만 all로 바꾼 추가 실행도300passed/10skipped,
  2slow/0leaky,329.747s, exit0이었다. 같은300개를 두 번 실행한 것이며600개 독립 coverage가 아니다.
  원래 leak의 이름·원인은 이 미재현 실행으로 복원할 수 없으며 cleanup 수리 완료로 처리하지 않는다.
- **공통 reporter 수리:** `tools/ci/run-local-test-scope.py`는 `--status-level leak`으로
  failed/slow/leaky 이름을 보존하고 성공 test body 출력은 계속 끈다. 최종 요약은 fail-only다.
  설치된 Nextest help의 live status-level에는 leak이 있지만 final-status-level에는 없으므로
  잘못된 final-level 옵션을 만들지 않는다. `uv run --frozen --extra dev python -m pytest tools/ci/tests/test_run_local_test_scope.py -q -o cache_dir=/tmp/qi-sdk-local-scope-pytest-20261008`
  은14passed, 두 영향 Python 파일의 `uv run --frozen --extra dev python -m ruff check`도 통과했다.
  Rust 실행 입력·timeout·concurrency·선택 inventory 변경은 없다.
- **Shared surface:** `just rust-profile validate-shared-surface`는 `VERIFIED`.
  all-target/all-feature compile 뒤 shared5binaries1,087passed,
  integration-fast15binaries248passed/8skipped, CLI2binaries25passed다.
  이 결과는 daemon 또는 Semantica Runtime proof를 포함하지 않는다.
- **Semantica owning QBC:** 아래 공통 요청을 `owner resolve`에서 확인한 뒤 같은 flags로
  `owner run --lane sdk-publish-recovery --jobs 2 --result-json </absolute/outside-checkout.json>`.
  `--execution-kind test --target-kind lib --max-test-threads 1 --ignored-policy exclude`를 사용한다.
  - `--package quanta-runtime-retrieval-kernel --selector-mode module --selector index_sdk_ingress::publish
    --compile-policy feature-isolation:quanta-runtime-retrieval-kernel.no-default.ed0772b29304`.
    기존 finished receipt는28passed/81filtered. retarget는 activation0회·원본 증거 유지,
    동일 identity replay는 정상 활성화. 실제 control IPC/전체 aggregate 재개 증거와 구분한다.
  - `--package quanta-contract-retrieval --compile-policy package-all-features`:
    `--selector-mode module --selector prepared_commit_receipt` 이름 필터와 exact
    `indexing::indexing_tests::source_bound_v5_artifact_requires_digest_bound_snapshot_bytes_v1`.
    전자는 `indexing_tests.rs`의 `include!`에 들어간 libtest 이름9건의 selection이며 파일 전체
    모듈 검증이라는 뜻이 아니다. 기존9+1GREEN은 artifact contract 범위이며 Runtime payload
    의미 검증이 아니다.
  - `--package quanta-runtime --selector-mode module --selector shadow_delta_orchestration
    --compile-policy feature-isolation:quanta-runtime.no-default.9c3270892708`.
    G1→G2→G3, unchanged/rename/delete/clear, scope3축, typed membership, canonical serde,
    missing/foreign predecessor·mutation tamper 거부를 실제 실행한다.
  - 같은 Runtime policy로 [기존 durable barrier6개](#o4-i0-03)를 exact 실행한다.
    각 selector의 witness와 finished receipt를 확인하며0tests/compile-only는 실패한 수용이다.
- **R3 expected partition:** 이전 owner 집합과 현재 producer emission을 fixture 입력으로
  고정하고 replace/tombstone/unchanged를 명시한다. 테스트 expected set을 SUT planner의
  출력에서 만들지 않는다. delete·rename·parent membership·다른 kind의 동일 ID·surface clear를
  검사하고, 의도한 replace/tombstone을 제거한 negative case도 검출해야 한다.
  기존 `shadow_exact_delta_production_orchestration_reopens_and_persists_derived_delta_v1`에
  이 oracle을 구현했다. Test-only1파일172줄이며 production planner 변경은 없다.
  관측 오류 수를 줄이는 로직이나 두 번째 production planner를 추가하는 작업이 아니다.
- **실제 aggregate process:** exact
  `retrieval::port_impls::index_projection_writer::source_bound_projection_assembly::authority_assembly::search_plane_handoff_dispatch::tests::shadow_delta_orchestration::completed_v5_publication::completed_v5_publication_prune_restart_then_g3_delta_v1`,
  및 같은 module의 `completed_v5_baseline_refuses_missing_predecessor_before_activation_then_retries_v1`을
  각각 exact selector로 실행한다. 위 Runtime policy·`--ignored-policy only`. 최종 소스로 산출한 daemon의 actual compiler artifact를
  `QUANTA_INDEX_SEARCHD_BIN`과 실제 SHA256으로 전달한다. G1/G2 완료→G1 삭제→SQLite/reader와
  SearchPlane 재시작→production G3를 실행한다. Projection owner와 테스트 observer 유지 범위를 기록.
  기존 daemon 경로의 bytes가 바뀌었으므로 옛 SHA를 그대로 재사용하지 않는다.
- **Quanta 최종 영향 경계:** SDK process L2의 full/move/delete/no-op/restart,
  `runtime_fast_suite`의 `sdk_frontdoor`, `runtime_extended_suite`의 `e2e_ingest_preflight`,
  공유 surface/activation 변경에 요구되는 `just rust-profile validate-shared-surface`와
  `just rust-profile test-daemon`을 최종 묶음에서 확인한다. Frontdoor 집중 성공을 전체 rail
  성공으로 승격하지 않는다. Full/release/remote CI·성능은 선택된 계약의 별도 상태로 남긴다.

**Merge 종료 조건:** 아래 채택 단위마다 필요한 owning check를 통과시킨다. V5 전체 통합은
Runtime owner·durable6·process·최종 dependency binding이 필요하다. 독립 tooling/preparation을
이 미실행 V5 proof 때문에 일괄 보류하지 않는다. Breaking publication은 실제 Semantica
consumer와 함께 검증한다. 해당 검증을 생략하기 위해 V5 파일을 임의로 덜어내지는 않는다.

Nextest leaky1은 별도 P2 관측이다. exit0/300passed와 동일 inventory의 미재현만으로
cleanup 수리를 주장하지 않되, 원인이 불명확한 관측 하나를 모든 SDK 변경의 무조건 merge
금지 조건으로 두지도 않는다. 다음 재현에서 owner를 보존하고, 변경 관련 child/FD 수명
결함이나 요구된 cleanup 계약 실패가 확인되면 해당 범위의 gate로 처리한다. Reporter 수리와
leak timeout은 유지한다. 전체 cold-root 복구·Linux operations·qualified performance는
별도 계약/입력이 필요하며 이번 SDK merge의 통과 증거로 혼용하지 않는다.

### 구조적 수리 우선순위

| 순서·경계 | 확인된 원인 또는 입력 상태 | 처리·수용 조건 |
| --- | --- | --- |
| P0 · SDK recovery 통합·실행 경계 | SDK의 원 publication/receipt 보존과 Runtime outbox·aggregate durable barrier는 양쪽 local main `0760680d`/`6bab127740a`에 전체 묶음으로 반영했다. SDK `9a47d86a`는 retargeted replay의 CAS를 제출 target이 아닌 검증된 원 publication으로 검사한다 | [Lifecycle ADR](../../../adr/OCT-05-003-active-query-and-runtime-lifecycle.md#replay-cas-preflight-authority-repair)로 SDK130/130·실제 debug daemon4/4와8 crash cuts·strict 및 원 target 복구를 이관. Quanta `9a47d86a`는 원격 게시됐다. Runtime Rust restart/migration test body는 기존 producer 컴파일 실패로 `NOT_RUN`; candidate base `f574fa82`의105 diagnostics는 현재 main 오류 수가 아님. Semantica consumer의 canonical push는 다른 작업의110 선행 커밋 때문에 거부됐으며 remote 게시는 미완료. 선행 owner publication 후 전체 consumer 게시와 Runtime 실제 회귀를 완료; Quanta 게시·daemon 성공을 pair 완료로 판정하지 않음 |
| E4-01 · 수리 후 비용 경계 | 중복 `plan_ops`는 main `4647097d`, 버릴 조회 객체 생성은 main `dfbf9213`에서 수리 완료. Frozen `703d0e68`의 원32,768files matching XL·10 lifecycle phase·독립 replay는 완료했다. Delta40.971s 중6 proof envelope 합계32.609s로 현재 바이트 인증·전체 metadata/inventory 순회·변경 파일의 고정 버킷 재구축은 남는다 | [수리 후 XL 진단](../../../adr/OCT-05-004-cost-capacity-and-qualification-boundaries.md#post-repair-matching-xl-diagnostic)으로 구현·실행·RSS를 이관하며 완료된 capture를 반복하지 않음. Canonical normalization/membership/admission·inventory/pinned identity·변조 거부와 cold query open/scrub의 독립 full proof를 유지. Shared macOS 단회 결과는 정식 speed/RSS 판정이 아님. O(delta) claim에는 실제 저장 불변성 보장과 별도 계약, 정식 performance에는 admitted host·사전 기준·반복 표본이 필요 |
| P1 · I0-03 R5 caller | Frozen producer의 parser retained-projection type/export 및 `first_refusal_v1` 계약이 컴파일되지 않는다. Actual 56 diagnostics/exit101 | 완전한 producer 수리와 clean source가 선행. Consumer feature 제거·오류 무시로 통과시키지 않으며 같은 pair의 caller list/run/restart를 재개. Kernel1/1 성공은 별도 |
| P1 · E1-01/03 review → admission | SQLAlchemy/Tailscale 판단은 unresolved/invalid source line으로 실제 실패. Zellij20은 판단 완료이며 기존 source license/split이 맞는다. 옛 model-cache 경로는 부재하지만 동일 assets는 별도 retained capture에서 복구 가능하다 | 최종 판단과 source-bound 검증을 유지. Zellij license/split/model 입력 재검증은 아래 ADR로 이관하고, 최종 source의 Contract/SDK·host/cache/lockfile·새 suite admission/capture를 연결. 오래된 receipt의 source/hash를 변경하지 않음 |
| 외부 입력 · holdout/operations/performance | 독립 gold/acceptance, OSA24 원 corpus, 설치 대상·observer/rollback, qualified host가 없다 | 각 입력이 존재하는 scope만 재개. Proposed SDK/policy나 fixture를 실제 target/qualification의 대체물로 추가 구현하지 않음 |

두 코드 수리 경계는 병렬이며 shared Cargo/schema/CI 및 무거운 build/native 실행은 직렬 통합한다.
Source-bound admission은 수리된 최종 SDK/producer proof 이후에 발행한다. 미완료 판단과 target 입력은
코드 결함과 구분하며, 완료된 ARB/B09/XL을 반복 실행해 이 선행 조건을 대체하지 않는다.

### 코드·테스트 대조 결과

아래는 현재 구현/테스트 범위와 실제 잔여의 구분이다. 테스트 소스가 있다는 사실은
이번 Rust 실행이나 최신 제품 qualification을 뜻하지 않는다.

| 범위 | 실제 코드·기존 테스트 | 남길 작업 |
| --- | --- | --- |
| E1 review/admission | `holdout_review.py`의 blind prepare·frozen validation·finalize, `run.py`의 실행/replay admission 검증, `corpus_binding.py`의 source split 검사; retained Ready9 AI license9repo/9,741files custody 대조 완료 | 실제 reviewer/adjudicator raw·최종 labels·새 holdout license/gold/acceptance·admission 발행 |
| E1 bootstrap | `evaluator.py::mean_ci`의 10,000 draws·16-key/256KiB bounded cache와 `test_bootstrap_cache.py`의 독립 고정 golden·validation controls | 추가 최적화는 actual paired full-caller 비용·parity를 보고 판정 |
| E2 reader/inventory | `live_lexical_external.py`의 selected-project acquired-reader 검증, `opengrok_query_witness.py`, workflow/matrix/ready-drain | 미실행 required cells·actual replay/join; caller가 요구할 경우에만 전체 loaded-reader witness 구현 |
| E4 F15 / Scale | Fault/query/clone-retry/daemon·fixed5bf Large/XL 및 fixed82bc XL 전수 검증6회 원인 추적; packed planner O(N²) 수리와 typed proof/current-byte authentication 통합·library1,035/통합36/daemon300 통과; e227 및 후속 두 publication 수리를 포함한703d matching XL32,768 실제 실행·독립 replay 완료 | E4-06 admitted Linux 반복 성능. 완료된 capture 재실행 없음 |
| QIT lifecycle/concurrency | Mixed-corpus lifecycle·SDK history8/8 및 Darwin TSan의 core2/lexical2는 [coverage ADR](../../../adr/SEP-27-005-catalog-recovery-supervision-and-proof-custody.md#selected-darwin-tsan-and-contract-execution)로 완료 이관 | 더 넓은 generated/repeat/native inventory. Search-node 한도는 nightly transition 실행이 아님 |
| Semantic | Exact-text semantic/hybrid·OS-process cache·model/revision/dimension matrix4/4와 corrected lib-test strict는 [semantic ADR](../../../adr/MAY-31-001-lancedb-semantic-generation-authority.md#text-vector-and-cache-identity)로 완료 이관 | 기존 OpenAI paraphrase rail(ignored)의 실제 API 입력/실행, installed CLI/live-provider release 및 upstream producer 검증 |
| I0 operations | 공통 typed action producer·pre-state refusal·checker/aggregate, optional paired caller/kernel archive | concrete target adapter·독립 observer 계약 구현, authorized target 입력 및 exact-pair/action 실행 |

완료된 foundation은 [review ADR](../../../adr/OCT-05-001-review-admission-and-result-identity.md#owners-and-regressions),
[semantic ADR](../../../adr/MAY-31-001-lancedb-semantic-generation-authority.md#text-vector-and-cache-identity),
[test coverage ADR](../../../adr/SEP-27-005-catalog-recovery-supervision-and-proof-custody.md#implemented-lifecycle-tests-and-remaining-coverage)가 소유한다.
이 대조에서 실행한 review/bootstrap/OpenGrok 범위는 fixture 검증이며 actual 제품 검색은 아니다.

SDK/Contract `e37123eb`, hosted CI `9221d771`, CS/SG/OG ready9 `09103820`의 완료는
각각 그 소스·입력 범위다. 후속 `a308972a`는 upload/SDK/scale을, `6a3f6afc`는 contract를
변경했다. 이전 결과를 최신 HEAD의 qualification으로 재표기하지 않는다.
새 소스의 CI·선택된 proof 회수는 [I0-02](#o4-i0-02), 실제 제품 수용은 각 잔여 owner가 담당한다.

## Quanta에서 할 작업

| 우선·owner | 실제 잔여 | 종료 조건 |
| --- | --- | --- |
| P0 · I0-02 / CI/integration | 선택된 PR/release·broader inventory; 후속 코드 변경의 영향 rail | 동일 source terminal·inventory·receipt. b9 regular CI 및 fresh release SDK27/original·relocated replay, 0d Contract191Rust/802Python·Darwin TSan4는 ADR로 완료 이관 |
| P1 · E4 / performance | 전체 sync/read/hash/metadata·segment fanout 비용, scanner·Semble·bootstrap 판정 | 원인별 실제 관측 및 독립 parity. 정식 속도는 admitted host·사전 기준·반복 표본 |
| P1 · E1/E2 / quality | labels/admissions·matching Quanta/Semble pair·native replay/full5·독립 채점 | required cells 및 query/unit/source/index scope, 미판단·실패·제외 분모 설명 |
| P1 · E1 / holdout | 실제 미사용 corpus/query/family·license/gold/name-span·typo 평가 | 독립 truth·critical strata·exposure/underfill, file hit와 declaration recovery 구분 |
| P2 · I0-03 / operations | concrete target adapter·provider·installed Linux/state/actions | 대상·독립 pre/post 계약·actual pair/host/config/state/retention/rollback 입력과 실행 |

### 잔여별 실행 가능 조건

| 잔여 | 현재 입력·실행 상태 | 다음 조치 |
| --- | --- | --- |
| E1 judgments/admission/holdout | `BLOCKED`: 151tasks/742pairs의 reviewer 배정·최종 판단0; 새 holdout license/gold/acceptance 부재. Retained Ready9 AI license custody는 완료 | 실제 독립 reviewer/adjudicator raw·해당 승인 입력을 받은 뒤 finalizer/admission/scoring. Blank form을 labels로 채우지 않음 |
| E2 other required native lanes | Gin v3 symbol/file·7개 default robust cohort·explicit typo1,189/독립 OSA1 absence99의 actual pair/replay, 새 B09 8cells, ARB 128captures/384route 공식 채점·replay는 `VERIFIED`; historical B09 current phase33은 `FAILED` | 완료 범위는 [native ADR](../../../adr/OCT-05-002-native-capture-clock-and-index-scope.md)로 이관. 원 source 없는 B09 OSA24는 `BLOCKED`; C3 새 labels/admission 및 source별 required inventory는 별도 |
| E2 Semble A/B adoption | `NOT_RUN`: Bat A/A 반복·Zustand0/1 selected parity 완료 | 대체 구현/모드와 사전 whole-caller 기준·고정 source 입력을 선택한 뒤 실제 A/B |
| E4 formal performance | `BLOCKED`: qualified Linux/host timeline·사전 효과/불확실성 기준 부재 | 대상 host·config·동일 boundary와 사전 paired schedule 입력 |
| E4 Scanner historical replay/performance | 옛 root/binary replay와 admitted-host speed는 `BLOCKED`; 별도 f606 fresh build/capture 두 closed receipts와338 parity는 `VERIFIED` | [Closed 진단](../../../adr/OCT-05-004-cost-capacity-and-qualification-boundaries.md#closed-scanner-build-and-capture-diagnostic)을 유지. 정식 speed/adoption은 별도 host·반복 schedule·사전 기준 필요 |
| QIT/pair/release | 선택된 broader/release는 `NOT_RUN`; e227/fixed77bc caller/list는 producer 컴파일로 `FAILED`, kernel 실제1/1은 `VERIFIED` | b9 fresh SDK27은 완료. Caller 재실행은 완전한 clean producer 수리 후; kernel 결과로 paired/operations 성공을 발행하지 않음 |
| I0 Linux actions/installed provider | `BLOCKED`: target 경로·독립 observer/rollback 계약·설치된 CLI/API grant 부재 | 실제 대상 입력 후 adapter 계약과 action 구현/실행. 현 staged registry로 deploy 완료를 발행하지 않음 |
| Conditional token/regex/bootstrap/policy | 측정·consumer 계약·독립 truth가 선행 | 병목/반례/선택된 정책 없이 새 캐시·API·범용 adapter를 추가하지 않음 |

### 벤치 실행 잔여 — 2026-10-08 소스·입력 대조

- Ready9: 최신151tasks·742pairs union의 독립 판단·final admission이 잔여다.
  Source6a3의5repo, fixedb262의 CLI·Django·Nushell·TypeORM 및 외부091 capture/replay는 완료됐다.
  Source별 raw·완료 범위·허용 reuse는 [Ready9 ADR](../../../adr/OCT-05-002-native-capture-clock-and-index-scope.md#completed-ready9-capture-scope)이 소유한다.
  NL file20tasks/repo·distinct-file 계약이며 bare-symbol workflow와 구분한다.
  b9 fresh release Bat20tasks/79files의 actual Quanta/Semble pair와 byte-identical
  verdict replay도 [진단 ADR](../../../adr/OCT-05-002-native-capture-clock-and-index-scope.md#selected-fresh-release-bat-pair-diagnostic)로 완료 이관했다.
  Quanta capped20/Semble success20이며 정식 품질·속도·다른 required lanes의 완료가 아니다.
  9repo/9,741files와106notice pairs의 retained AI license custody는
  [review ADR](../../../adr/OCT-05-001-review-admission-and-result-identity.md#retained-ready9-license-custody)로 완료 이관했다.
  이 scope를 새 holdout 승인·human review·redistribution clearance로 재표기하지 않는다.
- accepted55/PREP·5796 재발행 packet의 명시된 임시 root는 부재하여 원본 replay는 `BLOCKED`다.
  원본 byte 동일 복구 또는 새 source-bound 준비가 선행한다.
- Historical supplemental742pairs ledger와 historical Scanner A/B 실행 root는 현재 부재해 해당 원본 replay는 `BLOCKED`다.
  현재 source별151tasks·742pairs union과 review 입력은 보존돼 있으며 historical ledger와 다른 입력이다.
  Scanner의 durable Bat79files/338queries 입력·frozen a5 관측 캡처/parity는
  [cost ADR](../../../adr/OCT-05-004-cost-capacity-and-qualification-boundaries.md#observed-scanner-two-arm-diagnostic)로 완료 이관했다.
  별도 f606 두 fresh build/capture closed receipts와 canonical338 parity 비교는
  [closed 진단 ADR](../../../adr/OCT-05-004-cost-capacity-and-qualification-boundaries.md#closed-scanner-build-and-capture-diagnostic)로 완료 이관했다.
  옛 binary replay와 정식 speed/adoption은 [E4-03](#o4-e4-03) 잔여다.
- Scale: fixed5bf의 Large4,096·XL32,768 causal 비용/RSS·lifecycle·독립 replay,
  XL release OS-child restart/delete 및 offered-load3,743건은 완료됐다.
  원래 fixture와 cap을 유지했고 registry resident326,772,711bytes로512MiB 안에 들어왔다.
  Target200QPS는 포화로 달성180.039QPS이며 정식 성능 합격이 아니다.
  Fixed8642 Medium·fixed5c Large 및 소스별 기능/CI checkpoint는
  [cost ADR](../../../adr/OCT-05-004-cost-capacity-and-qualification-boundaries.md#completed-source-bound-checkpoints)이 소유한다.
  이 Mac diagnostic은 Linux physical-I/O·qualified performance를 대신하지 않는다.
  Linux 정식 rail은 macOS에서 unsupported_host로 거절됐으며 admitted host가 필요하다.
- `source-split-prepare/validated.json`은 source split만 검증한다. License/gold/acceptance가 아니다.
  Workflow/matrix/join/decision fixture 통과도 actual product search나 benchmark samples를 대신하지 않는다.

## 실행 순서

| 단계 | 실행·인계 | 선행 |
| --- | --- | --- |
| W0 | source/dirty/owner·claim/input/binary 영향 확인 | shared schema/DTO/registry/CI는 I0 단일 owner |
| W1 | E1 labels/rubric/holdout, E2 index scope, E4 비용·조건부 병목 조사 | 입력 부재는 해당 scope만 `BLOCKED` |
| W2 | 확인된 비용·반례의 owner 수리 및 영향 회귀 | 사전 correctness/resource 계약. 구현된 F15를 다시 만들지 않음 |
| W3 | I0 selected proof/CI → E1 admission ISSUE | PREPARE → 영향 VALIDATE → ISSUE |
| W4 | ready cells native capture/replay/join, matching capacity/A-B | source/input/profile/index/clock에 결속한 raw. 실패 sibling은 ready cell을 막지 않음 |
| W5 | 마지막 unjudged union → labels/admission → scores/CI·정책 판정 | E2 raw 및 E1 독립 truth. Holdout은 기존 ready cohort 재채점의 전역 선행이 아님 |
| W6 | I0 실제 target adapters·provider/Linux/운영 실행 | authorized inputs·independent observers·exact pair·staged registry 수용 |

OG 단독 native capture와 각 clean source의 scanner/capacity 진단은 최종 Quanta SDK proof와
독립 진행할 수 있다. Formal 비교·릴리스가 소비하는 source proof는 해당 scope에서 결속한다.
Source/fixture/static 조사는 병렬 가능하다. 실제 build/test/model/Docker/scale/performance는 host별
직렬 admission을 따른다. 이 문서는 새 병렬 에이전트나 무거운 실행을 등록하지 않는다.

## 공통 실행 조건

- `VERIFIED`는 실행한 범위, `FAILED`는 실행 실패, `BLOCKED`는 필수 입력 부재,
  `NOT_RUN`은 미실행이다. 미측정 조건부 변경을 `NOT_APPLICABLE`로 닫지 않는다.
- 현재 source/dirty/owner·입력·selector를 확인하고 좁은 결정적 rail부터 실행한다.
  새 raw/log/receipt는 checkout 밖 fresh root에 두고 기존 실패/partial를 보존한다.
- source/query/unit/model/profile/runtime/clock 변경은 영향 cells/proof를 재검증한다.
  qrel-only reuse도 원 native binding이 허용해야 한다. 옛 raw의 digest/revision을 바꾸지 않는다.
- unknown/unresolved/missing을 grade0/no-answer/empty success로 채우지 않는다.
  capped/timeout/capacity refusal·common-eligible0은 각각의 outcome으로 남긴다.
- [Config/generation policy](../../../adr/OCT-04-002-configuration-and-generation-policy.md)는
  별도 operator 요구에 따른 `Proposed`다. [Preparation SDK](../../../adr/OCT-04-003-source-preparation-sdk.md)는
  2026-10-08 직접 구현 요청에 따라 격리 후보로 구현했다: typed adapter, text/Markdown,
  caller prior manifest, 완전 source universe의 변경·이동·삭제·no-op 조정,
  `publish_outcome`과 원 receipt 기반 `activate_published`다. 새 wire/daemon registry는 없다.
  게시 후 observation 실패도 `AfterPublish` 증거를 유지하며 Semantica 소비자의 retry barrier를
  함께 변경한다. Candidate 구현과 shared-main·consumer 통합, 실행·release 판정을 구분한다.
  현재 후보의 SDK152·외부공개 API5·계약9와 SDK/searchctl strict·public API baseline은
  `VERIFIED`다. Fresh debug daemon5/5 및8 crash cuts도 `VERIFIED`이며 text/Markdown의
  이동·삭제·재시작 후 manifest 복구·no-op·별도 activation을 포함한다.
  Actual SDK frontdoor21/21(47 filtered)와 ingest preflight2/2(93 filtered)도 `VERIFIED`이며
  provider/release 판정이 아니다.
  Semantica 후보의 게시 후 오류 분류 QBC1/1은 `GREEN`; 두 failure stage를 함께 검사한다.
  Prepared receipt QBC9/9와 exact V5 artifact1/1도 `GREEN`이다.
  Runtime shadow-delta rail은109fd+후보 source에서 parser271 diagnostics/exit101로
  `FAILED`, 테스트 본문은 `NOT_RUN`이다. Restart/migration 및 coupled main·remote 통합은
  별도 경계이며 이 focused 결과를 release 수용으로 승격하지 않는다.

## E1

Owner: review/admission/evaluator/source oracle/split/gold/scoring.
Contract: [OCT-05-001](../../../adr/OCT-05-001-review-admission-and-result-identity.md).

### O4-E1-01

`BLOCKED` · SQLAlchemy 잔여 146pairs 최종 AI 정답 판정 및 Tailscale UDP
필터 밖 grade1/3 rubric 확정. 원본 SQL334/480과 reviewer raw를 보존한다.
2026-10-08 실제 실행과 한 차례 동일 입력 재시도에서도 SQLAlchemy는 unresolved pair/invalid source line,
Tailscale은 unresolved pair로 `FAILED`였다. 두 저장소의 labels/admission은 미발행이다.
원 frozen corpus/suite/query/rubric과 valid raw는
`qi-b08-closeout-20261004-2i72kj91/c3-review-resume-quota-qcshswey`에 있다.
새 실행·실패 원본은 `/private/tmp/qi-c3-oct8-review-v4/`에 별도로 보존한다.
완료 조건: C3 240tasks의 judgment provenance와 issued/excluded/failed/blocked 집합.
미판단 pair는 채점에서 제외하고 model의 미해결 판단을 grade나 human gold로 강제하지 않는다.

Zellij 20tasks/476pairs의 세 모델 판단·독립 cached replay·canonical AI labels/NL suite 발행은 완료했다.
[C3 실제 closeout](../../../adr/OCT-05-002-native-capture-clock-and-index-scope.md#selected-c3-ai-review-and-issuance)이
원본·명령·실패 이력·retained source split 대조를 소유한다. 완료된 판단을 다시 실행하지 않는다.
원 source/query/rubric와 requested model은 유지하며 human provenance·qualified verdict는 없다.

### O4-E1-02

`BLOCKED` · 신규151tasks/742pairs의 두 reviewer+adjudicator 판단 및 원 valid labels 병합.
현재 union의 모든742pairs는 원 query/source에 결속된 두 blank form에 포함돼 있다.
새4repo 입력은 `ready9-native-paged-20261007-01a10d0b/blind-review/`, retained
Lo·Mocha·Uvicorn·Zustand 입력과 전체 대조는
`/Users/songmin/.codex/task-evidence/ready9-retained-review-20261007-01a10d0b/review-input-coverage.json`이다.
8repo·160개 form task가151개 판단 대상 task를 포함하며 reviewer identity/label 변경은0이다.
Bat358+51=409를 재호출하지 않는다. 별도 historical 원 ledger
`/private/tmp/qi-current-nine-supplemental-pool-97eedd-actual-v1/ledger.json`은 현재 부재하다.
기록된 SHA `78013ed33e5647bfa5e109fc5edabb422e41dc48d0cbf4f63f713785642818ec`의 동일 원본 없이는
historical replay를 발행하지 않는다. 현재 union의 review는 준비된 입력으로 진행한다.
제품명/순위/점수는 review에 노출하지 않는다.
완료: source/query/rubric/threshold/model에 결속된 labels·독립 raw replay 및 reused/unresolved/excluded 집합.

### O4-E1-03

`NOT_RUN` · ready9 후속 final-source 및 SQLAlchemy/Zellij/Tailscale admission ISSUE.
Zellij canonical AI labels/NL suite20tasks는 발행됐고 retained split의 commit/universe/20families와 일치한다.
기존 local-source license의422files와 새 suite/corpus/split 결속은 재검증됐다. 옛 HF cache 경로 부재는
동일 revision/asset의 retained Gin20 cache에서 fresh external root로 복구했다. 이 입력 복구는
[C3 ADR](../../../adr/OCT-05-002-native-capture-clock-and-index-scope.md#selected-c3-ai-review-and-issuance)이 소유한다.
새 suite의 full admission·matching final-source Contract/SDK 및 host/cache/lockfile bindings가
미완료여서 admitted native는 `BLOCKED`다. 새 판단이나 license 재승인을 임의로 생성하지 않는다.
SQLAlchemy/Tailscale의 미완료 판단은 [E1-01](#o4-e1-01)이 한 번만 소유한다.
5796 재발행 packet의 임시 경로는 부재해 원본 replay는 `BLOCKED`다. Bat409 merged qrels/suite/pack,
각 repo source/runtime·split/license/review·matching proof를 동일 bytes로 검증한다.
CLI20tasks/519pairs의 historical ISSUE와 중단된 remaining batch를 구분한다.
C5 stale4 suites는 reissue 또는 명시적 exclusion. 완료: 원 labels provenance를 유지한 새
admission/result, wrong-source/threshold/grade/family/unit/runtime/proof/license 거절.

### O4-E1-04

`NOT_RUN` · 다른 supported declaration-name/span·typo cells와 후속 source 영향 평가.
Gin 4개 name-span controls와 canonical v3 symbol1,196·default file1,196의
실제 native 진단은 [ADR](../../../adr/OCT-05-002-native-capture-clock-and-index-scope.md#selected-gin-declaration-and-robustness-execution)로 완료 이관했다.
Driver f606/binaries b9의 scope를 후속 main·holdout으로 승격하지 않는다.
Independent source oracle·native selected unit으로 same-line/receiver/use-only/Unicode/case를 대조한다.
완료: supported/unsupported 분모·source-attested 선언 identity/name bytes/span. File hit는 name recovery가 아니다.

### O4-E1-05

`BLOCKED` · license approver·사전 acceptance/critical-stratum 기준; 독립 gold/holdout 미발행.
`qi-oct4-unseen-prepare-k7exyv41`의 source/release/split candidate를 검토한다.
Corpus-set5,684와 release code_only6,079는 다른 분모이며 candidate12repo는 미승인이다.
완료: source/query/family development/holdout 분리, exposure/near-copy/parser/license/gold provenance와
ambiguous/excluded/underfilled 집합. 1,000+ family 목표를 복제/exposed source로 채우지 않는다.

### O4-E1-06

`NOT_RUN` · final labels/admissions와 E2 native outcomes/union의 독립 재채점·reports.
Lane별 common eligible/operational coverage/repository-cluster CI/pool sensitivity,
name/NL/no-answer·ARB original/adapted·B09 분모를 유지한다.
완료: raw recomputation과 rows/denominators/scores/report 일치 및 모든 required-cell outcome 설명.

### O4-E1-07

`NOT_RUN` · 조건부 bootstrap 추가 최적화 판정. 실제 matching two-capture와1,196-row paired
full-caller cold compute/RSS·사전 목표/memory ceiling이 선행한다. Symbol 단일-route 진단은 paired caller가 아니다.
채택 시10,000 resamples/method/seed/draw/strata를 independent scalar/reference와 대조하고
NaN/Inf·duplicate task·draw 순서·hidden cache growth를 거절한다. 완료: 최적화+parity 또는 근거 있는 no-code 판정.

## E2

Owner: native external collector/index scope·Semble phases·required cells.
Contract: [OCT-05-002](../../../adr/OCT-05-002-native-capture-clock-and-index-scope.md).

### O4-E2-02

`NOT_RUN` · 남은 repository/profile 및 caller가 요구하는 actual reader/source/posting scope 판정.
OG ready9/180의 selected-project acquired-reader 증거는 그 요청 범위의 완료다.
All-project/global/loaded-reader flags는 false다. 전체 서비스 scope를 요구할 때만 별도
loaded-reader witness/consumer를 구현·검증한다. Disk/API/readonly seal·returned hits로 전체 indexed universe를 추정하지 않는다.
완료: scope별 missing/extra/unknown, 실제 service/query/index 결속과 독립 source/directory/settings 분모.

### O4-E2-03

Ready99repo의 terminal inventory는 완료됐다. 잔여는 다른 required inventory의
executed/reused/unsupported/failed/blocked/not_run 설명 및 ready drain이다.
Gin v3 native 및7개 default cohort의 실행/독립 replay는 완료됐다.
B09 원33cells/11,272selected rows의 original commitments 및 diagnostics는 대조 완료다.
Current phase replay33은 producer 증거 부재로 `FAILED`이며 원 완료와 분리한다.
재캡처 입력 대조에서 CLARC2/CSN6 cells의 1,350selected tasks·3,798source files는
원 manifest bytes와 대조돼 준비됐다. OSA24 cells의 원 checkout은 없어 `BLOCKED`다.
준비8 cells의 actual native는 f606에서8/8 exit0·phase/diagnostic/source 및 독립 score replay를 완료했다.
원 successful33 결과와 분리한다. 원본과 재실행은 [ADR](../../../adr/OCT-05-002-native-capture-clock-and-index-scope.md#retained-b09-scope-reconciliation)이 소유한다.
원 source와 qrel-only reuse를 구분한다. 살아 있는 process/malformed/wrong-repo terminal/
missing/output 경합은 success가 아니다. 완료: 누락 없는 terminal/input-byte inventory와 실패 sibling에 독립적인 실행.

### O4-E2-04

Ready9 capture/replay/projection과 원 source별 완료는
[ADR](../../../adr/OCT-05-002-native-capture-clock-and-index-scope.md#completed-ready9-capture-scope)가 소유한다.
실제151tasks/742pairs 판단·admission 잔여는 [E1-02](#o4-e1-02)/[E1-03](#o4-e1-03)에만 둔다.
Gin v3 exact symbol/file1,196 및 prefix/infix/components/default typo/declaration absence/
content absence/typo content absence7개 cohort와 explicit typo1,189/독립 OSA1 absence99는
[ADR](../../../adr/OCT-05-002-native-capture-clock-and-index-scope.md#selected-gin-declaration-and-robustness-execution)로 완료 이관했다.
Gin20의 original manifest bytes 복구·f606 native hybrid pair20·독립 byte-identical
verdict replay는 [진단 ADR](../../../adr/OCT-05-002-native-capture-clock-and-index-scope.md#selected-retained-gin20-hybrid-pair)로 완료 이관했다.
잔여 selected lanes는 C3 NL240 및 원 source 부재 등 후속 조건을 요구하는 B09 cells다.
ARB current32-token policy의 original13/88·retained adapter-v1 27/88·adapter-v2 88/88은
actual128 captures와384route 공식 채점·독립 replay를 완료했다. Historical original17과 current13을 구분한다.
새 read-only ARB scorer는 official raw sample/gold·base/universe·spec·query·record를 결속하며
top100chunks→20distinct files의 공식 Recall/MRR를 계산한다. 공식 archive/manifest와 descriptor-bound
parsed bytes·transitive clean official source/import origin을 검증하는 BCY extension까지 통합했다.
Focused44와 actual128 prepared-spec binding preflight는 `VERIFIED`; official3archives의
83release-chunk files/57unique snapshots도 복구·rehash했다. Native128/128 source-bound 독립 검증과
공식3arm Recall/MRR·BCY4k/8k/16k/32k 및 반복 score bytes가 검증됐다.
원 controller의 manifest/typed-context 오류와 e227 scorer의 combined/projected pack 거부는 보존했다.
6c5cda3c의 canonical projection 수리·정확한3route 및 digest 대체 거절 회귀까지45/45 통과했다.
[공식 진단](../../../adr/OCT-05-002-native-capture-clock-and-index-scope.md#arb-current-policy-and-official-file-scoring)이
source/분모/원본 경로를 소유한다. Current-v2 Recall@20 lexical0.484848/semantic0.308712/hybrid0.369318이며
cohort13/27/88 차이와 기존 노출 입력 때문에 paired improvement/SOTA holdout으로 발행하지 않는다.
미실행·refusal·failure를 zero score로 채우지 않으며 official corpus custody가 없는 BCY는 `BLOCKED`다.
원 B09 OSA/CLARC/CSN 캡처33개를 전부 미실행으로 재표기하지 않는다.
현재 decoder의 phase refusal은 [원 scope 대조](../../../adr/OCT-05-002-native-capture-clock-and-index-scope.md#retained-b09-scope-reconciliation)를 따른다.
Four typo populations1,192/1,178/1,192/1,192와 ARB original/adapted 분모를 합산하지 않는다.
[E2-03](#o4-e2-03)이 required outcomes·original raw/input/source/unit/clock replay를 한 번만 소유한다.

### O4-E2-05

`NOT_RUN` · 대체 구현/모드의 Semble 반복 A-B 및 재사용 채택 판정.
Bat 전체20tasks/79files·warmup1·3 measured repetitions의 두 A/A 실제 실행과
parent/worker 비용·rows/status/score-bit parity는 [native ADR](../../../adr/OCT-05-002-native-capture-clock-and-index-scope.md#selected-semble-repetition-and-warmup-diagnostic)로 완료 이관했다.
영향 source5개 SHA는 일치하지만 실행별 global HEAD는 관측하지 않았으므로 고정 HEAD qualification이 아니다.
Query pack의 초기 digest·query hash·manifest universe 결속과 실행 후 drift 거절은
[입력 계약](../../../adr/OCT-05-002-native-capture-clock-and-index-scope.md#semble-admitted-input-identity)으로 완료 이관했다.
Immutable validation·native rows/status parity 및 unattributed residual을 확인한다.
정식 speed는 [E4-06](#o4-e4-06)의 host/boundary/schedule을 따른다.

### O4-E2-06

Zustand 전체20tasks/50files·동일 cold probe/seed/3 schedules의 실제0/1 parity는
[native ADR](../../../adr/OCT-05-002-native-capture-clock-and-index-scope.md#selected-semble-repetition-and-warmup-diagnostic)로 완료 이관했다.
`NOT_RUN` · 다른 repo에서 quality warmup0을 선택할 때의0/1 parity 및 고정 HEAD 조건.
같은 task set/cold probe/profile/seed/repetitions와 protocol SHA/schedule/phase ledger,
task별 rows/status/score bits를 두 actual run에서 대조한다. 그 전에는1을 유지한다.
Order-sensitive 차이가 있으면0을 채택하지 않으며 speed는 warmup≥1이다.

## E4

Owner: lexical lifecycle/query cost·scanner/scale/load·policy RCA.
Contract: [OCT-05-004](../../../adr/OCT-05-004-cost-capacity-and-qualification-boundaries.md).

### O4-E4-01

`NOT_RUN` · matching release의 full/delta/delete/no-op/reopen 전체 비용 qualification.
Native segment 재사용·live-BM25·F15 changed-bucket publication은 구현됐다.
같은 timed seal 응답의 request/source/receipt에 결속한 단계 관측 보존·검증도
[cost ADR](../../../adr/OCT-05-004-cost-capacity-and-qualification-boundaries.md#per-seal-ingest-observation)에 반영했다.
Pre-intent/base proof·lexical proof·semantic commitment·source finalization의
opt-in stderr trace도 전체 묶음으로 반영했다. 중첩 span은 exclusive 합산 비용이 아니다.
Scale owner 회귀 44/44와 영향4package strict Clippy는 `VERIFIED`.
Fixed82bc XL attribution은 [전수 검증 추적](../../../adr/OCT-05-004-cost-capacity-and-qualification-boundaries.md#xl-repeated-validation-trace)으로 완료 이관했다.
단일3,437byte delta90.61s 중 전체 proof6회75.64s, no-op91.64s다. Source/build가 다른 원93.969s와 속도 비교하지 않는다.
Packed-source O(N²) 조회는 이분검색으로 수리했고 planner4/통합33/strict Clippy는 `VERIFIED`다.
주병목 수리는 통합됐다: 발행 범위의 identity/manifest/policy에 결속한 typed proof를 유지하고,
각 기존 refusal 경계에서 현재 bytes를 다시 인증하며 decode/normalization/posting 결과를 재사용한다.
완전한 묶음의 library1,035passed/1ignored·파일 변경 통합36passed·daemon300passed/10skipped와
strict Clippy/format/hexagonal/module 검사는 `VERIFIED`; main 파일은 검증된 bytes와 같다.
e227 matching XL32,768 실제 실행은352.747s exit0이며 canonical profile 독립 replay가 일치한다.
원82bc와 corpus/seed/tier manifest/runtime config·기본 cap이 같다. Delta42.517s/no-op45.034s/delete43.148s를
관측했다. Delta proof6회32.596s 중 cold 독립 walk2회12.585/12.393s와 반복 인증4회1.757–2.298s는
남는다. 공유Mac 단일 profiled 진단이며 qualified speed가 아니다. 성공 bool/mtime 재사용으로
변조 거부를 우회하지 않는다.
Remaining: cold 전수 검증·metadata/custody 읽기·hash/fold, delete-mask O(max_doc), correction/
NoMerge segment fanout, transient peak 및 foreground/maintenance CPU/read/write/fsync 분해.
반복 file plan은 main `4647097d`에서 publication-owner metadata custody로 수리했다.
현재 root/manifest bytes·operation/policy binding과 staged-source admission을 매번 확인하며
writer가 실제 target의 plan을 한 번 소비한다. 최종 plan31/파일 변경36·strict Clippy는
`VERIFIED`; library 직렬368passed/1ignored도 확인했다. 첫 병렬 실행의 writer-lock 실패는
보존한다. 새 daemon 및 matching XL은 `NOT_RUN`이며 과거42.517s를 새 수리의 수치로 쓰지 않는다.
완료와 남은 비용 경계는 [계획 custody 경계](../../../adr/OCT-05-004-cost-capacity-and-qualification-boundaries.md#publication-plan-custody-and-remaining-structural-cost)가 소유한다.
Publication-only output은 main `dfbf9213`에 통합했다. 같은 canonical source/normalization/
membership/page 검증을 수행하며 조회용 source와 posting directory를 보관하지 않고,
typed publication proof는 query handle로 전환할 수 없다. Lexical373passed/1ignored·F15통합3·
파일 변경통합36 및 strict Clippy는 `VERIFIED`. 완료·손상/정책 대조 회귀와 정확한 명령은
[publication output 경계](../../../adr/OCT-05-004-cost-capacity-and-qualification-boundaries.md#publication-only-verification-output)가 소유한다.
Causal producer의 source/binary와 independent markers를 결속한다. Seal retention gauge는 unique-inode
regular-file st_size이며 restart gauge0·st_blocks·physical I/O/true peak와 구분한다.
완료: 원인별 exclusive 비용·명시적 clock/resource domain·fresh rebuild score/page parity.

### O4-E4-03

`BLOCKED` · historical Scanner A-B original `/private/tmp/qis.utp62qk5`와 옛 binary가
부재하여 그 실행의 원본 replay는 불가능하다. 기록된 SHA나 과거 수치로 입력을 재구성하지 않는다.
복구된 Bat79files/338queries 입력과 frozen a5의 두 실제 캡처·관측 parity는
[cost ADR](../../../adr/OCT-05-004-cost-capacity-and-qualification-boundaries.md#observed-scanner-two-arm-diagnostic)가 소유한다.

별도 f606 두 fresh build/capture closed receipts와 canonical
`query_timing_overhead.py --scanner-ab`338 parity 검증은
[closed 진단 ADR](../../../adr/OCT-05-004-cost-capacity-and-qualification-boundaries.md#closed-scanner-build-and-capture-diagnostic)로 완료 이관했다.
옛 a5 관측 비교는 중단 target의 normal resume를 포함하므로 그 source의 closed receipt로 승격하지 않는다.
두 진단을 최신 main의 성능 검증으로 재표기하지 않는다.
정식 speed/adoption은 [E4-06](#o4-e4-06)의 host·반복 입력과 사전 whole-caller
keep/modify/withdraw 기준이 필요하다. Child 개선은 whole-call 악화를 상쇄하지 않는다.

### O4-E4-04

`NOT_RUN` · 조건부 persistent token authority. E4-01/03의 actual after-scanner full-caller에서
repeated-scan 병목과 memory/build tradeoff가 충족될 때만 채택한다.
Exhaustive tokenizer/OSA1/source/grammar/folded name witness와 delta/delete/no-op/reopen,
cold-open/build/residency/cap/cancel 독립 수용이 필요하다.

### O4-E4-05

완료 이관 · fixed5bf matching release의 Large4,096·XL32,768 causal/replay,
XL open-loop 및 release daemon의 실제 OS restart/delete 진단·기능 scope는
[cost ADR](../../../adr/OCT-05-004-cost-capacity-and-qualification-boundaries.md#completed-source-bound-checkpoints)가 소유한다.
원래 fixture·cap을 유지한 이 완료 실행은 잔여가 아니다. 정식 반복 성능은 [E4-06](#o4-e4-06)에 남긴다.
Text paging·source Arc sharing·absolute query deadline과 lexical418·daemon300·strict 검증도
같은 ADR로 이관했다. 선택들은 중복 집계하지 않는다.
Medium OS-child restart·offered-load와 새 runtime/profile 영향 검증은 이 완료 범위와 구분한다.
Frozen `492d2fdc`의 XL 기능 proof는 `VERIFIED`이며 기존e371 runtime/open-loop도 완료다.
이는 최신 F15/RSS source의 측정을 대체하지 않으며 완료된 기능 proof를 미실행으로 재표기하지 않는다.
Scale-supported-v1은 pair1GiB/total2GiB·client600s·source128MiB/100,000records·vector256MiB·
staged body512MiB·process4GiB의 별도 계약이다. SDK30s·ordinary inline cap·이전 history default와 구분한다.
Frozen5796 default timeout/posting-cap 실패를 후속 override 성공으로 재표기하지 않는다.
완료: independent corpus/result/count oracle, over-limit typed refusal, offered/served/errors/timeouts/drops
reconciliation·retained bytes·live/RSS·elapsed. Fixture 축소/cap 상향만으로 요청 gate를 닫지 않는다.

### O4-E4-06

`BLOCKED` · Darwin frequency 등 qualified-host 입력; 정식 반복 실행은 `NOT_RUN`.
Continuous load/frequency/thermal/power/disk timeline·사전 effect/uncertainty·same completed-output boundary,
exact source/binary/input/config/topology를 확보한다. B07 최소5fresh roots/route별1,000warm observations,
warmup≥1·randomized paired schedule·독립 schedule/source/raw replay가 필요하다.
Host probe1회·shared-host/phase/scale 진단으로 qualified performance를 발행하지 않는다.

### O4-E4-07

`NOT_RUN` · 조건부 검색 정책 변경. Independent qrel/span/unused holdout 이후 Default OSA23·Gin4·
NL/semantic residuals를 candidate/contribution/rank unit/budget/cap/source/model/generation으로 추적한다.
Confirmed defect/accepted policy/label ambiguity/unsupported/qualification gap을 구분하고 같은 qrel의
독립 ablation을 수행한다. Explicit OSA1은 default 성공이 아니다. 채택 시 critical strata/no-answer/ambiguity 사전 기준을 충족한다.

## I0

Owner: shared contracts/DTOs/registry/CI/dependency·source impact/release.
Contract: [OCT-05-004](../../../adr/OCT-05-004-cost-capacity-and-qualification-boundaries.md).

### O4-I0-02

선택된 PR/release·broader inventory와 후속 코드 변경의 영향 rail이 잔여다.
b9 fresh release SDK27/27 및 original/relocated portable replay는
[coverage ADR](../../../adr/SEP-27-005-catalog-recovery-supervision-and-proof-custody.md#selected-fresh-release-sdk-execution)로 완료 이관했다.
0d Contract191Rust/802Python 및 original/relocated portable replay는
[coverage ADR](../../../adr/SEP-27-005-catalog-recovery-supervision-and-proof-custody.md#selected-darwin-tsan-and-contract-execution)로 완료 이관했다.
Source0d의 regular CI6jobs 및 Rust4,281passed/0failed/30ignored·Python4,323passed/30skipped는
실제 terminal·inventory·receipt·모든 job source SHA 대조로 완료됐고 아래 ADR로 이관했다.
Bench 성공은 컴파일이다. 이후 docs-only checkpoint를0d CI source로 재표기하지 않는다.
b9의 원 final verify2275는 executor 시작 전 infrastructure_fail이며 원 workflow는 failed로 보존됐다.
실패 final만 재시도한 verify2277은 실제 checkout/source command exit0이고 기존5workers를 상속해
regular CI가 완료됐다. Rust4,281/Python4,323은 원 worker 횟수이며 재시도에서 중복 실행되지 않았다.
후속 코드 변경은 영향 hosted rail을 회수한다.
코드 `6c5cda3`의 regular CI 6jobs는 동일 source에서 모두 `VERIFIED`다.
Rust 4,299selected/executed/passed·0failed·30ignored inventory와 Python 4,410passed/35skipped를
원 receipt/raw/inventory 및 job source로 대조했다. 이후 문서 전용 커밋은 별도 focused lint 범위다.
완료된 regular main CI/F15·query/restart·cache focused/strict·SDK/runtime checkpoint는
[ADR](../../../adr/OCT-05-004-cost-capacity-and-qualification-boundaries.md#completed-source-bound-checkpoints)와
[cache 계약](../../../adr/MAY-31-001-lancedb-semantic-generation-authority.md#text-vector-and-cache-identity)이 소유한다.
공개 SDK/contract·wire/state/query 변경은 executable authority의 영향 rail과 portable replay를 실행한다.
Runtime autotests=false의 실제 suite/selector를 사용한다. Auxiliary PR coverage/main,
compiler/focused/local/full/release를 구분하고 과거 결과를 최신 source로 재표기하지 않는다.

### O4-I0-03

구현 잔여 · concrete deploy/activate/restore-forward adapters·independent pre/post contract.
실제 Linux host/config/state/retention/rollback 입력은 `BLOCKED`이며 dependent action은 미실행이다.
공통 typed producer/parser/checker/recipes는 완료됐다.
2026-10-08 격리 후보의 operational contract는 root/경로 alias 및 binary·config·state의
ancestor overlap을 실행 전 거부한다. `uv run --frozen --extra dev python -m pytest -q
tools/ci/tests/test_proof_operational_result.py`는61 passed로 `VERIFIED`다. 경로 문자열 검증은
실제 target filesystem의 symlink authority·independent observer·배포/복원 실행 증거가 아니다.
별도 exact-pair caller는 producer 컴파일 `FAILED`, kernel1/1은 `VERIFIED`; paired/operational acceptance는 `BLOCKED`다.
Runner-candidate-only는 operational receipt가 아니다.
2026-10-08 최초 Semantica 확인 시 `77bc829ad70bd32b9c22f92c849a9d6a2aa6b9da`에서
1,286dirty paths를 확인했다. R5 입력으로 고정한 같은 HEAD의 clean
`.codex-semantica-oct8-scc-proof` checkout도 존재한다. 이후 움직이는 Semantica main과
이 exact-pair 입력을 동일 source로 취급하지 않는다.
실제 Cargo runtime/kernel resolver는 이 Quanta checkout의 contract/IPC/SDK로 해석됐다.
QBC metadata exit0의 child stdout은 owner 로그에 보존되지만 터미널 stdout은 비어 있어
기존 Quanta pipe/check_output 연동은 `FAILED`다. 같은 invocation의 nonce/run/status/원본 bytes를
확인하는 metadata adapter와 default typed Nextest archive로 caller 연결부를 수리했다.
추가 foreign-cwd 회귀에서 실제 Python namespace 충돌을 발견해 Quanta `tools`
package 소유권을 명시했다. 수정 후 출력 bridge·locator·foreign-cwd 회귀 60/60은
`VERIFIED`. Clean Quanta b2599와 fixed77bc producer의 실제 runtime/kernel
resolver preflight도 `VERIFIED`다. 두 profile 모두 같은 Quanta7개 reachable package와
producer nested Cargo.lock에 결속됐으며 원본은 `/private/tmp/qna7vj2zq6/r5-clean-{runtime,kernel}-resolved.json`이다.
b2599 원격 Python CI는 Linux에 없는 `/private/tmp` 기본 경로로 `FAILED`였다.
main48fd의 수리는 OS 임시 경로 정규화·3파일/4경계 변조 거절·typed runner 및 observed harness
호출 경로 대조를 포함한다. Focused128과 full 실패 이후 source-contract tail276은 `VERIFIED`다.
Local full 실행의4,058passed/1failed는 수리 전 결과로 보존하며 최신 full-suite 성공으로 합산하지 않는다.
48fd 원격 Python1576은 동일 source에서4,355passed/30skipped·Ruff check/406-file format으로
`VERIFIED`다. 별도 P00 selection386은 중첩하며 합산하지 않는다. 후속 tooling 수정과 Rust CI는 별도 scope다.
48fd Rust1578의 receipt/raw/inventory 및 source를 대조해4,286selected/executed/passed,
0failed·30ignored inventory를 확인했다. Bench1577/final verify를 포함한48fd regular6contexts는
같은 source에서 성공했다. 후속 c38 regular6contexts도 성공했고 원 Rust1584 receipt/raw/inventory는
4,290selected/executed/passed·0failed·30ignored, Python1582는4,355passed·30skipped다.
Bench는 실제 benchmark 실행이 아닌 compilation이며 이 결과를 후속 tooling source로 재표기하지 않는다.
QBC metadata completion probe는 별도로 `verification source changed during QBC execution`로
`FAILED`다. 원인은 미확정이며 실제 Nextest 실행 실패로 재표기하지 않는다.
Clean Quanta c38/fixed77bc의 첫 actual paired recipe는 새 all-feature release build를13m08s에
완료한 뒤 dependency_lock 경로 기준을 nested workspace에 중복 적용해 `FAILED`였다.
실제 caller/kernel Nextest list/run은 시작하지 않았다. 원 controller log·fresh daemon을
`/private/tmp/qna7vj2zq6/r5-c38-failure.json`과 연결해 보존했다. 검증기는 producer와 같은
paired-checkout 상대 경로를 사용하도록 수리했고, 동일 file guard를 expensive build 전에 실행한다.
Actual resolver 출력의 현재 manifest/lock 대조와 producer→runner 경로 roundtrip·pre-build refusal·custody
focused133은 `VERIFIED`다. 후속 clean9c0/fixed77bc actual recipe는 fresh build를2m29s에
완료한 뒤 host Xcode Python3.9에서 QBC `tomllib` import가 실패했다. 원 controller·daemon은
`/private/tmp/qna7vj2zq6/r5-9c0-failure.json`에 결속해 보존했다. 실제 Nextest list/run은 미시작이다.
현재 shell은 producer의 canonical `python-env.sh` resolver를 모든 Python helper에 사용하며,
QBC module origin/import를 expensive build 전에 검사한다. Actual Python3.12.12로 frozen77bc
QBC import·manifest/lock preflight와 focused133은 `VERIFIED`다. 이후 실제 실행은 아래 source별로 분리한다.
Clean2df/fixed77bc의 세 번째 actual recipe는 fresh release build를5m28s에 완료한 뒤
caller/list의 raw feature composition이 QBC의 미등록 compile-surface admission에서 `FAILED`였다.
실제 caller/kernel assertions는 미실행이며 원 controller·daemon bytes는
`/private/tmp/qna7vj2zq6/r5-2df-failure.json`에 결속했다. 수리된 fixed R5 owner recipe는
caller/kernel 각각의 정확한 list/run4 shapes만 QBC typed command-model admission으로 실행한다.
Feature/target/selector/phase drift는 실행 전에 거절하고 lane/source/immutable receipt custody는
기존 QBC owner가 유지한다. Focused137 및 실제 frozen QBC admission4 shapes의 model 대조는
`VERIFIED`; actual Nextest와 운영 qualification으로 승격하지 않는다.
Clean86b/fixed77bc actual4는 fresh daemon3m17s 후 caller/list completion 단계에서
예상 source digest의 explicit completion environment 누락으로 실패했다. 두 source는 clean/HEAD
일치였으며 assertions/kernel은 `NOT_RUN`다. 원 controller·daemon은
`/private/tmp/qna7vj2zq6/r5-86b-failure.json`에 보존했다. Canonical source digest를 compile admission과
completion 양쪽에 전달하도록 수리했으며 producer equality guard는 유지한다.
Hosted2df Python1594는4,388passed/33skipped 후 외부 helper ShellCheck SC1091에서 실패했다.
그 context는 `FAILED`로 보존하고 supplied producer-source annotation으로 hook을 수리했다.
Clean e227/fixed77bc actual5는 fresh daemon2m45s 후 caller/list의 실제 Cargo 컴파일에서
`quanta-adapters-parser`56 diagnostics·exit101로 `FAILED`다. QBC의 explicit completion와
immutable receipt publication은 완료했으며 caller assertions는 `NOT_RUN`이다.
원본은 `/private/tmp/qna7vj2zq6/r5-e227-failure.json`과 `r5-clean-pair-v5/`에 보존했다.
Sibling kernel을 별도 실제 list/run하여 exact-full-bundle/transition-v2 receipt 테스트1/1을
통과했다(`/private/tmp/qna7vj2zq6/r5-kernel-e227/result.json`). Kernel 성공을 caller/pair/operations로
승격하지 않는다. 이후 Semantica main f574의 committed parser subtree는 fixed77bc와 같고
다수 dirty 변경이 있어 clean repaired producer bundle은 아직 없다. Caller 재실행은
그 완전한 producer 수리 후에만 수행한다. Resolution preflight로 이 테스트를 대체하지 않는다.
외부 작업을 reset/stage하거나 dirty pair를 qualified로 발행하지 않는다.
외부 Semantica R3 fixed partition/omission oracle은 `6f5dc8fc13f`에 구현했다.
현재 Runtime compile 실패로 실행 수용은 남았다. 최신 경로·결과는
[SDK 재감사](#sdk-구현-후보-통합-재감사--2026-10-08)가 소유한다.
기존 producer의 prior-state binding·plan assembly와 독립 expected-partition 검증을 구분한다.
고정77bc source의 `load_prior_shadow_semantic_state_v1`는 이전 lexical batch가 delta이면
누적 semantic 상태의 복원을 거부한다. 이 과거 source 관찰은 새 Runtime 검증 결과가 아니다.
다음은 격리 native recovery의 **과거 실행 순서**다. Current-main 오류 수나 최신 소스
수리 목록으로 재사용하지 않는다. 최신 fresh pair 결과는 위 재감사에 기록했다.
2026-10-08 Semantica109fd 후보는 기존 prepared artifact V5에 canonical current semantic-owner
snapshot을 넣고, 정확한 직전 completed aggregate·Lexical member·terminal receipt를 통해
다음 delta의 prior 상태를 읽는다. 새 durable protocol이나 누적 batch-history 목록을 만들지 않는다.
Snapshot은 repo/revision/base/current generation·manifest/batch digest·source event를 검사하고
V3/V4 predecessor의 missing state를 추측하지 않는다. G1 payload 제거 후 G3 계획, untouched
owner·rename/delete·surface clear·V5 canonical serde/restart·독립 partition oracle 테스트를 추가했다.
기존 prepared receipt QBC9/9 및 exact V5 artifact snapshot bytes/closed-serde/reference 검증1/1은
`GREEN`이다. Actual Runtime shadow-delta QBC는109fd+후보의 parser271 diagnostics로
`FAILED`; 테스트 본문은 `NOT_RUN`이다. 이 수치는 current shared main의 오류 수가 아니다.
Missing nested modules·native admission/borrow/port contracts의 complete caller repair를 진행하며,
Kernel 오류 분류1/1 성공을 G1→G2→G3 동작 수용으로 승격하지 않는다.
추가 adversarial review는 replacement 본문 일치, tombstone/clear 이후 부재,
ReplaceGeneration의 정확한 scope 집합과 owner identity/digest 검증을 보강했다.
Owner kind가 다른 동일 ID와 ClusterCard membership의 재개방·변조 거부 회귀도 추가했다.
이 추가 Runtime 테스트는 아직 `NOT_RUN`이다. 실제 completed V5 aggregate publication 이후
G1 artifact prune→restart→G3 경로도 별도 실행이 필요하다. Planner 재개방이나
`aggregate_publication_v2: None` fixture는 그 증거가 아니다.
Parser source는 다른 owner의 native allocation 계약 migration과 연결되어,
1,726개 외부 차이 파일을 frozen proof 입력으로만 격리했다. 이 파일은 우리 변경으로 stage/
게시하지 않는다. 모듈 경로 검사와 영향 boundary preflight6정책은 `VERIFIED`다.
후속 Runtime owner 실행은 `--locked` 입력 불일치로 `FAILED`, 실행된 테스트는0개다.
Lock 복구는 registry package/version을 유지하고 path package7개의 dependency 목록만
후보 source에 맞췄다. 다음 actual Runtime QBC는 외부
`quanta-analysis-interproc-contract` 컴파일91 diagnostics로 `FAILED`, 테스트 본문은0개다.
이는 captured 입력의 결과다. Baseline 파일까지 포함한 전체 비교에서 이후 owner 수정은
4개 파일·기존9개 진단 위치에 대응한다. 나머지82개 진단 위치는 main과 같지만,
82는 재컴파일 오류 수가 아니다. Current shared-main의 오류 수로 재표기하지 않는다.
후속 격리 검증은 최신 interproc/PTA owner35파일을 갱신하고 해당 crate 컴파일을 통과했다.
이어 Java hierarchy 오류 transfer의 source admission 인자 누락1건으로 exit101이었다.
격리 수리 후 다음 실행은 Java literal iterator lifetime1건으로 exit101이었다.
두 실행 모두 테스트 본문0개이며 current shared-main 판정이 아니다. Java의 두 국소 수리는
별도 외부 검증 입력으로 남기며 SDK/V5 변경에 섞어 게시하지 않는다.
최신 외부 입력 read-only 대조는 목록58경로 외 referenced child8개 부재·2개 차이도 확인했다.
초기 조회에서 없던 `canonical_verified_operations_v3.rs`는 이후 owner가 생성했다.
두 Java 수리 이후 Runtime QBC는 외부 `quanta-runtime-kernel-contract` 컴파일19오류,
exit101/테스트 본문0개로 `FAILED`다. 이 역시 captured 후보의 결과이며 current-main 오류 수가
아니다. 최신283 dependency crate roots의 source 대조는 결합 가능 차이217개와 기존 격리
parser/PTA proof repair와의 충돌50개를 찾았다. 이 추가 capture는 적용하지 않았다.
두 lockfile은 SDK의 `ciborium`/`sha2` edge와 새 foreign edge를 결합해야 한다. 충돌 없는
일부만 가져오거나 legacy getter/Clone/current loan 우회로 통과시키지 않는다. 완결된
canonical foreign owner bundle을 결합한 뒤 동일 feature 계약으로 Runtime 테스트를 다시 실행한다.
Root/nested 두 `Cargo.lock` 모두 SDK의 기존 `ciborium`/`sha2` version을 재사용한다. 초기 root lock
거부 및 V5 match tuple compile 실패를 보존한다. Contract lib(test)의 native corpus import/helper
visibility 오류도 실제 관측 후 test-only owner에서 수리했으며, 수리 후 위 세 QBC selection이 성공했다.
초기 Semantica boundary build는236 PASS/2 FAIL로 `FAILED`였으며, 두 실패는 이번 수정 밖의
executor/parser authority와 response receipt posture 경로다. Foreign 입력을 포함한 후속 full build는
232 PASS/6 FAIL이다. 하나는 fixture ignore 속성 편집 중 source 변경으로 verdict가 거부됐다.
Canonical initializer3개 소비자는 공통 constant로 수리했고 해당 guard는 `VERIFIED`다.
편집 동결 후 canonical repo-identity guard도 재실행해 `VERIFIED`를 확인했다.
나머지 unowned4개 실패는 analysis-port foundation manifest, executor shell extraction,
native carrier derive contract, response receipt posture다. 이전 preflight7/7, 최신 parser6정책
preflight 및 initializer/python-min/identity focused PASS를 full/release gate로 승격하지 않는다.
Parser gate closeout의 default language substrate 및 기존 inventory/source-corpora 거부는 별도
전역 gate 잔여다. Consumer feature 제거·policy 완화·Quanta full rebuild로 우회하지 않는다.
최종 결합 감사는 frozen aggregate replay의 visibility 오류도 찾았다. SDK가 원본 A를
복구·활성화한 뒤 consumer가 제출 대상 B와 receipt digest를 비교하면 활성화 후 Terminal이
될 수 있다. 후보는 `publish_outcome`의 원본 binding과 frozen 제출 binding을 CAS 전에
비교하며 불일치 시 원본 `AfterPublish` 증거를 보존하고 activation을 호출하지 않는다.
동일 identity replay의 정상 활성화도 유지한다. Kernel owner `index_sdk_ingress::publish`
QBC는 finished owner receipt `GREEN`:28passed/81filtered였고 신규 회귀2개를 실제 실행했다.
이는 activation adapter 호출 전 owner 결정의 검증이며 실제 control IPC나 전체 Runtime
aggregate process 검증은 아니다. Pre-fix runtime 재현은 `NOT_RUN`, 결함 근거는 reachable
source 경로였다.
Runtime compile 복구 후 다음 durable barrier를 `quanta-runtime`, `test/lib`, `exact`,
`feature-isolation:quanta-runtime.no-default.9c3270892708`로 각각 실행한다. 아직 `NOT_RUN`이다.

- `retrieval::port_impls::index_projection_writer::tests::source_bound_v9_preparing_with_durable_v3_ref_advances_after_restart_v1`
- `retrieval::port_impls::index_projection_writer::tests::prepared_artifact_existing_final_retry_syncs_directory_before_receipt_v2`
- `retrieval::port_impls::index_projection_writer::tests::published_token_custody_survives_writer_drop_gc_and_pending_replay_v2`
- `retrieval::port_impls::index_projection_writer::tests::source_bound_v9_cas_rejects_foreign_published_manifest_after_ready_v1`
- `indexing_machine_v2::store::pending_aggregate_replay_v2::tests::v16_migration_preserves_active_reference_and_installs_releasing_state_v2`
- `indexing_machine_v2::store::pending_aggregate_replay_v2::tests::v16_migration_rejects_ambiguous_prepared_reference_edge_v2`

실제 aggregate G1/G2 완료·G1 artifact 삭제·SQLite/새 reader 재개·SearchPlane 재시작·G3
publication fixture도 구현했다. 정확한 selector와 daemon BIN/SHA 및 ignored-policy는
[SDK remaining integration](../../../adr/OCT-04-003-source-preparation-sdk.md#remaining-coupled-integration)이 소유한다.
이 fixture는 projection/manifest owner를 유지하며 전체 projection-store cold restart를
증명하지 않는다. `AtomicIndexProjectionWriterV1::new`의 빈 root 초기화와 artifact-store attach의
root 미복구, `maybe_reuse_persisted_prepared_commit_v1`·
`published_chunk_identities_by_path_for_generation_v1`의 existing-root 요구가 별도 source gap이다.
Native reproducer는 `NOT_RUN`; manifest-only root 추측이나 새 durable protocol로 우회하지 않는다.
Target/Linux 입력 부족이 upstream 검증 코드 구현의 선행은 아니다.
[Installed/pair/action ADR](../../../adr/OCT-05-004-cost-capacity-and-qualification-boundaries.md#installed-paired-and-operational-acceptance)와
[release/proof](#release-and-proof)가 P00–P12/provider/installed/state/pair/operations의
세부 수용을 소유하고 실행 registry가 graph를 소유한다.
완료: CODE_QUALIFIED/DEPLOYED/ACTIVATED/ROLLBACK_PROVEN 각각의 실제 prerequisites·observed action.

## 잔여 실행 진입점

| 범위 | Canonical command |
| --- | --- |
| Review/unit owner | `uv run --frozen --extra dev python -m pytest tools/ci/tests/test_holdout_review.py tools/ci/tests/test_retrieval_native_span_projection.py tools/ci/tests/test_source_oracle_suite.py -q` |
| Native owner | `uv run --frozen --extra dev python -m pytest tools/ci/tests/test_opengrok_index_scope.py tools/ci/tests/test_live_lexical_external.py -q` |
| Actual quality matrix | `uv run --frozen --extra dev python tools/benchmark/retrieval/run.py quality-matrix --spec <issued-spec.json>`; `quality-matrix-verify --spec <same-spec.json>` |
| Actual external native | `uv run --frozen --extra dev python tools/benchmark/retrieval/live_lexical_external.py --spec <issued-spec.json>`; `--verify <native-root>` |
| Contract/SDK | `just retrieval-contract-proof <fresh-root>`; `just retrieval-sdk-proof-fresh <fresh-root>`; `portable_proof.py verify --receipt <root>/execution-context.json` |
| Scale/open-loop | matching `scale_matrix` / `open_loop_matrix` binaries의 실제 `--help` 및 external output |
| Pair / release | `just rust-verify-hellgate-cross-repo <Semantica-checkout>`; `just proof-p12a-proof-infrastructure`; actual manifests로 proof-authority code/release/final gates |

기존 ID는 아래 경로표와 ADR 계약으로 해석한다. 날짜별 packet/티켓을 다시 만들거나
같은 작업/무거운 rail을 다른 목록에 중복 등록하지 않는다.

## Test and platform

Owner: I0 + 실제 test/adapter owner. Contract:
[선택된 회귀·플랫폼](../../../adr/SEP-27-005-catalog-recovery-supervision-and-proof-custody.md#selected-regression-and-platform-acceptance),
[CI provider](../../../adr/SEP-28-001-circleci-provider-and-credit-boundary.md).

| 남은 scope | 종료 조건 |
| --- | --- |
| QIT-00/01/05 | 선택된 catalog의 독립 oracle·public wire negative/re-encode 및 lexical/ANN/fusion/filter/metamorphic 결과. Source registration과 실제 execution을 구분 |
| QIT-02/03/04 | 기존 lifecycle/F15/history fixture 재구현 없음. Darwin core2/lexical2 TSan 완료를 제외한 broader generated/repeat/native 및 추가 선택 storage/marker/CAS crash 결과 |
| QIT-06 / installed · J7Q-02 | 실제 설치된 SDK/CLI/daemon·consumer lifecycle/recovery·operator/preview/explain wire proof. Local/scripted peer 결과는 별도 |
| QIT-07 | 선택된 risk-owner mutation/fuzz/coverage·survivor disposition/expiry·minimized input. 옛 미승인 비율·횟수 목표는 Git history로 퇴역 |
| MISC-03 | 모든 선택 adapter의 실제 large success/failure stdout/stderr·many-entry metadata/archive/JSONL·interrupt·heap/bounded I/O |
| MISC-04/05 / QIT-09 | 완료된 b9 regular CI·fresh SDK27/original·relocated replay 및0d Contract·선택TSan을 제외한 추가 native/installed/Linux scope의 canonical nonzero inventory/terminal·replay. API/model 및 조건부 diagnostics는 선택한 범위만 판정 |
| MISC-06 / QIT-08 | 같은 selector·assertions의 paired test cost 및 query-observer/deadline/fetch parity. 정식 latency/scale/relevance는 E4/E1에 한 번만 실행 |
| MISC-07 | 선택된 registry profile·actual command/input/native capability/replay/exclusion 정합성. 지원하는 실제 multi-repo/product pilot; 작은 fixture로 all-language/full-platform을 승격하지 않음 |

필수 입력이 없으면 그 claim만 BLOCKED, 선택됐으나 미실행이면 NOT_RUN이다.
원 source의 완료된 main CI/cache/lifecycle/history 결과는 ADR에 남기며 새 source 결과와 섞지 않는다.

## Semantic

Owner: producer + semantic/search-plane/SDK/operator. Contract:
[generation/cache](../../../adr/MAY-31-001-lancedb-semantic-generation-authority.md),
[selected semantic/ANN](../../../adr/SEP-26-002-retrieval-observation-experiment-and-default-policy.md#semantic-and-ann-proof).

- 실제 typed-source ReplaceGeneration/Delta/no-op/tombstone membership과 prior-base,
  resolver/aggregate/outbox retained state·paired restart를 두 저장소의 matching source로 검증한다.
- Partial derivation 후 restart·delete/replacement·complete seal·model/dimension refusal·blocked activation,
  installed CLI/live provider/release를 선택한 rail에서 실행한다. 기존 cache matrix는 완료다.
- 실제 provider request/failure/cache와 선택된 latency/pending-work/seal-lag/model/policy/blocked 이유를
  기존 export에서 확인한다. Boot-time metric 존재는 live 동작 증거가 아니다.
- 증분 reuse는 stable producer semantic-owner identity가 필요하며 불명확하면 full rebuild한다.
  Model/render/normalization 변경은 coordinated rebuild/refusal 및 비용 검증이 필요하다.
  Batched/async worker·새 cache tier/projection은 측정·consumer 요구 이후 별도 결정이다.
- NL semantic relevance는 E1의 독립 pool/holdout, encoder latency/memory는 E4,
  큰 corpus ANN recall은 독립 exhaustive oracle이 소유한다. Exact-symbol 성공으로 대신하지 않는다.

## Release and proof

Owner: I0 + producer/운영 담당. Contract:
[installed/pair/actions](../../../adr/OCT-05-004-cost-capacity-and-qualification-boundaries.md#installed-paired-and-operational-acceptance).
Proof ID/target/host/staging/graph는 [실행 registry](../../../../tools/ci/proof-authority.toml)와
독립 checker가 소유한다. 이 표는 구현된 foundation을 다시 만드는 작업표가 아니다.

| 기존 scope | 남은 acceptance / 선행 |
| --- | --- |
| R0 / P00 | 선택된 final-source raw nonzero inventory·actual producer/native/SDK 및 relocated replay. Wrong counts/source/binary/host/run·partial/tamper refusal; trusted runner authority |
| R1 / P03–08 | 등록된 concrete release targets와 독립 oracle: activation/exact ACK/publish-only; held physical view·GC/churn/cancel; fixed quality IDs/order/windows; actual SDK wrong identity; real provider identity/egress/budget/cancel; process signal/child/readiness/FD/lease/shutdown |
| R2 / P09 | 실제 release process의 active-root stale/missing/divergent/backend-loss/restoration·cadence. Admin ring denial/caps/wrap/drop/gap/instance/restart/two-UID/request correlation. Existing root probe는 full-content scrub이 아님 |
| R3 / upstream | Semantica `6f5dc8fc13f`에 fixed replace/tombstone/unchanged·부모 membership·omission negative oracle 구현. Runtime compile `FAILED`로 test body `NOT_RUN`; canonical producer 결합 후 owning QBC 실행이 남음. Supplied scope/prior binding과 구분하며 관측된 omission 사고로 표기하지 않음 |
| R4 / S21-11 / P10 | Current-format exclusive-lease backup/verify/restore 및 native exporter mutation/replay·release/authorized target root. Disposable owner fixture는 실제 data/host qualification이 아님 |
| R5 / S21-12 / P11 pair | 두 clean source와 실제 상대 Cargo dependency/lock·fresh build/test/daemon·V2 positive/negative/exact replay. 기존 candidate-only caller archive는 운영 proof가 아님 |
| R6 / S21-12/13 / P11–12 | Concrete deploy/activate/restore-forward adapters·독립 pre/post observer 및 authorized Linux/config/state/retention/rollback 입력. 이어 P12A와 final require-all/bind-source graph; CODE/DEPLOYED/ACTIVATED/ROLLBACK 별도 verdict |

R3 upstream oracle의 owning Runtime 실행에는 Linux/운영 입력이 선행하지 않는다.
실제 target 입력 부재가 dependent action만 막는다. Historical handoff 감사는 현재 release와 별도다.

## Legacy scope routes

영구 조건은 ADR, 현재 입력/실행 상태는 위 owner에 한 번만 둔다.

| 퇴역한 RFC/플랜/티켓 ID | 현재 owner / 계약 |
| --- | --- |
| CS-BENCH-01/03 · S30-B01/02/03/05/08/09 | E1; [corpus/gold/holdout](../../../adr/OCT-05-001-review-admission-and-result-identity.md#corpus-gold-and-holdout-acceptance), [통계·default](../../../adr/OCT-05-001-review-admission-and-result-identity.md#statistical-units-and-default-decisions) |
| S30-B06 / ARB | E1/E2; per-case official base·original/adapted/no-gold·file/context budget는 위 corpus 계약 |
| CS-BENCH-02/04 · S30-B04 | E2/E4; [native/mutation](../../../adr/OCT-05-002-native-capture-clock-and-index-scope.md#native-matrix-and-incremental-acceptance) |
| CS-ENG-02 · S30-B07 · J7Q-03/04 | E4; [whole-pipeline/host/sample](../../../adr/OCT-05-004-cost-capacity-and-qualification-boundaries.md#whole-pipeline-measurement-acceptance) |
| J7Q-01/02 | E1 및 installed owner; [preview/operator](../../../adr/SEP-27-005-catalog-recovery-supervision-and-proof-custody.md#consumer-preview-and-operator-acceptance) |
| QIT-00–09 · MISC-03–07 | [Test/platform](#test-and-platform); 품질·비용은 E1/E2/E4 |
| SEM-OWN / May25 | [Semantic](#semantic); async worker/cache/projection 새 설계는 조건부 |
| CS-INT-01 · SEP-21 R0–R6/S21-11/12/13 | I0; [Release/proof](#release-and-proof) |
| CS-ENG-04 · OCT-04 proposed designs | [Deferred regex](../../../adr/SEP-27-003-code-search-source-and-preview-contract.md#deferred-regex-allocation-cap-cs-eng-04) 및 [Proposed ADR](../../../adr/README.md). 선택되지 않은 구현/릴리스 gate를 만들지 않음 |
