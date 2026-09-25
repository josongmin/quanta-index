# SEP-26 Gap Register — P0~P2 전수 감사 (2026-09-26)

각 티켓의 작업 항목·합격 기준·테스트 글머리를 구현 상태와 대조한 빵꾸 목록.
상태: `OPEN`(미해결) / `CLEAN_FILE`(지금 작업 가능) / `BLOCKED-DIRTY`(공유 dirty 파일 대기) / `EXTERNAL`(레포 외부 입력 필요) / `DONE`.

## RBR-00 (P0) — proof contract
- inventory 3역할 exact-match(python 231 / rust 89 / sdk 12) — DONE
- sep-26 closure 등록 + 거부 테스트 — DONE
- PROFILE-CONTRACT 정의 — DONE
- receipt 최신 HEAD 재발급 — BLOCKED-DIRTY(타 writer)

## RBR-01 (P0) — diagnostics
- 응답 보존(explanation/window/executed engines) — DONE (v2 diagnostic)
- query 단계별 elapsed/call counts 계측 — PARTIAL(응답 단계 보존은 v2/v3 diagnostic으로 DONE; 단계별 timing은 PRODUCT-API 대기)
- publish 내부 단계 timing — 계측 엔진 DONE(RBR-10 `build_stream_reported`, 63 passed); 공개 SDK 노출은 PRODUCT-API 대기
- bounded opt-in stage trace — 조건부(원인 분리 실패 시만), 보류 정당
- diagnostic on/off overhead 측정 — OPEN · BLOCKED-DIRTY

## 감사·보완 이력 (2026-09-26 라운드 11-12)

- 적대적 감사 16건 중 14건 수정 완료(발견 8·16은 이 라운드에 종결; 잔여 없음). 커밋 `36b2f8c5`, `2a2b296f`.
- RBR-07 분해 레일 상륙(5 passed, 192s): short-result exact completion이 sealed effort에서 발동하지 않음을 실측 고정. RBR-10 계측 상륙(63 passed).
- RBR-01 잔여(서버 내부 publish 단계 timing의 공개 노출)은 공개 SDK API 변경을 수반하므로 별도 계약 작업으로 분리 — `OPEN · PRODUCT-API`.
- RBR-06 evaluator 지표·RBR-12 준비는 여전히 공유 dirty 대기(evaluator.py/run.py v5 진화 중).

## RBR-02 (P0) — query policy
- 정책 엔진·v4 identity·재도출 oracle·tamper 거부 — DONE
- 실daemon 문장 vs 식별자 distractor fixture — DONE(`2a2b296f`)

## RBR-03 (P0) — Semble profiles
- 4 프로파일 dispatch·lane 격리·위조 거부 — DONE(stub)
- **실제 pinned Semble 개발 캡처 + raw output** — OPEN · EXTERNAL(외부 venv 실행) + BLOCKED-DIRTY(semble.py/run.py)

## RBR-04 (P1) — symbol producer — DONE
- 5개 언어 fixture, combined 게시, digest 바인딩, 실daemon publish/reopen — DONE

## RBR-05 (P1) — symbol route
- registry·route·증명·no-answer·capture 모델·route-generic — DONE
- receipt — BLOCKED-DIRTY
- 동명이인 실daemon 케이스 — DONE(`2a2b296f`)

## RBR-06 (P1) — span 회계·청커 대조
- indexed vs SDK line span vs scored bytes + expansion ratio 기록 — OPEN
- **rank-only MRR/Hit@1, exact-span Recall@10, context bytes 보조 지표** — OPEN · BLOCKED-DIRTY(evaluator.py)
- **청커 A/B matrix(strict vs line-aligned, whole_file 대조)** — OPEN · CLEAN_FILE(chunking/*)+ EXTERNAL(실 코퍼스 실행)
- **손계산 fixture(mid-line UTF-8/CRLF/1024초과 한 줄/nested 긴 함수/attributes/다중 정의)** — OPEN · CLEAN_FILE(chunking_contract.rs)
- 지표 이름·scorer identity 분리 — BLOCKED-DIRTY(evaluator.py)

## RBR-07 (P1) — parity·exact/ANN
- **encoder full-vector parity harness(pinned Python ref vs Rust)** — OPEN · CLEAN_FILE(crates/quanta-index-embed/model2vec.rs) + EXTERNAL(venv 모델)
- manifest에 encoder/tokenizer/정밀도/truncation 고정 — OPEN
- exact exhaustive cosine oracle vs 실제 dense lane 분해 — OPEN · semantic crate(CLEAN_FILE 여부 확인 필요)
- 255/256 경계·short-result 테스트 — OPEN

## RBR-08 (P2 조건부) — exact-name ranking
- 진입 조건 probe(심볼 route에서 정답 후보가 참조/부분일치보다 낮은 재현 사례) — OPEN · RBR-05 완료로 이제 가능 · BLOCKED-DIRTY(sdk_roundtrip.rs)
- 진입 조건 없으면 유지 결정으로 종료 — probe 결과에 따라

## RBR-09 (P2 조건부) — fetch 비용
- RBR-01 stage timing이 전제 — 01 완성 후 실험 · EXTERNAL(실측)
- fetch matrix harness 코드 — OPEN (04 완료 후 우선순위)

## RBR-10 (P2 조건부) — ingest delete 비용
- owner/window 수·delete 호출/commit·단계별 시간 계측 코드 — OPEN · semantic crate 확인 필요
- 실험·최적화 후보 — EXTERNAL

## RBR-12 (P1) — 평가 종료
- task family 정의·split key·leakage validator 확장 — OPEN · BLOCKED-DIRTY(evaluator.py)
- holdout 고정 manifest — EXTERNAL(외부 설계)
- 최종 pair/replay — 적용 티켓 종료 후

## 우선순위 실행 순서 (지금 가능한 것 먼저)

1. **RBR-06 손계산 청킹 fixture** — CLEAN_FILE — 이번 라운드 착수
2. **RBR-07 parity harness 구조** — CLEAN_FILE(embed crate)
3. 타 writer 커밋 → **RBR-01 stage timing + RBR-06 evaluator 지표 + RBR-02/05 실daemon fixture** 일괄
4. RBR-08 probe → 측정 근거 확보
5. RBR-09/10 계측 → 실험
6. RBR-12 준비
