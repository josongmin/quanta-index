# 03 · 조건부 경로 posting 블록 설계와 전환

**보류.** 최근 제품 본문은 4,096개 SourceAdvance를 게시한 뒤 materialization에서 기한을 소진했다. 경로 객체 등록·자식 검증이 다시 지배한다는 근거가 생기면 기존 TEXT 포맷 작업에 반례와 검증 조건을 합친다. [최신 4K 결과](04-product-acceptance.md#기존-실행기-결과) · [채팅별 실행권](README.md#채팅별-실행권과-재사용-범위)

**착수 조건.** 현재 packing으로도 경로 맵의 물리 객체 등록·자식 검증이 읽기 예산을 압박한다는 4K 근거가 있어야 한다. [01](01-attribution.md)의 경로 맵 설치 읽기 차액·설치 객체·자식 수와 동일 입력을 기준선으로 쓴다. 패킹으로 제품 수용을 충족하면 이 패킷은 보류한다.

## TEXT가 구현할 계약

| 변경 경계 | 작업과 불변조건 |
| --- | --- |
| 문서와 포스팅 | 경로가 있는 **모든 Source owner**를 대상으로 한다. 불변 세그먼트의 문서 표가 로컬 docID와 안정된 owner를 대응시키고, 경로 term 사전이 정렬된 docID 블록을 가리킨다. 파일의 실제 revision은 조회가 고정한 SourceCut에서 확인한다. 본문 미색인 파일도 경로 질의에 남는다. 기존 본문의 정렬 docID·경계 검사·페이지 코덱을 적용 가능한 범위에서 재사용한다. [본문 wire](../../../crates/quanta-index-lexical/src/native/text_segment/wire.rs#L9) · [현재 전체 경로 수집](../../../crates/quanta-index-lexical/src/native/text_segment/build_v2.rs#L590) |
| FULL·증분 | 같은 owner·같은 경로의 본문 교체에서는 SourceCut만 새 revision을 선택하고 경로 posting 블록 쓰기는 **0건**이어야 한다. rename은 이전·새 경로의 membership만 변경한다. 삭제는 새 view에서 제외한다. 동일 term 반복, 대소문자 alias, Unicode, 통계·순서·중복 제거는 기존 공개 결과와 같아야 한다. [현재 교체 처리](../../../crates/quanta-index-lexical/src/native/text_path.rs#L503) · [기존 case 병합](../../../crates/quanta-index-lexical/src/native/text_path/term_cursor.rs#L152) |
| 조회·병합 | term 후보를 기존 owner 순서로 병합하고 선택된 view에서 원본 경로를 확인한다. merge 시 docID를 다시 매겨 해당 범위의 블록·문서 표를 함께 게시한다. 새 root가 참조하는 SourceCut과 블록은 Catalog의 도달성·회수 규칙을 따른다. [기존 질의 호출부](../../../crates/quanta-index-lexical/src/native/text_query.rs#L1754) · [기존 bounded compaction](../../../crates/quanta-index-lexical/src/native/text_segment/compact_v2.rs) |
| 저장 정책·전환 | writer·reader·root 형식·output policy를 함께 정의한다. Tree는 ObjectRef, Layered 경로 맵은 인라인 digest를 담고 두 레이아웃은 별도 정책을 사용한다. 새 포맷은 빈 루트부터 구축해 독립 검증 후 기존 expected-view 게시 경로로 전환한다. 중단되면 이전 view를 계속 열 수 있어야 한다. 이전 root를 새 포맷으로 해석하지 않는다. [현재 root 분기](../../../crates/quanta-index-lexical/src/native/text.rs#L8565) · [제품 레이아웃 검증](../../../crates/quanta-index-searchd-runtime/src/native_product.rs#L1317) |

## 채택 기준

다음 반례는 새 포맷을 구현하는 경우 필수다.

| 반례 | 검사할 공개 동작 |
| --- | --- |
| 동일 경로·owner에서 본문 revision만 변경, 또는 reverse path의 owner를 변조 | 이전·새 view에서 각각 해당 SourceRef를 얻는다. 선택한 SourceCut의 owner, SourceRecord와 reverse path owner를 인증하고 불일치를 거절한다. [현재 인증](../../../crates/quanta-index-lexical/src/native/text.rs#L9899) |
| 삭제·rename 뒤 이전 블록에 해당 term이 여전히 존재 | 살아 있는 owner만 DF에 반영한다. 경로 문서·토큰 통계, BM25 점수와 순위를 고정 입력으로 대조한다. 색인 길이 한도를 넘는 토큰이 `alpha`와 `beta` 사이에 있으면 인접 phrase로 매치되지 않아야 한다. [DF 사용](../../../crates/quanta-index-lexical/src/native/text_query.rs#L1758) · [phrase 재검사](../../../crates/quanta-index-lexical/src/native/text_query.rs#L82) |
| 같은 repository·path의 후보가 여러 개이고 최고 점수가 동점인 경우 | 기존 그룹 accumulator에서 점수 최고·동점 최소 owner를 대표로 선택한다. 그룹화 후 공개 순위, top K와 다음 페이지, `matched/remaining`을 독립 기대값과 비교한다. [대표 선택](../../../crates/quanta-index-lexical/src/native/text_path_group.rs#L180) · [경로 순위 계약](../../../crates/quanta-index-contract/src/native_protocol/read_session/ranking.rs#L14) |

1. 파일 추가·본문만 교체·rename·삭제를 순서대로 실행한다. 원본 경로 전수 스캔으로 만든 기대값과 경로 결과의 owner, revision, 순서, 건수를 비교한다. 흔한 term·희소 term·빈 결과·혼합 대소문자·Unicode·본문 미색인 파일을 포함한다. 이전 pinned view와 새 view 양쪽에서 merge 전후·재시작 후에도 같은 대조를 실행한다.
2. 잘못된 docID·누락 child·다른 SourceCut을 거절하고, 게시 직전 실패하면 이전 view와 재시도 결과가 보존되는지 검사한다. 기존 `layered_text_full_patch_pinned_views_and_restart`와 `layered_text_sixteen_live_layers_compact_without_losing_owners`의 시나리오를 재사용하고 새 포맷 reader/writer를 실제로 선택했는지 확인한다. [제품 회귀](../../../crates/quanta-index-searchd-runtime/tests/native_product_process/segmented_layered.rs#L316)
3. 동일 4K·동일 admission·같은 cold/warm 조건에서 초기 구축, 본문만 교체, rename·삭제, 흔한/희소 term 조회를 비교한다. 설치 객체·child 검증 수, 논리 읽기·쓰기, wall time, peak 메모리를 기록한다. 초기 게시 한도와 검색 정확성을 충족하고 쓰기 개선이 질의의 기존 한도를 깨지 않아야 한다.
4. 현행 Tree selector는 [04](04-product-acceptance.md)에서 원 조건대로 판정한다. 새 포맷을 출하 후보로 선택하려면 그 포맷을 명시한 후보에서도 같은 제품 시나리오를 검증한다. 기존 Layered 제품 6/6은 해당 후보의 증거로만 재사용한다. [기존 Text 수용 범위](../nov-20-native-cutover-closeout/tickets/07-product-qualification.md#ann-재시작-후속-실행)
