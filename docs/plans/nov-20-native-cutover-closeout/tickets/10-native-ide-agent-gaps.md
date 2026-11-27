# 10 · Native IDE·에이전트 기능 공백: 매치 위치, 심볼 정의, 오버레이 경로

11월 24일 [Native 보강 계획](../../nov-24-native-enhancement/readme.md#p1--ide에이전트-기능과-운영-안정성)의 P1 항목 중 01–09 어느 원장에도 담당이 없던 세 항목을 맡는다. 전환 게이트가 아니다. 단, 모든 일치 위치를 전제로 하는 전역 치환·자동 편집 기능을 출하하면 10.1은 그 기능의 P0 조건이 된다. 기본 검색 공백은 [09](09-native-search-gaps.md)가 맡는다.

## 진행 기록

**11월 24일 착수.** 10.3은 레인 C, 10.2 거절 표면 테스트는 레인 D에서 진행한다. 10.2a는 사용자 결정 전까지 **잠정 (2) 명시적 거절 유지**로 두고 새 심볼 기능은 만들지 않는다. 10.1은 09.1의 SDK·Runtime 변경이 합쳐진 뒤 2차로 진행한다.

**11월 24일 10.3 1차 합류 (레인 C, main 작업 트리 반영·미커밋).**
- **계약.** 오버레이 파일 신원은 `(repository, path)`다. 저장소는 호출자 문자열이 아니라 선택된 뷰의 인증된 durable 레코드에서 가져온다.
  - durable owner는 자기 레코드의 `source_repo_id`를 그대로 쓴다(경로를 바꾸는 경우 포함).
  - 아직 저장되지 않은 새 파일은 명시 경로와 `repository_witness`(같은 뷰에 durable 레코드가 있는 다른 owner)를 내야 하고, witness의 저장소를 따른다. 뷰에 저장소 목록이 따로 없어서, witness로 뷰 소속을 O(1)에 인증한다.
  - witness가 뷰 밖이거나, 새 파일에 witness가 없거나, durable owner나 삭제 행이 witness를 내면 `InvalidInput`이다.
  - `selected_path_owner`는 경로 맵의 저장소별 bucket으로 owner를 찾는다. 같은 저장소 안의 중복은 계속 `Conflict`다.
  - 프로토콜 변경(breaking): `NativeOverlayRequest::Update`·`NativeOverlaySnapshot`에 `repository_witness`가 추가됐고, SDK `update_overlay_at_path`에 인자가 하나 늘었다.
- **수용 `VERIFIED` (레인 보고).** 집중 nextest 55/55.
  - lexical 오버레이 테스트: 두 repo의 같은 `shared.rs`에 대한 수정·삭제·생성, 중복과 rename 충돌, witness 거절 케이스.
  - 저장소 단위 테스트: witness 보존·fence·corpus identity.
  - 데몬 UDS `repository_identity.rs`.
  - 재시작 뒤 이전 세션 거절과 새 세션 재제출, 게시본 결과의 `Durable`/`Overlay` 원본 종류 표시 회귀.
  - 수정 전에는 새 테스트 4건이 실패하는 것을 확인했다. rustfmt는 깨끗하다. clippy는 스냅샷의 기존 위반 때문에 크레이트 단위로 통과시킬 수 없어 `BLOCKED`이고, 레인이 바꾼 줄에는 위반이 0건이다.
- **남음 (C2 진행 중).** 저장 시 저장소 일치 검사, 오버레이 hit의 저장소 노출, Semantic 오버레이 Deep 범위의 `(repository, path)` 키, public-api baseline 재생성(현재 손으로 수정한 상태).

**11월 24일 10.3 2차 합류 (레인 C2, main 작업 트리 반영·미커밋).**
- **저장 검사.** 오버레이 행을 저장할 때 stage된 durable 레코드의 저장소가 행의 신원과 맞는지 확인한다. durable owner는 기준 뷰의 자기 레코드 저장소, 새 파일은 witness의 저장소와 비교한다. 다르면 `Conflict`다.
- **hit의 저장소.** `NativeOverlayTextHit.repository: Option<RepoId>`(`None`은 저장소 없는 슬롯)를 추가했다. durable hit는 자기 레코드에서, 오버레이 hit는 행의 신원에서 저장소를 가져온다. SDK는 응답에 같은 `(repository, path)`가 두 번 나오면 거절한다. 오버레이 읽기의 Path 투영과 Preview는 durable 전용이라는 규칙을 명시적으로 `InvalidInput`으로 만들었다.
- **Semantic Deep.** 코드 변경 없음. Deep은 effort 경로로만 닿고, 같은 ticket과 같은 오버레이 행으로 먼저 도는 Fast lexical 단계가 witness와 `(repository, path)` 충돌을 검사한다. 이를 지키는 회귀 테스트를 추가했다(새 파일에 witness가 없거나 뷰 밖이면 단계 0개, provider 호출 0회).
- **public-api.** 고정 툴체인 `nightly-2026-08-01`이 설치돼 있지 않아 공식 재생성은 **BLOCKED**(설치에 다운로드 승인이 필요하다). 바뀐 항목만 비교했을 때 손으로 고친 줄은 설치된 nightly의 출력과 같다.
- **수용 `VERIFIED` (레인 보고).** 수정 전에 실패하는 것을 확인했고(lexical 6건, 데몬·SDK 3건), 수정 후 집중 실행 60/61. 남은 1건은 아래 부하 민감 테스트다. `native_overlay_uds`·`native_effort`는 2스레드에서 19/19.
- **감사 결정 (C3 진행 중).**
  - witness 없는 새 파일 저장을 저장소 없는 레코드로만 제한하던 규칙은, 빈 뷰에서 이름 있는 저장소의 첫 파일을 저장하지 못하게 한다. 그래서 신원을 주장하지 않은 행은 생산자가 stage한 인증 레코드의 저장소를 따르도록 바꾼다.
  - `overlay_save_requires_original_source_cas_and_live_publisher`는 병렬 부하에서 30초 publisher lease가 끝나 실패한다. 상수를 늘리지 않고 프로토콜대로 갱신하는 구조로 고친다.

**11월 24일 10.3 3차 합류 (레인 C3, main 작업 트리 반영·미커밋).**
- **witness 없는 새 파일 저장.** 저장소를 주장하지 않은 행은 생산자가 stage한 저장소(이름 있는 저장소 포함)를 따른다. 신원을 주장한 행(durable owner, witness가 있는 새 파일)은 계속 저장소가 맞는지 검사한다. 빈 뷰에서 `repo-c`의 첫 파일을 오버레이로 저장한 뒤 게시하고 durable hit로 조회하는 테스트가 통과했다. 수정 전에는 실패하는 것을 확인했다.
- **부하 민감 테스트의 근본 원인.** overlay·save·stub 요청은 publisher lease가 살아 있는지 확인만 하고 연장하지 않는다. 연장은 `InteractionActivity`·`InteractionRenew`만 한다. 그런데 테스트는 갱신 없이 한 세션을 길게 돌렸다. 게다가 lease가 만료됐을 때의 `Conflict`가 약 26개의 위조 입력 거절 검사가 기대하는 `Conflict`와 구분되지 않아, 검사가 엉뚱한 이유로 통과할 수 있었다.
- **수정 (테스트 하네스만).** `LivePublisher`가 프로토콜로 lease를 갱신한다. 예상한 거절 뒤마다 갱신이 성공해야 하므로, 그 거절이 요청 자체 때문임이 증명된다. lease 길이와 스레드 수는 바꾸지 않았다. `native_overlay_uds`와 `native_effort` 전체를 기본 스레드로 3회 연속 실행해 54/54.
- **진행 중 (C4).** 다른 데몬 테스트의 lease 사용도 같은 구조로 바꾸고 `Conflict` 기대가 어느 원인에서 왔는지 확인한다. `native_effort` LEAK 표시의 원인을 찾는다.

**11월 24일 C4 (레인 C, 미합류).**
- **lease 구조.** 공용 테스트 헬퍼 `tests/common/interaction_lease.rs`의 `LivePublisher`를 `native_overlay_uds`, `native_effort`, `native_product_process`의 lease 사용처 전체에 적용했다. lease 만료로도 생길 수 있는 `Conflict` 기대는 모두 바로 뒤의 갱신 성공으로 원인을 확인한다. 그대로 둔 것은 명시적 `End`, 재시작, 의도적인 20ms lease처럼 lease가 사라졌기 때문에 기대되는 거절뿐이다. `native_overlay_uds`+`native_effort`를 기본 스레드로 3회 연속 실행해 54/54(통합 기준선 재실행 중).
- **LEAK 원인 (판정: 문서화).** 자식 프로세스 누수가 아니다. macOS Rust `std`는 `pipe()`를 만든 뒤 따로 close-on-exec를 설정하고, `posix_spawn`에 `POSIX_SPAWN_CLOEXEC_DEFAULT`를 주지 않는다. 그래서 nextest가 테스트 프로세스를 병렬로 띄우는 사이에 형제 테스트가 다른 테스트의 pipe를 물려받는다. 원 스냅샷에서도 재현되고, 저장소 밖의 std 전용 실험(병렬 6회 중 2회)으로 확인했다. workspace가 `unsafe_code = "forbid"`라 테스트나 하네스로는 고칠 수 없고, LEAK는 테스트를 실패시키지 않는다.
- **범위 밖.** `native_product_process`를 필터 없이 병렬로 돌릴 때의 환경 실패(legacy 바이너리 환경변수, TMPDIR 0700, 30초 기한)는 저장소 레시피가 필터·단일 스레드로만 실행하는 바이너리라 이번 범위가 아니다.
- **10.1a 착수 (C5).** 계약을 먼저 만든다. `native_protocol/text_matches.rs`에 요청, 매치 행(저장소·경로·owner·Source 신원·원문 `[start, end)`), 완료 enum `{More, Exhausted}`, 뷰·인스턴스·질의 digest·마지막 위치에 묶인 커서를 정의한다. 매치는 rg 호환 leftmost-first 비중첩 매치다.

**11월 24일 C4·10.1a 합류 (레인 C, main 작업 트리와 통합 기준선 반영·미커밋).**
- **C4 `VERIFIED`.**
  - 통합 기준선에서 `native_overlay_uds`+`native_effort`를 기본 스레드로 3회 연속 실행해 54/54, LEAK 없음.
  - `native_product_process`의 lease 테스트 2건은 이름 지정·단일 스레드로 3/3 통과.
  - `ann_quality` 4096×64는 lease를 잡기 전의 Source 수집 단계(`submit SourceAdvance`, `stage_mutations`)에서 30초 기한 초과로 3/3 실패했다. 부하를 배제하지 못했고 lease 변경과는 무관하다.
- **10.1a 계약 (`native_protocol/text_matches.rs`).**
  - 타입:
    - 요청: 선택 뷰, 인스턴스, 질의, `after`, limit 1..=1024.
    - 질의: Literal 또는 Regex, 필수 case 정책, 범위 제약. 패턴 1..=4096바이트.
    - 행: 저장소, 경로, owner와 Source `ObjectRef`, 원문 `[start, end)`. 빈 범위는 금지.
    - 페이지: 완료 `More | Exhausted`. 부분 완료는 없다.
    - 커서: 뷰, 인스턴스, 질의 digest, 마지막 `(repository, path, owner, start)`.
  - 의미:
    - UTF-8 텍스트로 허용된 Source만 대상이다.
    - rg 기본값처럼 줄 단위로 찾고, 줄을 넘는 매치는 없다. `\n`이 필요한 패턴은 거절한다.
    - leftmost-first 비중첩이고, 빈 매치는 버린다.
    - UTF-8 문자를 쪼갤 수 있는 바이트 수준 구성은 거절한다.
    - Insensitive는 Unicode simple case folding이다.
    - 정렬은 `(repository, path, start)`이고, 예산 초과·취소는 요청 전체 거절이다.
  - 검증: 페이지 decode 때 행 수, 범위, 경로, 정렬, 파일 안 단일 신원·비중첩, 완료와 커서의 일치를 확인한다. 요청에 대해서는 뷰·인스턴스·질의가 맞는지, 커서 다음부터 시작하는지 확인한다.
  - 계약 테스트 7/7.
- **감사 결정.**
  - Source 신원은 레인 B의 `NativeSourceIdentity`가 합류하면 그 타입으로 바꾼다.
  - 요청도 decode할 때 검증한다(C6a).
  - 경로 규칙이 SDK·런타임·계약 세 곳에 복제된 것은 B 합류 뒤 하나로 합친다.
  - request/response enum 연결은 B 합류 뒤에 한다.
  - lexical 엔진 핵심(C6b)은 Literal 앵커와 A1 입장 규칙을 재사용하고, `rg --json` 골든으로 수용한다.

**11월 24일 10.1b 엔진 핵심 합류 (레인 C6a·C6b, main·통합 기준선 반영·미커밋).**
- **C6a.** 매치 위치 요청도 decode·encode할 때 `validate()`를 거친다. 수정 전에는 위조한 요청이 decode를 통과하는 것을 확인했다. 계약 테스트 8/8.
- **C6b `text_match_scan.rs`.**
  - 매칭: `\n`으로 줄을 나누고 `\r`은 줄에 남긴다. 줄마다 `^`·`$`가 걸린다. leftmost-first 비중첩이고 빈 매치는 버린다. Insensitive는 regex 엔진의 simple case folding이다.
  - 패턴 거절: `regex_syntax` `utf8(true)`로 UTF-8을 쪼갤 수 있는 바이트 구성을 거절하고, HIR을 훑어 `\n`이 필요한 패턴도 거절한다. 둘 다 본문을 읽지 않는다.
  - 후보: 대소문자 구분 리터럴(NFC 이미지와 같은 경우)만 Literal 앵커를 쓴다. 나머지는 전수 검사 상한(4,096)을 먼저 확인한다.
  - 페이지: 파일은 `(repository, path)` 순서다. 커서는 `start` 다음부터 이어지고, owner나 Source가 바뀌면 `Conflict`다. 다음 매치를 하나 더 확인해 `More`·`Exhausted`를 정한다. 완성한 페이지는 `validate_for`로 스스로 검증한다.
  - 수용 `VERIFIED`: 손으로 만든 바이트 골든 7건(반복·인접·겹침 후보, NFC·분해형·CRLF, Kelvin, 페이지 경계와 커서, 거절 패턴 읽기 0회, 예산·취소, 4,097파일 상한과 앵커 3파일 읽기 3회 이하). lexical 36/36. 변이 4종(커서 비교, 빈 매치, 줄바꿈 판정, owner 검사)이 모두 테스트에 걸렸다.
- **감사: 큰 저장소 기능 공백 (C6c·C6d 진행 중).**
  - 대소문자 무시 리터럴과 regex는 앵커가 없어, 4,096파일을 넘는 뷰에서 거절된다.
  - C6c: regex 엔진의 simple-fold 동치류로 gram 변형을 만들고, 전체 scalar 테스트로 그 변형이 빠짐없음을 증명한다.
  - C6d: `regex_syntax` literal 추출로 모든 매치가 반드시 포함하는 리터럴 집합을 얻었을 때만 앵커를 쓴다.
- **C6c 합류 (main·통합 기준선 `a6fc5d1f1`, 미커밋).**
  - 대소문자 무시 리터럴의 각 문자를 regex 엔진이 쓰는 `regex_syntax` simple-fold 클래스(`try_case_fold_simple`)로 펼쳐 Literal 앵커를 만든다.
  - 앵커 모듈에 변형 출처를 매개변수로 받는 `CharVariants`를 추가했다(구현: `CaseInverse`, `ExactScalars`, `SimpleCaseFold`). 기존 `anchor_needle` 동작은 그대로다.
  - 재사용할 `RequiredLiterals`·`anchor_required`(멤버 앵커의 합집합, 하나라도 앵커가 안 되면 전수 검사)를 추가했다.
  - 대소문자 구분 리터럴은 Literal 형제가 raw 바이트 trigram을 색인하므로 `ExactScalars`로 앵커한다. 분해형 같은 비 NFC 리터럴도 앵커된다.
  - 수용 `VERIFIED`: 4,097파일 뷰에서 `needle_k`·`NEEDLE_\u{212A}`(Kelvin 포함)는 각각 본문을 3회 이하로 읽고 정답 위치를 내며, 미끼 파일은 읽지 않는다. 테스트는 먼저 `ResourceLimited`로 실패하는 것을 확인했다.
  - 모든 scalar에 대해, 엔진이 `(?i)`로 매치하는 문자가 그 문자의 변형 집합과 정확히 같음을 확인했다. 변이 2종이 테스트에 걸렸다. lexical 38/38.
- **C6d 합류 (main·통합 기준선 `94ae83e32`, 미커밋).**
  - `RequiredLiterals::of_regex`는 `regex_syntax` `Extractor`의 Prefix 집합, Suffix 집합을 차례로 쓴다. 모든 매치가 유한 집합의 한 멤버로 시작(끝)한다는 문서화된 계약에 근거한다.
  - 무한 집합이나 빈 멤버가 있으면 그 집합은 건너뛴다. 빈 유한 집합은 매치가 없다는 뜻이라 후보 0이다. 길이 한도에서 잘린 멤버는 UTF-8 경계까지 줄인다. Insensitive는 fold 클래스로 정규화해 `SimpleCaseFold`로 앵커한다.
  - 줄 단위 매칭은 매치를 줄일 뿐이므로, 본문 단위 포함만 요구하는 앵커는 여전히 상위집합이다.
  - 수용 `VERIFIED`: 4,097파일 뷰에서 ripgrep 15.1.0 `--json` 골든 8개 패턴의 바이트 범위가 일치하고 본문 읽기는 1–3회다(`foo\d+bar`, alternation, `(?i)` Kelvin, suffix 전용, 빈 집합, 100바이트에서 잘린 멀티바이트 prefix·suffix). 필수 리터럴이 없는 패턴은 읽기 0회로 거절된다. 테스트는 먼저 실패하는 것을 확인했고, 변이 2종이 테스트에 걸렸다. lexical 39/39.
  - lexical의 경로 규칙 복제본은 계약의 `is_canonical_source_path`로 바꿔, 경로 규칙은 이제 한 곳뿐이다(합친 뒤 컴파일 확인은 C7a).
- **C7a `VERIFIED`.** 통합 기준선 `94ae83e32`에서 lexical 53/53, `native_overlay_uds` 9/9. lexical의 경로 규칙을 계약 함수로 바꾼 부분이 컴파일된다.
- **C7b 합류 (main·통합 기준선 `492a2bc25`, 미커밋).** 오버레이 행에도 `repo:`·`lang:`이 적용된다.
  - `TextOverlaySource`에 인증된 `repository`·`language`가 들어간다. 언어는 행의 durable owner 레코드의 것만 쓰고, 새 파일은 witness의 언어를 물려받지 않는다.
  - 메타데이터가 없는 상태에서 필터를 쓰면 A2와 같은 `InvalidInput`이다. 기존 거절 가드는 제거했다.
  - 수용 `VERIFIED`: reader 테스트 3건(두 repo의 같은 경로에서 `repo:`로 선택, durable 언어, 새 파일 `lang:` 거절)은 먼저 실패하는 것을 확인했다. 질의 수준 테스트는 새 계약으로 바꿨다. 변이 2종이 테스트에 걸렸다. lexical 91/91, `native_overlay_uds` 9/9.
- **C8 합류: `repo:` 필터의 제품 예산 결함 수정 (main·통합 기준선 `03905eb1d`, 미커밋).**
  - 제품 읽기 예산: SDK는 기한만 보낸다. 데몬 설정의 work bytes는 512MiB, 질의 메모리 need는 64MiB다.
  - 결함: `repo:` 패턴 하나에 문서 엔진 최악 상수(12MiB와 parser 3.2MB, 약 15.8MB)가 과금됐다. 그래서 `/n.edle/ repo:… ×4`가 제품 한도에서 `ResourceLimited`로 거절됐다. 데몬 테스트로 먼저 재현했다.
  - 수정: `repo:` 패턴은 `lq-regex` `FieldRegex`(Thompson NFA, `nfa_size_limit` 1MiB, PikeVM, leftmost-first)로 컴파일한다. NFA heap, PikeVM cache, epsilon-closure stack 상한을 합친 실측 보유량으로 과금한다. parser·NFA 예약은 생성하는 동안에만 잡는다. NFA 한도를 넘으면 typed `ResourceLimited`다. 예산을 늘리거나 단순 이름을 특례 처리하지 않았다.
  - 실측: `repo:` 1개는 약 1.2KB, 4개는 약 4.7–5.2KB(이전 63MB), Unicode 클래스 4개는 약 94KB.
  - 수용 `VERIFIED`: field 엔진과 문서 엔진이 10개 패턴×7개 값에서 같은 결과를 낸다. 과금 테스트는 먼저 실패하는 것을 확인했다. 공개 SDK 요청 형태와 기본 예산으로 두 repo에 `language_any_of`, `repo:` 4개 regex 최대치, 오버레이 유무를 모두 실행해 `native_overlay_uds` 10/10. lq-regex 107/107, clippy 0건.
- **C9 합류: 본문·경로 regex 엔진 과금 결함 수정 (main·통합 기준선, 미커밋).**
  - 재현: 제품 한도(64MiB)에서 본문 regex 3개, 경로 필터 2개, `file.contains`, `repo:`, `lang:`이 섞인 질의는 엔진 5개×약 15.8MB로 `ResourceLimited`였다. 데몬 테스트에서 red로 확인했다.
  - 수정: `lq-regex` `DocumentEngine`을 쓴다. 정방향·역방향 Thompson NFA는 각 10MiB 한도, prefix prefilter, lazy DFA는 정·역방향 각 `cache_capacity` 2MiB(meta와 같은 설정), 그리고 PikeVM이다. lazy DFA가 `GaveUp`이나 `Quit`(비 ASCII에서 Unicode `\b`)을 내면 같은 검색을 PikeVM으로 다시 한다.
  - 과금은 NFA·prefilter·PikeVM cache·closure stack 상한에 lazy DFA가 있을 때 역방향 NFA와 cache 용량 2개를 더한 값이고, HIR heap은 구조로 측정한다. parser와 생성 grant는 생성하는 동안에만 잡는다.
  - 실측: 엔진당 약 4.2MB(이전 15.78MB). 본문 4개는 16.8MB(이전 63.1MB), 본문 4개와 경로 2개는 25.2MB(이전 94.7MB).
  - 수용 `VERIFIED`: lq-regex 골든 109/109(`regex::bytes` 대조, Unicode·`(?i)`·`(?m)`·`\b`·비 UTF-8·빈 매치). lazy DFA의 `GaveUp`·`Quit`이 PikeVM으로 넘어가 같은 답을 내는지 검증했다. `native_overlay_uds` 11/11, lq-structural 82/82, search-plane 66/66.
- **C10 1단계 합류: 엔진을 명시 설정한 `meta::Regex`로 교체 (main·통합 기준선 `2bfe84281`, 미커밋).**
  - 벤치마크(릴리스 빌드, 16MiB 코퍼스 671개 문서, 15회 중 최솟값, 매치 수 동일 확인)에서 명시 설정 meta가 `DocumentEngine`보다 느린 경우는 없었다. inner 리터럴 `\w+foo\w+`는 34.6→1.0ms, suffix 리터럴은 12.8→0.3ms, Unicode `\b\w+ing\b` find는 479.9→5.5ms, 시작 앵커는 18.3→0.4ms다. 1MiB full DFA 변형은 alternation에서 3배 느려 채택하지 않았다.
  - 설정: LeftmostFirst, `nfa_size_limit` 10MiB, `hybrid_cache_capacity` 2MiB, `onepass_size_limit` 1MiB, `dfa(false)`. backtracker는 frame stack을 제한하는 설정이 없어 `backtrack(false)`로 끈다. `document.rs`는 삭제했다.
  - 과금은 `memory_usage()`에 cache 상한을 더한 값이다. cache 상한은 lazy DFA 3×2MiB와 PikeVM 항(상태×48바이트와 closure frame)이다. 무거운 검색 뒤 `Cache::memory_usage`가 이 상한 안에 있음을 테스트로 증명했다. 엔진당 약 6.3MB.
  - lq-regex 골든 109/109, `native_overlay_uds` 11/11, lq-regex clippy 0건.
- **C10 2단계 합류: 질의 준비 control scope 거절 수정 (main·통합 기준선, 미커밋).**
  - 재현: 제품 한도에서 `/needle/ /n.edle/ /alpha/ /^needle/ file:src`가 `ResourceLimited`였다(red).
  - 런타임 lowering의 regex 준비 backing(regex당 12.8MB)은 이제 `tokens.parse()` 동안에만 Temporary로 잡는다.
  - `validate_native_text_input`은 `NativeTextPlanBacking { retained, planning }`을 돌려준다. retained는 구조 부분과 regex당 필수 gram 조건 상한(`MAX_NECESSARY_GRAM_NODES` 512 × 노드 크기)이고, planning은 엔진당 수치를 `LexicalPlanner::plan` 동안에만 Temporary로 잡는다. lq-regex는 planner 상한을 공개 상수로 내보낸다.
  - 효과: 본문 regex 4개와 `file:src`에서 51.2MB+16.0MB를 질의 내내 잡던 것이 이제 parse 동안 51.2MB, plan 동안 16MB만 잡고, 끝까지 남는 것은 약 66KB다.
  - 수용 `VERIFIED`: `native_overlay_uds` 11/11(durable·오버레이, 본문 regex 4개와 전체 필터 조합 포함), lexical 37/37, lq-regex 109/109, 런타임 lowering 7/7. 변경하지 않은 상태에서도 실패하는 기존 결함: 런타임 lib 4건, `native_index_uds` 3건, legacy 바이너리 환경 5건.
- **C10 3단계 합류 (main·통합 기준선 `07114a291`, 미커밋).**
  - Native fan-out 한도는 계약 상수(`MAX_NATIVE_FILE_CONTAINS_LEAVES`·`MAX_NATIVE_PATH_CONTAINS_LEAVES`·`MAX_NATIVE_PATH_FILTERS`) 하나에서 가져온다. lexical의 사본은 삭제했다.
  - `file:` 경로 필터는 `FieldRegex`로 컴파일한다. 언어는 이전과 같다(같은 HIR, NFC·`(?i)` 적용 없음). 과금 테스트는 먼저 실패하는 것을 확인했다. 필터 4개의 과금이 26.2MB에서 1MiB 미만으로 줄었다. 경로 7개 패턴×5개 값(Unicode 경로 포함)에서 문서 엔진과 결과가 같다.
  - 허용되는 가장 무거운 질의(본문 regex 4개, `file:` 4개, `repo:` 4개, `file.contains`, `path.contains`)가 제품 한도에서 durable·오버레이 모두 정답을 낸다. 이전의 `ResourceLimited`는 명시된 한도가 아니라 과금 계산 탓이었다(red 확인).
  - `native_overlay_uds` 11/11, lq-regex 109/109. lexical 793건 중 기존 3건만 실패한다.
- **C11 합류 (main·통합 기준선 `49b1ec76e`, 미커밋).** `repo.has.file`의 `path:`·`name:` 매처는 전체 일치(`into_full_match`) `FieldRegex`로 컴파일한다. 매처 4개의 과금이 25.8MB에서 1MiB 미만으로 줄었다(red 먼저 확인). 전체 값 일치 의미, case 무시, `name:` 마지막 segment 규칙은 그대로다. `content:` regex는 본문 전체를 검색하므로 문서 엔진에 남는다. 골든 19/19, lq-regex 110/110.
- **C12 설계 (구현 전).** 경로 gram 색인은 Literal 형식을 재사용하는 별도 PathGram 멤버로 둔다. FileMap V3 루트와 Text wire는 바꾸지 않는다. 선택 규칙은 `pinned_literal_sibling`과 같고, 앵커는 `anchor_needle`과 `CaseInverse` 그대로이며, 검증은 경로 문자열에서 한다. frontier만 바뀌면 `Reuse`다. 블록은 4KiB를 제안한다. 추정 이득은 1만 경로에서 거의 없고, 10만 경로에서 reads 7–25배다. 그래서 C13에서 10k·100k를 실측하고, 100k에서 5배 이상이면 구현한다.
- **C13 실측: PathGram 멤버는 만들지 않는다.** 릴리스 빌드, 7회 중 최솟값이고 양쪽의 매치 수가 같다. 코퍼스는 저장소 형태의 경로로 평균 38.6바이트, Unicode 디렉터리가 약 2%다. gram 색인 크기는 경로 map의 6.9배다(10만 경로에서 52MB).
  - 100k 선택적(sensitive): census 1,563 reads 대비 90 reads로 17.4배다.
  - 100k 선택적(folded): 3.5배.
  - 100k 중간(1% 경로): 오히려 12–72배 더 읽는다.
  - 선택도가 낮은 needle은 대체 경로(census)로 가며 앵커 시도 비용으로 +6–20 reads가 든다.
  - 결정 규칙(100k 선택적·중간에서 5배 이상, 대체 경로에서 손해 없음)을 충족하지 못했다.
  - 원인은 색인 조회가 아니라 `anchor_needle`이 후보마다 다른 창의 gram 조합을 probe하는 데 있다(후보×창×조합만큼 블록을 읽는다). 4,096 owner 상한도 경로에서는 비용 상한이 되지 못한다(10k의 `src`가 상한 아래라 앵커가 쓰였고 census의 84배였다).
- **C14 진행 중.** 공용 앵커(본문 `file.contains`, fold 앵커, regex 필수 리터럴, 매치 스캔이 함께 쓴다)를 gram마다 posting을 한 번씩 순차로 읽는 merge-join(창 안 변형은 합집합, 창 사이는 교집합)으로 바꾼다. 결과 집합이 기존과 같은지 무작위 동치 테스트로 증명하고, 경로·본문 코퍼스에서 전후를 측정한다.
- **C14 완료(11월 24일, 통합 합류 대기).**
  - **동치:** 고정 seed 무작위 테스트(본문 80개 × needle 160개)에서 merge와 probing의 결과가 `ExactScalars`·`SimpleCaseFold`·`anchor_needle` Sensitive/Folded 모두에서 같다. 빈 앵커가 아닌 경우가 100건을 넘어야 통과한다.
  - **참값 상위집합:** 결과가 참값을 모두 포함하는지 세 기준으로 대조한다. 바이트 `contains`, `regex`의 `(?i)` 리터럴, `apply_case(nfc(body))` 정책이다. İ와 ı를 포함한다.
  - **비용 테스트:** 바꾸기 전에는 probing이 18,040회 읽어 red였고, 바꾼 뒤 300회 이하로 green이다.
  - **측정 조건:** release 빌드, 부하 300–500에서 측정했다. 수치는 읽기 횟수(r)와 읽은 바이트(b)다.

  | 코퍼스 · needle | census | probing(전) | merge(후) |
  | --- | --- | --- | --- |
  | 경로 10k · mid, fold | 157 r / 757K | 11,318 r / 182.5M | 434 r / 3.88M |
  | 경로 100k · mid, fold | 1,563 r / 7.56M | 112,142 r / 1.81G | 2,296 r / 5.25M |
  | 본문 30k · mid, fold | 30,000 r / 154M | 31,618 r / 505.8M | 1,134 r / 5.54M |

  - **결과 요약:** 중간 선택도 needle에서 읽기가 5–50배, 바이트가 30–340배 줄었다. 선택적 needle은 probing과 ±3% 안이다.
  - **비선택 needle:** 결과 4,096 이하의 비선택 needle은 여전히 census보다 많이 읽는다. 구조 입장 규칙상 예상된 것이고, 추정치로 거절하지 않는다.
  - **측정하지 않은 것:** PathGram은 만들지 않았다. 본문 100k는 메모리 내 코퍼스가 약 1.1GB라 측정하지 않았다.
- **진행 중.** 레인 B4: `NativeSourceIdentity` 교체, request/response·런타임·SDK·CLI 연결, 데몬 E2E. 레인 B3: 경로 규칙을 계약의 함수 하나로 정리.

**11월 24일 10.1 end-to-end 연결 (레인 B4, 통합 기준선 재검증 중·미합류).**
- **계약.**
  - `NativeTextMatch.native_source`를 공용 `NativeSourceIdentity`로 바꿨다.
  - `NativeIndexRequest::TextMatches`(tag 77, 상태 없이 뷰 선택)와 `NativeReadSessionRequest::SearchTextMatches`(tag 17, 세션의 고정 뷰)를 추가했다. 응답은 `NativeIndexResponse::TextMatchPage`다.
  - wire inventory를 갱신했고 검사가 통과한다.
- **lexical·런타임.**
  - lexical에는 `NativeTextMatchReader`(`IndexQueryPort`)를 추가했다.
  - 런타임은 `NativeServiceCapabilities.text_matches`로 등록하고, 상태 없는 경로와 세션 경로를 dispatch한다. 세션 경로는 세션 뷰가 아닌 선택을 `InvalidInput`으로 거절한다. 페이지마다 Core coverage와, 완료 상태와 전수 여부가 일치하는지 확인한다.
  - 합류 때 해결한 충돌: 제품 리더 하나를 `with_literal_sibling`과 함께 만들어 랭킹 리더와 매치 리더가 공유한다. 그래서 매치 스캔도 제품에서 Literal 앵커를 쓴다.
- **SDK·CLI.**
  - `NativeUdsClient::search_text_matches`(`_in_read_session`)와 상위 `client.lexical().matches(query, limit, after)`를 추가했다. `.text()`와 같은 세션 보유 모델(뷰당 lease 하나, renewer, 신호만 보내는 Drop)을 공용 `NativeViewSessions`로 쓴다.
  - `searchctl matches`를 추가했다.
- **수용 `VERIFIED` (레인 보고, 이전 기준 `499afc250`).**
  - 데몬 E2E 3건: ripgrep 15.1.0 `--json` 골든으로 두 case 모드, CRLF, UTF-8 `café`, 반복·인접 매치, 인접 매치 사이의 페이지 경계, 상태 없는 경로와 세션 경로의 동일성, CLI 페이징을 확인했다. 게시 뒤에도 보유한 커서는 이전 뷰를 끝까지 읽고, 다른 뷰로 다시 묶은 커서는 `Conflict`다. lease를 해제하면 뷰가 회수된다. 예산 초과·취소는 거절된다.
  - 테스트는 먼저 실패하는 것을 확인했다. 계약 9/9, `code_search` 5/5, searchctl 70/70.
- **감사 후속 B4b (P0).** 커서가 owner만 묶어서, 같은 owner의 내용 변경을 위조된 커서로 감지하지 못한다. 커서가 `NativeSourceIdentity` 전체를 묶고, 다르면 `Conflict`가 되도록 고친다.
- **BLOCKED.** public-api baseline 재생성은 고정 툴체인 `nightly-2026-08-01` 설치(다운로드 승인)가 필요하다.
- **B4·B4b 통합 기준선 합류 (`457010a04`, 미커밋).**
  - 재검증 `VERIFIED`: 계약 9, lexical 16, searchctl 70, E2E `text_matches` 3, `code_search` 5, `repository_query_contract` 2.
  - B4b: 커서가 `native_source: NativeSourceIdentity` 전체를 묶는다. 같은 owner의 Source가 바뀐 뒤 다른 뷰에 다시 묶은 커서는 `Conflict`다. 수정 전에는 데몬이 다른 텍스트의 바이트 위치에서 페이지를 이어 읽는 것을 red로 재현했다. 페이지 검증도 `next.native_source == last.native_source`를 요구한다.
- **main 합류 보류 (사용자 확인 필요).** main 작업 트리에서 이 세션이 아닌 다른 편집 주체(확인한 Claude 세션 두 곳은 아니라고 답했다. 같은 머신의 다른 에디터 에이전트로 추정)가 같은 파일을 진행 중으로 고치고 있다: `sdk/src/client/native_code_search.rs`(커서 `native-code-search-path-v2`, revision 바인딩, 10:57), `contract/src/native_protocol.rs`, `native_protocol/read_session.rs`, `searchd-runtime/src/native_read_session_transport.rs`. B4의 이름 변경(`NativeViewSessions`·`NativeTextRoute`)·`TextMatches` 변형·커서 변경과 3-way로 15곳(1·3·7·4)이 충돌한다. 그 작업을 덮어쓰지 않도록 main에는 적용하지 않았다. 패치는 레인 디렉터리의 `PENDING-main-b4.patch`다.

## 10.1 · 원문 매치 위치를 이어 읽기

**현행 코드.** Native Literal hit는 owner와 Source 레코드만 반환한다. Text Preview는 별도 단발 요청에서 최대 240바이트 문맥과 32개 하이라이트를 준다. 유지 세션의 페이지 응답과 오버레이 경로에서는 Preview를 제공하지 않는다. [Literal hit](../../../../crates/quanta-index-contract/src/native_protocol.rs#L1075) · [Preview 상한](../../../../crates/quanta-index-contract/src/native_protocol/text_preview.rs#L18) · [SDK Preview 범위](../../../../crates/quanta-index-sdk/src/native_uds.rs#L2657)

- [x] **10.1a 계약.** 같은 고정 뷰의 `(repo, revision, path, source digest)`와 원문 `[start_byte, end_byte)`를 매치 단위로 반환하는 커서 계약을 정한다. 커서는 view·instance·query·마지막 `(owner, start_byte)`에 묶이고, 다른 뷰나 원문 해시가 바뀐 파일에서는 거절한다. 완료 여부(전수 완료, 예산 거절, 취소)를 응답에 명시한다.
- [x] **10.1b 수용.** 반복 매치, 겹치는 매치, UTF‑8 다바이트·NFC·CRLF, 멀티라인, 페이지 경계에 걸친 매치, 원문 해시 변경을 고정 원문의 `rg --json` 독립 결과와 바이트 단위로 대조한다.

**담당** 기존 TEXT·SDK 오너. **상태** 10.1a·10.1b 통합 기준선에서 `VERIFIED`(계약 C5·C6a, 엔진 C6b–C6d·C10, end-to-end B4·B4b, rg 골든 E2E). main은 B4가 보류 중이라 main 작업 트리에는 계약·엔진만 있고 런타임·SDK 연결은 없다. 감사 후속: 매치 스캔 regex 과금(C, H-C), 커서 태그의 `-v1` 제거(B).

## 10.2 · 심볼 정의 검색의 Native 대체 또는 퇴역

**현행 코드.** Native Text 입구는 `path`·`name`·`content`·`lang` 술어만 받고 `symbol.has.name`은 거절한다. searchd Runtime에는 Native Symbol 질의 경로가 없다. 구 Quanta Symbol 조회는 [04](04-maintained-test-migration.md)에서 고정 old-Q 데몬 테스트로만 유지된다. [Native 술어 허용 목록](../../../../crates/quanta-index-lexical/src/native/text_preflight.rs#L114) · [거절 회귀](../../../../crates/quanta-index-lexical/src/native/text_preflight.rs#L441)

- [ ] **10.2a 결정.** (1) 생산자가 제공한 심볼 권위(이름·종류·원문 범위)를 Native 뷰에 연결해 정의 검색을 제공하거나, (2) Native에서 퇴역시키고 명시적 거절과 대체 경로(기존 분석 경로의 심볼 해석)를 [04](04-maintained-test-migration.md)·[06](06-tantivy-retirement.md) 퇴역 목록에 기록한다.
- [ ] **10.2b 수용.** (1)이면 두 repo의 같은 이름·같은 경로, 삭제된 정의, DELTA 뒤 이동한 정의를 독립 심볼 목록과 대조한다. (2)이면 공개 SDK·CLI에서 거절 오류와 안내를 확인한다.

**담당** 기존 TEXT 오너와 Semantica 심볼 생산자. **상태** 10.2a 결정 필요.

**11월 24일 10.2b 잠정 (2) 수용 `VERIFIED` (레인 D3, main·통합 기준선 반영·미커밋).** 실제 데몬 테스트 `symbol_refusal`로 확인했다.
- 같은 뷰에서 `needle`은 1건이 매치된다(빈 코퍼스가 아님을 확인하는 대조군).
- `symbol.has.name(needle)`과 그 AND·OR 조합은 SDK(읽기 세션 페이지와 단발 `search_text`)에서 I/O 전에 `QueryRejected{Lexical(_)}`로 거절된다. SDK를 우회한 raw 요청은 `Refused(InvalidInput)`이다. 어느 쪽도 빈 결과가 아니다.
- 거절 뒤에도 같은 세션이 계속 답한다.
- 발견: SDK 코드는 `Lexical(ParseFail)`인데 메시지는 `UNSUPPORTED_COMBO`라서 CodeSearch 경로의 `UnsupportedCombo`와 다르다. SDK 레인에 넘겼다.
- 제품 결정(대체 또는 퇴역)은 여전히 사용자 몫이다.

## 10.3 · 오버레이의 저장소별 경로 식별

**현행 코드.** 한 읽기 요청의 오버레이 원문끼리 상대 경로 문자열만 비교해 같으면 `Conflict`를 반환한다. 오버레이 행은 `OwnerKey`·revision·본문·경로만 갖고 저장소 신원은 비교에 쓰지 않는다. 여러 저장소가 들어 있는 뷰에서 서로 다른 repo의 같은 상대 경로를 동시에 편집하면 정상 요청이 거절된다. [경로 비교](../../../../crates/quanta-index-lexical/src/native/text_reader.rs#L851) · [오버레이 행](../../../../crates/quanta-index-lexical/src/native/text_reader.rs#L54) · [프로세스 내 오버레이](../../../../crates/quanta-index-searchd-runtime/src/native_overlay.rs#L1)

- [x] **10.3a 계약.** 오버레이 충돌 판정의 키를 `(repo, path)`로 바꾼다. 저장소 신원은 호출자 문자열이 아니라 선택된 뷰의 인증된 Source 메타데이터에서 가져온다.
- [x] **10.3b 수용.** 두 repo의 같은 경로에 대한 미저장 생성·수정·삭제가 각각 자기 repo의 결과만 가린다. 같은 repo·같은 경로의 중복 행은 계속 `Conflict`다. 저장 CAS 경합, 재시작·publisher 유실 뒤 이전 세션 거절과 새 세션 재제출, 게시본만 조회된 결과의 원본 종류 표시를 확인한다.

**담당** 기존 TEXT·Runtime 오버레이 오너. **상태** 10.3a·10.3b `VERIFIED`(C 10.3·C2·C3·C4, 저장 시 저장소 검사, hit 저장소 노출, lease 원인 확인, `native_overlay_uds` 11/11). 오버레이 `repo:`·`lang:` 필터(C7b)와 경로 leaf(E2b)도 합류했다.
