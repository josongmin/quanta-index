# 구조 개선 구현 ledger

> 이 문서는 진행 증거다. green test 없이 finding을 `passed`로 올리지 않는다.
> 상태 어휘: `planned | in_progress | implemented_not_run | passed | failed | blocked | not_applicable`

## 0. Frozen base

| 항목 | 값 |
| --- | --- |
| 감사 HEAD | `4914156f4191daa3e12998bdb38f2b821a057fdd` |
| 시작 시 확인한 HEAD | `4914156f4191daa3e12998bdb38f2b821a057fdd` (동일) |
| branch | `main` |
| dirty product source | 없음 |
| user-owned untracked | `docs/bugbash/` (이 캠페인의 입력 authority) |
| host | darwin 24.6.0, macOS |
| cargo target root | `~/Library/Caches/quanta-index/target` (warm, 21G) |

### 입력 문서 digest

실행 프롬프트 §2가 요구하는 입력 고정. `shasum -a 256`.

| 파일 | sha256 |
| --- | --- |
| `docs/bugbash/sep-16/findings.md` | `8baff0adf9ee0ef5a367171174c2c63aa76090f9175c1a9d8173414361137da1` |
| `docs/bugbash/sep-16/structural-remediation-plan.md` | `03286f8277b4d1d7163a1215a26250dc53489a011e23f85a140dece2d5b405f1` |
| `docs/bugbash/sep-16/test-plan.md` | `423ebb97e13a9c1ab1a20b047e55b27ea2dd1a297f5f52ff392e5088c01a2abf` |
| `docs/bugbash/sep-16/implementation-agent-prompt.md` | `10c79e927b5cdf9f890abcfa64321c55bc172c595c2ed9a6519b434c2444c295` |
| `tools/ci/test-authority.toml` | `72c9d40138666a33327421d4161b0c68059912144026b6c815b44bcbd213154c` |
| `Justfile` | `f215db9d089917c5e70e3c77569f8781f97cd60386eae28a9702bdb7adc70585` |

## 1. 실행 프롬프트 대비 적용한 수정 4건

프롬프트 원안을 그대로 쓰지 않고 아래 4건을 반영해 실행한다. 근거는 세션 리뷰에서 제시했다.

### M1 — 완료 정의를 W0 gate 결과 조건부로 분리

원안 §9의 완료 정의는 all-or-nothing이라 §5가 허용한 gate 실패와 모순된다. G0-L/G0-S가 막히면 W3가 차단되고 QI-BB-003/006/017/027/030이 구조적으로 닫을 수 없게 된다. 아래처럼 미리 분기한다.

| Gate 결과 | 완료 정의 |
| --- | --- |
| G0-L PASS | QI-BB-006 lexical lane = native snapshot 재사용으로 close. full-copy writer 삭제 |
| G0-L BLOCK | QI-BB-006 lexical lane = `blocked`, 사유·probe 증거 기록. QI-BB-003/030은 **기존 layout 위에서** close (physical GC + seal 완결성은 layout 독립) |
| G0-S PASS | QI-BB-027 = native ANN version/coverage로 close |
| G0-S BLOCK | QI-BB-027 = ANN manifest 계약 + typed 동작만 close, layout 전환은 `blocked` |
| G0-C PASS | W2 catalog 전면 도입 |
| G0-C BLOCK | W2는 선택한 대안 engine 재probe까지 `blocked`. W1/W4/W5는 계속 |
| G0-R PASS | hard-cancel 보장 구현 |
| G0-R BLOCK | cooperative-only cancellation을 **명시적 계약**으로 고정. process isolation은 별도 probe |

gate BLOCK은 실패가 아니라 확정된 설계 사실이다. BLOCK을 우회하려고 gate 기준을 낮추지 않는다.

### M2 — local commit 허용 + W별 green landing state

원안은 commit을 금지했다. 그러나 `no-dual-write` + 단일 실행자 + 136k LOC 교체에서 commit 금지는 복구 지점을 0으로 만든다. C1↔C2 사이에서 중단되면 authority가 절반만 교체된 broken 상태로 남는다.

- **허용**: `main`에 local commit (이 repo는 feature branch를 쓰지 않는다)
- **계속 금지**: push, PR 생성, deploy, production state-root migration/삭제
- 각 commit은 **green landing state**여야 한다: 그 지점에서 `just rust-profile test-fast`가 green이고 authority가 단일하게 일관됨
- 중단은 green landing state에서만 한다

### M3 — G0-C 증거 항목 확장

원안의 G0-C는 commit p95/WAL/pragma만 요구한다. 이 repo는 `#[derive(Serialize)]`까지 금지하며 cold-build 예산을 지키므로 신규 vendor 도입 비용을 gate에 포함한다. 추가 제출물:

- cold-build 시간 delta (신규 dep 추가 전/후, 동일 lane)
- MSRV 1.92.0 호환 확인
- `just rust-deny` / `just rust-machete` 통과
- **대안 engine 기각 사유** (최소 redb). 대안 없는 ADR은 거수기다

### M4 — QI-BB-007 순서 역전

원안은 hash embedder를 dev/test로 격리하면서 lexical-only mode 신설도 금지하고, 동시에 IT-16(production relevance)은 credential 부재로 blocked가 된다. 이는 **검증 가능한 대체물이 생기기 전에 로컬 유일 동작 default를 제거**하는 순서다. findings 자체가 QI-BB-007을 `P1*`(제품 결정 의존)로 표기했다.

- hash profile은 **기능 유지**. 명시적 `dev` 라벨과 profile identity만 먼저 고정한다
- production learned profile의 judged relevance가 **실측된 뒤에** 격리 여부를 결정한다
- 이 결정은 제품 계약이므로 구현자가 단독 확정하지 않고 `blocked`로 올린다

## 2. W0–W7 / C1–C4 상태

| ID | 상태 | 근거 |
| --- | --- | --- |
| W0 | in_progress | §3 참조 |
| W1 | planned | |
| W2 | planned | G0-C 의존 |
| W3 | planned | G0-L/G0-S 의존 |
| W4 | planned | |
| W5 | planned | G0-R 의존 |
| W6 | planned | M4 적용 |
| W7 | planned | |
| C1 | planned | |
| C2 | planned | |
| C3 | planned | |
| C4 | planned | |

## 3. W0 상세

| Gate | 상태 | 증거 |
| --- | --- | --- |
| W0-0 scan fixture 복구 (QI-BB-010 전제) | in_progress | §3.1 |
| G0-L Tantivy snapshot | planned | |
| G0-S Lance snapshot | planned | |
| G0-C catalog | planned | |
| G0-R runtime | planned | |

### 3.1 scan fixture 복구

**재현한 실패** (`4914156`, 2026-09-16):

```
./scripts/cargow run -p quanta-index-scan-experiment --bin scan_vs_index --locked -- \
  --out-dir <tmp>/scan1 --chunks 200 --chunk-bytes 256 --needle-count 5 --samples 5
-> scan_vs_index: typed failure GENERATION_IDENTITY_INCOMPLETE:
   lexical: incomplete generation has no sealed identity at <...>/g1/search-corpus-generation-identity.cbor
```

**원인 분류**: product defect 아님 / fixture defect. 실험 binary가 legacy `LexicalChannelOp` + `LexicalIndexBuildPort::build` 경로를 쓴다. 이 경로는 seal identity를 쓰지 않는다. 현재 authority 경로는 `SearchCorpusBatchBuildPort::build_batch(&SearchCorpusIngestBatch { seal: true, .. })`이며 `crates/quanta-index-lexical/src/lib.rs:3396` 에서 seal identity를 persist한다. 컴파일은 정상이다 (실행 불가는 runtime 계약 불일치).

## 4. Finding 상태 (QI-BB-001–032)

초기값은 findings.md 확정 상태 그대로이며 owner 배정만 기록한다.

<!-- FINDINGS-TABLE -->

## 5. 실행 command 기록

<!-- RUN-LOG -->
