# 09 · Native 기본 검색 연결, 본문 질의 입장 정책, Zoekt 비교 합격선

11월 24일 [Native 보강 계획](../../nov-24-native-enhancement/readme.md)에서 찾았지만 01–08 어느 원장에도 담당이 없던 기본 검색 항목을 맡는다. 계획 문서는 제안과 근거만 담고, 구현·실행 상태는 이 티켓에 기록한다.

## 진행 기록

**11월 24일 결정과 착수.** 사용자가 끝까지 진행하도록 위임해 아래 결정으로 착수했다. 오너 확인 전까지 잠정 결정이다.
- 09.1a: **(1) Native 연결.** 기존 CodeSearch 문법(`quanta-index-search-plane/src/lowering.rs`)을 공용 위치로 옮겨 하나의 구현으로 Native 계획에 연결한다.
- 09.2a: **B · 구조적 후보 상한.** DF 합 대신 질의 구조로 상한을 계산한다(Term=DF, Phrase·AND=최솟값, OR=합, 단독 NOT·regex·contains·repo=무제한). AND는 DF가 가장 작은 커서를 기준으로 나머지를 탐색해, 원문을 검증하는 파일 수가 이 상한을 넘지 않게 한다. 새 임계값은 두지 않는다.
- 09.3: 고정 Zoekt 빌드에서 etcd의 로컬 독립 정답 4건을 확인했다. 출하 후보·Linux 입력과 짝 비교는 09.3a–d에서 수용한다.
- 구현은 현재 작업 트리 스냅샷 `802ac88a2`(`refs/lanes/snapshot-20261124`)에서 나눈 레인 worktree로 진행하고, 끝나면 main 작업 트리에 합친다. 레인 A: 09.2·09.1c(lexical), 레인 B: 09.1a·b·CLI·d와 `language_any_of` 하향.

**11월 24일 09.2 1차 합류 (레인 A1, main 작업 트리 반영·미커밋).**
- **구현.** `text_query.rs`의 DF 합 사전 거절을 질의 구조에서 계산한 후보 상한으로 바꿨다(`candidate_bound`). Term=DF, Phrase=가장 드문 항, AND=유한한 자식 중 최솟값, OR=합(넘침은 `ResourceLimited`), NOT·regex·contains·repo 단독=무제한. 상한의 유한 여부가 컴파일러의 `anchored` 판정과 어긋나면 `BackendViolation`이다(두 규칙이 같음을 코드로 확인했다).
- **평가기.** 손상 검출 계약 때문에 posting 합집합은 지금처럼 끝까지 읽는다. 원문 검증과 순위 계산은 구조적 후보(`structural_candidate`)나 경로 hit인 파일에만 한다. 후보가 아닌 파일은 어떤 원문 술어 결과로도 질의를 만족할 수 없어 결과·점수·`matched`가 같다. posting 커서가 선언한 DF를 넘는 항목을 거절하므로, 원문을 읽는 파일 수는 상한을 넘지 않는다. 리프프로그 탐색은 커서에 seek가 없고 어차피 posting을 끝까지 읽어야 해서 넣지 않았다.
- **수용 `VERIFIED`.**
  - 고정 4,097파일 fixture `conjunctive_admission`: `rare AND common AND file.contains` 1파일, 원문 읽기 1회, 기준 점수와 비트 단위로 같다. OR(상한 4,097)과 census가 필요한 NOT은 원문 0회 읽고 `ResourceLimited`. `common` 단독(4,096)은 수용된다. NOT·regex 조합도 정답과 맞는다.
  - 대수 골든과 건전성 전수 검사 2건.
  - 집중 nextest 107/107(레인), 기존 테스트 변경 0건. 합류 후 main 스냅샷 `b8889c28b`에서 오케스트레이터가 다시 실행한 structural·conjunction·universe·file_contains·anchored_source_predicate 선택 21/21 통과, exit 0.
  - 변이 검사: 구조적 후보 게이트를 끄면 테스트가 "AND가 본문 4,096개를 검증했는데 상한은 최소 DF 1" 메시지로 실패한다(레인 보고).
- **기존 결함 (A1과 무관).** 같은 스냅샷의 lexical lib 전체 746개 중 3개 실패는 A1을 되돌려도 똑같이 실패한다: `read_capability_admits_only_its_frozen_text_instance`, `native_root_json_is_funded_through_put_and_refusal_preserves_wire_commitment`, `excluded_change_drops_window_before_root_install`. lexical 기존 clippy 위반 177건과 `quanta-index-unicode` clippy 오류도 기존 상태다. A1이 추가한 줄의 clippy와 rustfmt는 깨끗하다.
- **남음.** 09.2b의 공개 SDK·CLI 실행은 09.1(레인 B)이 합쳐진 뒤 진행하고, 07 원장의 입장 정책 문구도 그때 갱신한다.

**11월 24일 09.1c lexical 합류 (레인 A2, main 작업 트리 반영·미커밋).**
- **의미.** `repo:<pattern>`은 레코드 저장소 id 문자열에 대한 비앵커·대소문자 구분 regex다. 여러 개면 모두 맞아야 하고 최대 4개다. `@rev`가 붙거나 패턴이 비면 `InvalidInput`이다. `lang:<id>`는 앞뒤 공백을 자르고 ASCII 소문자로 바꾼 `LanguageCode`와 정확히 같아야 하며, 별칭은 펼치지 않는다(옛 Quanta `normalize_lang_id`와 같다). 여러 `lang:`은 any-of다. 두 필터 모두 순위·top‑k 전에 적용한다.
- **실패 처리.** 후보 레코드에 저장소나 언어 메타데이터가 없으면 `InvalidInput`이고, 조용히 불일치로 넘기지 않는다. 오버레이와 함께 쓰면 오버레이에 인증된 메타데이터가 없으므로 거절한다(10.3 이후 재검토 대상). planner가 떨어뜨린 필터가 결과를 넓히지 못하도록 원 필터와 계획된 필터의 개수·순서가 같은지 확인한다.
- **수용 `VERIFIED` (레인 보고).** `metadata_filters` 3건(같은 상대 경로의 두 repo 분리, 낮은 점수의 유일한 python 파일이 `lang:python` top‑1, 메타데이터 없는 레코드 거절)과 preflight 1건. 집중 nextest 63/63. 수정 전 3건 실패를 확인했고, 메타데이터 누락을 불일치로 바꾸는 변이를 넣으면 거절 테스트가 실패하는 것도 확인했다. 바꾼 기존 테스트는 1건으로, 미지원 필터 예시를 `lang:rs`에서 `rev:main`으로 바꿨다. lexical 전체에서는 기존 3건만 실패한다.
- **남음.** 런타임 `language_any_of`를 LQ `lang`으로 내리는 일은 레인 B가 한다. 오버레이 행의 `repo:`·`lang:` 필터는 레인 C의 저장소 신원 위에서 후속 작업으로 진행한다.

**11월 24일 09.1 1차 합류 (레인 B1, main 작업 트리 반영·미커밋).**
- **구현.** CodeSearch 문법을 `quanta-index-contract/src/query/code_search.rs` 한 곳으로 옮겼다. 레거시 search-plane `lowering.rs`는 기존 오류 코드를 유지하는 얇은 wrapper가 됐다. Native용 `lower_code_search_native_query`가 추가됐다(리터럴→`file.contains`, regex→Content regex, `path:`→경로 필터, `select:path`). Runtime·lowering·SDK 페이지의 CodeSearch 거절 세 곳을 제거했다. SDK에 `native_code_search_request`가 생겼다. 새 의존 간선은 없다.
- **수용 `VERIFIED` (레인 보고).**
  - Native 데몬 `code_search_contract` 3/3: 레거시 frontdoor 단언, 대소문자·범위·같은 파일 AND·커서 마지막 페이지, 미지원 옵션의 typed 오류. 수정 전에는 3/3 실패를 확인했다.
  - 레거시 문법 포함 lib 84/84, SDK 레시피 블록 8/8.
  - 바꾼 기존 단언은 1건이다. 저장소 없이 stage한 fixture에서 CodeSearch가 이제 `SnapshotUnavailable`을 낸다.
  - 기존 결함(B1과 무관): SDK `exact_graph_reply_uses_only_its_funded_admitted_route`, Runtime `native_uds_source_lifecycle_status_old_view_and_reopen`. 둘 다 스냅샷 원상태에서도 실패한다.
- **오케스트레이터 감사: 09.1 미충족 항목.**
  1. 리터럴이 앵커 없는 `file.contains`로 내려가서, 4,096파일이 넘는 뷰에서는 리터럴만 있는 질의가 `ResourceLimited`로 거절된다.
  2. bare 단어가 본문만 매치한다. 레거시는 본문 또는 경로를 매치했다.
  3. 경로만 있는 질의를 거절한다.
  4. contains·regex에 점수가 없어 결과가 동점이고 repo·path 순서로 나온다.
  5. 공개 SDK `.text()` 빌더가 Native 연결에서 아직 `UnsupportedNativeQuery`를 낸다.
  6. `language_any_of`를 LQ로 내리지 않았다(B1 당시 preflight가 파일 단위 `lang:`을 거절했기 때문이며, A2 합류로 해소 가능).
  - 모두 틀린 답이 아니라 명시적 거절이라 합류는 했다. 5·6은 레인 B2에서, 1–4는 아래 09.1e에서 해결한다.
- **09.1e 계약 (레인 A, lexical 실행).**
  - gram 길이 이상인 `file.contains` needle은 Native Literal gram posting 교집합으로 후보를 정한다. case:no는 gram마다 대소문자 변형을 합집합으로 펼친다(Zoekt 방식). 단일 문자로 대응되지 않는 대소문자 매핑이 있으면 앵커로 쓰지 않는다. A1 상한 대수에서는 Term과 같은 후보 공급자가 된다.
  - 경로 메타데이터로 평가하는 표현식 leaf `path.contains`를 추가한다. 상한은 일치하는 경로의 정확한 개수다.
  - 경로만 있는 질의는 메타데이터 전용으로 실행한다.
  - contains 기반 질의의 점수 함수를 레거시 CodeSearch 정의에 맞춰 명시한다.
  - 이후 레인 B가 bare 단어를 `file.contains(t) OR path.contains(t)`로 내린다.

**11월 24일 통합 확인과 09.1e 결정.**
- **A·C 통합 확인 `VERIFIED`.** A1·A2와 C 10.3을 합친 스냅샷에서 lexical 집중 테스트 64/64, 오버레이·저장소 테스트 26/26, 런타임 `native_overlay_uds` 7/7(두 번 실행). 오버레이와 함께 쓴 `repo:`·`lang:`이 `InvalidInput`을 내는 회귀 테스트(`metadata_filters_refuse_overlay_queries`)도 합류했다.
- **09.1e 설계 결정 (레인 A 분석 반영).**
  - **Literal 접근.** Text 읽기는 고정된 뷰 안에서, 같은 Source owners에 묶인 유일한 Literal 형제 멤버만 쓴다. 멤버가 없거나 여럿이거나 묶인 Source가 다르면 앵커로 쓰지 않는다. 제품은 Literal descriptor를 `required`로 등록한다.
  - **gram 안전 규칙.** `file.contains`는 NFC와 대소문자 정책을 적용한 뒤 비교하므로 raw byte gram을 그대로 쓰면 일치를 놓친다(예: U+212A KELVIN→`k`, `İ`→`i`+U+0307, 분해된 `é`). 그래서 needle 문자가 정책상 단일 문자 raw 형태로만 만들어질 때만 gram에 안전하다고 본다. 안전한 문자 3개로 된 창을 모든 raw 변형으로 펼쳐 합집합을 만들고, 창끼리는 교집합을 취한다. 역변환 표는 검증 코드와 같은 정규화·대소문자 코드에서 만들고, 전체 Unicode scalar를 훑는 완전성 테스트로 증명한다.
  - **경로 leaf.** `path.contains`를 `native_content.rs` 술어 화이트리스트에 추가한다.
  - **순위.** BM25로 근사하지 않는다. 레거시 CodeSearch 성분 점수(경계·subtoken 보너스, 선언 보너스, 경로 근거, 근접도, 항 순서 tie-break)를 순수 함수로 옮기고, 레거시 단위 테스트를 독립 골든으로 쓴다. 품질 개선은 P2 지표로 판단한다.
  - **체크포인트.** E1 Literal 앵커, E2 `path.contains`와 경로 전용 질의, E3 순위 이식.

**11월 24일 09.1 2차 합류 (레인 B2 1부, main 작업 트리 반영·미커밋).**
- **`.text()` Native 경로.** 연결이 Native면 `LexicalQueryBuilder::execute`가 CodeSearch를 새 `client/native_code_search.rs`로 보낸다.
  - `active(repo, rev)`는 보호된 Source head를 정확한 뷰로 해석하고, 읽기 세션에서 path page를 한 번 읽는다.
  - 커서 토큰은 `native-code-search-path-v1:` + CBOR `NativeTextPathCursorV1`이다.
  - 오래된 pin은 `GenerationMismatch`, 다른 뷰의 커서는 `QueryCursorGenerationMismatch`, 형식이 틀린 토큰은 `CursorInvalid`이고, 그 밖의 Native 거절은 `SdkError::Native`다.
  - `candidate_id`는 레거시 골든과 같다.
- **`language_any_of`.** 런타임 lowering에서 언어마다 `LqFilter::Lang`을 만든다(모든 문법). 질의 안의 `lang:`과 동시에 쓰면 `InvalidInput`이다.
- **수용 `VERIFIED` (레인 보고).**
  - 공개 `.text()` 빌더로 레거시 frontdoor와 같은 호출을 하는 데몬 테스트 `code_search_frontdoor`를 포함해 데몬 4/4(두 번), 단위 32/32와 7/7.
  - 수정 전에는 `UnsupportedNativeQuery`와 언어 메타데이터 거절로 실패하는 것을 확인했다.
  - 기존 결함: `runtime_fast_suite`는 원 스냅샷부터 `lifecycle_tests.rs:363` `SymbolId` 미선언으로 컴파일되지 않는다.
- **조사 중.** 레인을 합친 스냅샷에서 `text_shard_contract` 3건이 게시 중 `Unknown(OperationKey … seq 2)`로 실패한다(B1 원 스냅샷에서는 통과). 레인 D가 스냅샷을 차례로 좁혀 원인을 찾는다.
- **감사 보완 지시 (B2 진행 중).**
  - text instance 설정 없이도 기본 `.text()`가 동작해야 한다. 프로필에서 제품 Text descriptor를 가진 유일한 인스턴스를 쓰고, 없거나 여럿이면 Usage 오류를 낸다.
  - 커서가 이후 게시에도 원래 뷰를 읽도록, 클라이언트가 읽기 세션을 보유하고 갱신한다(현재는 호출마다 열고 닫아서 페이지 사이에 회수될 수 있다).
  - Source 신원을 빈 값으로 두지 않는다.
  - snippet·line은 E3 전까지 "제공 안 됨" 상태로 명시하고, 0 같은 값을 넣지 않는다.

**11월 24일 09.1e E1 합류 (레인 A, main 작업 트리 반영·미커밋).**
- **Literal 형제 찾기.** `NativeTextIndex::with_literal_sibling(format)`이 설정돼 있을 때만, 고정된 뷰 멤버 중 같은 Source owners에 묶인 유일한 Literal 멤버를 쓴다. 멤버 키·binding·edge가 일치하지 않으면 `CorruptObject`, 없거나 여럿이거나 묶인 root가 다르면 앵커로 쓰지 않는다. 오버레이 읽기는 앵커를 쓰지 않는다.
- **안전 문자 표.** 검증 코드의 `nfc`·`apply_case`로 모든 Unicode scalar를 한 번 훑어 만든다. raw 문자가 NFC 결과로 분해·결합 없는 단일 scalar가 되고 대소문자 변환이 같을 때만 변형으로 인정한다. 다른 경로로도 도달할 수 있는 이미지 문자는 안전하지 않다. 의존성 `unicode-normalization`은 workspace 고정 버전이며 deny.toml이 허용한다.
- **앵커.** 안전 문자 3개로 된 창의 모든 변형 조합에서 UTF-8 trigram을 만든다. 상한이 가장 작은 창으로 후보를 만들고, 다른 창으로 거른다. 결과가 4,096을 넘거나 쓸 수 있는 창이 없으면 앵커를 쓰지 않는다. A1 대수에서 앵커가 있는 `file.contains`는 `Finite(|S|)` 후보 공급자다.
- **수용 `VERIFIED` (레인 보고).**
  - 4,097문서 중 3개에만 있는 "needle"은 본문을 3번 이하로 읽고 답한다. case:no `NEEDLE`도 같다. 형제가 없으면 거절하고, 여럿이면 앵커를 쓰지 않아 거절한다.
  - Kelvin·U+037E·분해된 é·그리스어·한자를 맞게 찾는다. `İ`는 앵커를 쓰지 않고 전수 검사로 찾는다.
  - 역변환 표의 완전성·건전성을 두 정책에서 전체 scalar로 확인했다.
  - lexical lib 전체 767/767. 앞서 기존 결함으로 기록한 3건도 이 스냅샷에서는 통과한다.
- **감사: 제품 경로 미연결.** 런타임 `NativeTextReader`가 Literal 형제 형식을 넘기지 않는다. 테스트 하네스의 프로필은 Text 인스턴스만 stage한다. 그래서 E1b에서 리더를 연결하고, Text에 Literal 형제가 설정되면 같은 Source의 Literal 인스턴스를 반드시 포함하도록 프로필 검증을 넣고, 4,096파일 초과 뷰에서 공개 `.text()`를 끝까지 실행하는 수용 테스트를 추가한다.

**11월 24일 09.1 B2 2부 (레인 B, 미합류·검증 중).**
- **구현 완료 (단위 테스트 `VERIFIED`, 데몬 테스트 `BLOCKED`→재실행 중).**
  - Text 인스턴스는 프로필에서 제품 Text kind를 가진 유일한 인스턴스로 자동 선택된다. 명시 override도 있고, 없거나 여럿이면 Usage 오류다. 다른 kind를 설정하면 제품 시작이 거절된다.
  - 클라이언트가 뷰별 읽기 세션을 보유하고 갱신한다. 이어지는 페이지는 `cursor.view`를 권위로 쓴다.
  - `LexicalCandidate.native_source`(owner와 Source ObjectRef)를 추가했다. 레거시 `source_sha256`은 파일 바이트 SHA라 의미가 달라 채우지 않는다.
  - `searchctl code-search`(repo·revision·`--language`·연속 페이지).
  - 09.1d 질의 완전성 테스트(AND·OR·NOT·우선순위·phrase·양의 항 없음·넓은 regex·count 거절·예산 만료와 취소).
- **차단과 조치.** 레인 스냅샷에는 `request_decode.rs:513`의 `let names` 컴파일 오류가 있었다. main에서는 다른 세션이 `let mut`로 이미 고쳐 두었다. 그래서 레인을 최신 main 스냅샷 `8b20dcb48`로 옮기고 패치를 다시 적용했다.
- **감사 보완 지시.**
  - 새 이름에서 V1 접미사를 뺀다.
  - Drop은 호출 스레드에서 I/O를 하지 않고, 세션 해제는 renewer 스레드에 맡긴다.
  - 세션 유지 테스트는 400ms 대기 대신 retention을 명시적으로 실행하고, 보유 세션이 없으면 실패하는 red 실행을 보인다.
  - line 번호를 `Option`으로 바꾸는 일은 E3 이후에 승인한다.
- **동시 편집 주의.** 다른 세션이 main 작업 트리를 함께 수정하고 있다(`native_history.rs`, `native_dispatch.rs`, `request_decode.rs`, `nov-24-native-enhancement/tickets/` 실행 패킷 Q01–V03). 그 패킷은 이 원장의 09·10 번호를 참조하고 상태를 여기서 관리한다. 레인 합류는 적용 전에 `apply --check`로 겹침을 확인한다.

**11월 24일 09.1e E1b (레인 A, 미합류).**
- **리더 배선.** `NativeTextReader::with_literal_sibling(format)`을 추가하고, 제품 리더가 `config.literal.format`을 넘긴다.
- **프로필 규칙.** `AdapterRegistry::require_companion(Text kind, Literal descriptor)`를 추가했다. Text 인스턴스가 있는 프로필에는 Literal 형식·kind·`CoreRequired`·`SourceLocal` 인스턴스가 정확히 하나 있어야 하고, 아니면 `InvalidInput`이다.
  - 이 규칙은 `validate_profile`에 있어서 stage, Reconfigure, 게시된 프로필의 부팅 검증에 모두 적용된다. 그래서 Text만 있는 기존 상태는 부팅되지 않는다(배포 전이라 허용).
  - 이 저장소에는 기본 제품 프로필이 없다. Semantica가 Text 옆에 serve 설정의 `literal` descriptor와 같은 kind·schema·format, `CoreRequired`, `SourceLocal` 인스턴스를 stage해야 한다.
- **E2E (작성, 미실행).** 4,097파일 중 3개에만 "needle"이 있는 뷰를 만든 뒤, 매치되지 않는 4,094개 본문 객체를 삭제하고 공개 `.text("needle")`을 실행한다. 성공하면 본문을 3개 이하로 읽었다는 증명이 된다. Text만 있는 프로필은 stage 단계에서 `InvalidInput`이다.
  - 4,097파일 FULL 게시가 부하 속에서 7분 넘게 걸렸다(07 원장의 알려진 FULL 비용). 그래서 프로토콜로 단계 수를 제한한 확정과 무거운 테스트 레시피로 정리하고, 통합 기준선에서 실행한다.
- **E1b 합류 (main·통합 기준선 `a446c120a`, 미커밋).**
  - companion 규칙은 red→green으로 확인했다(kernel `profile_` 24/24).
  - 제품 Text 프로필은 모두 Literal 인스턴스를 갖도록 테스트 fixture를 갱신했다. legacy candidate 16/16.
  - E1 결함 수정: 앵커 집합이 모두 비면 0바이트 메모리 예약이 생겨 제품 메모리 범위에서 `InvalidInput`이 났다. 0바이트 예약은 건너뛰도록 고쳤고, 테스트로 먼저 재현했다.
  - 제품 스위트 102건 중 11건 실패는 레인 변경을 뺀 기준선에서도 같다(legacy 바이너리 환경변수, socket 0700, drain, 30·120초 SourceAdvance 기한).
- **E2E `FAILED`(미수렴, 레시피 `just rust-test-native-literal-anchor`).** 600초 단계 1회 만에 transport 기한이 끝났다. 데몬 샘플 두 번 모두 Text·Literal 빌드 전, Source 설치의 `install_repository_path_maps`(map `apply_stream`, SQLite prepare)에 CPU 약 14%로 머물러 있었다. 기준선의 Large 테스트도 같은 단계에서 기한을 넘기고, 측정 당시 load가 약 200이었다. 레인이 모두 멈춘 뒤 다시 실행해, FULL 경로 지도 설치 비용인지 부하 탓인지 가린다.
- **E1c 합류 (main·통합 기준선 `7092e627a`, 미커밋).**
  - 레인 D와 A의 게시 확정 헬퍼를 `tests/common/publication_settle.rs` 하나로 합쳤다. `native_product_process`와 `native_index_uds`가 같이 쓴다.
  - 규칙: 그 단계의 예산이 `DeadlineExceeded`일 때만 `Transport` 오류를 결과 미확정으로 보고 `status`로 돌아간다. 기한이 남아 있을 때의 transport 오류와 `Protocol`은 실패다. `Unknown`은 같은 key일 때만 미확정이다.
  - 단위 테스트는 규칙을 끄면 실패한다. 두 바이너리에서 2/2. 공용 헬퍼로 바꾼 뒤 `native_boundary_delta…`(68초)와 dense layered(193초)가 통과했다.

**11월 24일 09.1 B2 3부 (레인 B, 통합 기준선 재검증 후 합류 예정).**
- **수용 `VERIFIED` (레인 보고, 이전 통합 기준선 `ddefe22b6`).**
  - lib 단위 101/101.
  - 데몬 8/8: `code_search_contract` 3, `code_search_frontdoor` 2(CLI E2E와 게시·retention 뒤 커서 유지 포함), `query_completeness` 2, 제품 Text kind 거절 1.
- **red 실행 `VERIFIED`.** 호출마다 세션을 열고 닫으면, retention이 고정되지 않은 뷰를 회수한 것을 관찰한 뒤 2페이지가 `SnapshotUnavailable`로 실패한다. 보호된 경로는 head 뷰를 살려두지 않는다. 그래서 클라이언트가 세션을 보유하는 구조가 필요하다.
- **세션 정책.** 서버는 확인된 Release 이후 활성 슬롯을 반환한다. Release ACK와 정산 기록은 메모리 상한과 TTL 안에서 보관하며, 보관 상한에 이르면 가장 오래된 유휴 기록부터 회수한다. SDK는 뷰마다 lease를 재사용하며 `Drop`은 renewer 스레드에 신호만 보낸다. 단발 CLI는 반환 전에 명시적으로 해제한다. [Q01 정책과 검증](../../nov-24-native-enhancement/tickets/Q01-sdk-codesearch.md#확장-frontdoor-실행과-세션-수리)
  - 서버 측 계수 방식은 해당 오너가 확인할 항목으로 남긴다. 해제와 동시에 한도에서 빼지 않으므로, 세션을 자주 여닫는 클라이언트는 5분 안에 한도에 걸린다.
- **오류 코드 일관성.** 계획할 수 없는 형태는 `Lexical(UnsupportedCombo)`, 진짜 파싱 실패는 `Lexical(ParseFail)`로 하나의 분류 함수(`native_query_refusal_code`)를 쓴다. 심볼 거절 테스트를 `UnsupportedCombo`로 강화했고, 수정 전에는 보고된 `ParseFail` 불일치로 실패했다.
- **합류 완료 (main·통합 기준선 `2d1d52f17`, 미커밋).** 통합 기준선 `778bb519e`에서 재검증했다. lib 104/104, 데몬 9/9(`code_search_contract` 3, `code_search_frontdoor` 2, `query_completeness` 2, `symbol_refusal`, 제품 Text kind 거절). 변경한 줄의 clippy·rustfmt는 깨끗하다. clippy를 돌리지 못한 대상은 하네스 `--all-targets`(`corpus.rs:1636`)와 `runtime_fast_suite`(`SymbolId`)로, 둘 다 원래 컴파일되지 않던 것이다.
- **09.1 체크리스트 현황.**
  - 완료: 09.1a(연결), 09.1b(공개 `.text()` 데몬 계약), 09.1b-CLI(`searchctl code-search`), 09.1c(repo·lang), 09.1d(질의 완전성).
  - 남음: 09.1e의 경로 매칭, 경로만 있는 질의, 레거시 순위(레인 A의 E2·E3)와 레거시 snippet·line 필드(E3 뒤 `Option` 타입).

**11월 24일 09.1e E2 합류 (레인 A, main·통합 기준선 `a379e58a0`, 미커밋).**
- **`path.contains`.**
  - Native 술어 화이트리스트에 추가했다. `PlanLeaf::PathContains`와 `Clause::PathContains`가 생겼다.
  - 고정된 뷰의 `source.paths` map을 한 번 훑는 census(`text_path_census.rs`)로 평가하고, 본문은 읽지 않는다. 경로마다 검증 코드의 `file_contains_mask`(NFC와 대소문자 정책)를 적용한다.
  - 후보 상한은 일치 경로의 정확한 개수(Finite)다. 그래서 `file.contains(t) OR path.contains(t)`는 `file.contains` 쪽에 Literal 앵커가 있을 때 유한하다.
- **경로만 있는 질의.** 빈 표현식에 `file:` 필터가 하나 이상 있으면 경로 메타데이터로만 실행하고, 필터가 없으면 `InvalidInput`이다.
- **수용 `VERIFIED` (레인 보고).**
  - 경로 매치는 본문 읽기 0회이고 대소문자 정책을 따른다. OR·NOT 조합도 맞다.
  - 4,097파일에서 `path.contains`는 읽기 0회, 앵커가 있는 OR는 본문 검증이 상한 3+2 이하다. 형제 인덱스가 없으면 읽기 0회로 거절된다.
  - preflight와 계약 파싱 테스트를 추가했다. 모든 테스트는 먼저 실패하는 것을 확인했다. lexical 782건 중 기존 3건만 실패한다.
- **`repository_query_contract` 정리.** 이 테스트는 A2 이전 계약(`repo:` 거절)을 고정하고 있었다. 새 계약은 이렇다: `repo:corp-a`는 필터가 적용되고, 메타데이터가 없는 레코드는 `InvalidInput`이며, `fork:`·`archived:` 등 레거시 repo 표면은 계속 거절한다. 미지원 술어는 정확한 `UnsupportedCombo`를 낸다.
- **레인 B가 낼 LQ 형태.**
  - bare 단어: `file.contains(t) OR path.contains(t)`
  - `content:t`: `file.contains(t)`
  - `path:t`: `path.contains(t)`
  - `path:regex:/re/`: `LqFilter::File{PathOnly}`
  - 경로만 있는 질의: `LqExpr::Empty`와 `file:` 필터
  - `path.contains`·`file.contains`·`file:`는 각각 최대 4개다.
- **감사 후속.**
  - E2b(P0): 오버레이 읽기에서 path leaf를 거절하면, 미저장 편집이 있는 IDE 세션의 기본 검색 전체가 거절된다. 오버레이 행은 인증된 경로로 직접 평가하고, durable 행은 오버레이가 가린 owner를 빼고 census로 평가한다.
  - P2: 경로 census는 질의마다 경로 map 전체를 훑는다. 경로 trigram 색인 후보다.
- **E2b 합류 (main·통합 기준선 `41cbbf19d`, 미커밋).** 오버레이가 있는 읽기에서도 path leaf를 평가한다.
  - 오버레이 행은 인증된 자기 경로로 `path.contains`, 경로 전용 `file:`, 앵커가 있는 `file.contains`를 판정한다.
  - durable 행은 census와 Literal 앵커로 판정하되, 오버레이가 가린 owner(수정, 삭제, 이름 변경 전 경로)는 오버레이 행이 우선한다.
  - 상한은 정확하다: 가려지지 않은 durable owner 수에 일치하는 오버레이 행을 더하고, Term·Phrase는 오버레이를 반영한 DF를 쓴다.
  - "오버레이 문서 4,096 초과" 거절은 앵커가 없을 때만 적용한다.
  - 수용 `VERIFIED`: 수정·생성·이름 변경·삭제와 두 repo의 같은 경로를 확인했다. 4,097문서에서 durable 본문 읽기는 5회 이하로 정확한 집합을 낸다. 테스트는 먼저 실패하는 것을 확인했다. lexical 787건 중 기존 3건만 실패하고, 런타임 7/7.
- **E3 착수.** 레거시 selected-file 점수(경계·경로 100/75/40+35/15, occurrence `2*(min(n,4)-1)`, exact_case 5, proximity `32-gap`)를 순수 함수로 옮기고, 레거시 단위 골든을 쓴다. 파일마다 대표 매치 위치(바이트 범위와 줄)를 함께 내서 레인 B가 snippet·line에 매핑한다.

**11월 24일 09.1e B5 합류: CodeSearch lowering을 E2 형태로 변경 (main·통합 기준선 `6e441c512`, 미커밋).**
- bare 리터럴 `t`는 `file.contains('t') OR path.contains('t')`, 여러 단어는 단어별 OR의 AND다. `content:t`는 `file.contains`, `path:t`는 `path.contains` leaf(case:yes가 아니면 대소문자 무시), `path:regex:/re/`는 `LqFilter::File{PathOnly}`다. 표현식 leaf가 없으면 `LqExpr::Empty`와 `file:` 필터가 된다.
- 레거시 경로 전용 단언 `path:regex:/beta[.]rs/` → `src/beta.rs`를 frontdoor 테스트에 원래대로 복구했다. 경로에만 있는 단어(`alpha`, `readme`)도 찾는다.
- Native leaf 한도(`file.contains`·`path.contains`·`file:` 각 4개)를 넘으면 SDK가 I/O 전에 `LexicalErrorCode::LimitExceededFanout`(`CodeSearchQueryError::NativeFanout`)으로 거절한다. SDK를 우회하면 데몬이 `InvalidInput`을 낸다.
- 의도한 의미 변경: `case:yes context`는 `context.rs`의 경로에도 일치하므로 `[context.rs, lower.rs]`다.
- 수용 `VERIFIED`: 테스트는 먼저 실패하는 것을 확인했다(계약 4건, 데몬 4건). 계약 9/9, search-plane lowering 50/50, 런타임 lowering 7/7, E2E `code_search` 5/5, `text_matches` 3/3.
- 남음: snippet·line의 `Option` 타입(E3 뒤), lexical 한도 상수를 계약 상수 하나로 통일(C10 2단계).

**11월 24일 09.1e E3 합류: 레거시 CodeSearch 순위 (main·통합 기준선 `63af10475`, 미커밋).**
- `text_code_search_rank.rs`는 레거시 selected-file 점수 함수만 순수 함수로 옮겼다(d8a490565e6f `searcher/code_search.rs`). 경계 100/75/40과 경로 basename +35·그 외 +15, occurrence(겹침 포함, 최대 4), exact-case +5, 같은 scope의 두 개 이상 항에 대한 proximity `32-gap`이다. 검증된 Source 바이트와 인증된 경로에서 계산하므로 추가 읽기가 없다. 정렬은 점수 내림차순, repository, path 순이다.
- 적용 형태: `select:path`이고 `file:` 필터가 없으며, 항 하나이거나 항들의 `All`인 질의. `file.contains`는 Content, `path.contains`는 Path, 같은 needle의 `file.contains OR path.contains`는 Both다. 그 밖의 질의는 기존 BM25를 쓴다.
- 대표 매치 `CodeSearchMatch`: `Content{start,end,start_line,end_line}`(원본 바이트 범위와 1-based 줄) 또는 `Path{start,end}`. 레거시의 1차 witness 규칙을 따르고, 비 NFC Source는 `MappedText`로 원본 바이트에 대응시킨다. 대응표가 예산을 넘으면 질의 전체를 `ResourceLimited`로 거절하고, 조용히 `None`으로 두지 않는다.
- 수용 `VERIFIED`:
  - 레거시 단위 골든과 고정 점수를 재현한다(single 100, many 106, adjacent 231, distant 200, plain 233, folded 105, path 135, İ 40/45/40, NFD 40). 점수 함수를 stub으로 바꾸면 실패한다.
  - 공개 SDK `.text()` 데몬 테스트 `legacy_rank_order`의 순위가 손으로 계산한 레거시 점수와 같다. 점수 함수를 끄면 실패한다.
  - lexical 804건 중 기존 3건만 실패하고, 런타임 17/17.
- **B5와 결합한 end-to-end `VERIFIED` (통합 기준선 `63af10475`).** basename에만 있는 항은 135(대소문자 무시 시 140), 디렉터리에만 있는 항은 115(120), 본문에만 있는 항은 100(105)이고, `path:gamma case:yes`는 135와 115로 손 계산과 같다. 데몬 9/9, lexical 805건 중 기존 3건만 실패한다. 이 케이스들은 B5와 E3가 이미 만든 동작을 고정하는 것이라 red 단계는 없었다. 테스트 파일은 E4 체크포인트와 함께 합친다. 4,097파일 무거운 E2E는 부하(load 67–443)로 49분 동안 끝나지 않아 중단했다(`NOT_RUN`).
- **감사 후속 E4.** 레거시는 범위 없는 regex를 Both로 매치하고 점수를 매겼다. 지금은 regex를 본문에만 매치하고 점수가 0이다. 경로 metadata에서 평가하는 `path.regex` 표현식 leaf를 `FieldRegex`로 추가하고, `best_regex_in`을 이식한다. 레인 B는 lowering을 바꾼다.
- **B8 예약.** 대표 매치를 wire와 SDK의 `start_line`·`end_line`·snippet(레거시 snippet 구성을 그대로 이식)에 연결한다.

**11월 24일 09.1e E4 합류: 레거시 regex parity (main·통합 기준선 `66daf2ec3`, 미커밋).**
- `path.regex` 표현식 leaf를 추가했다. 계약 화이트리스트와 `MAX_NATIVE_PATH_REGEX_LEAVES`=4를 두었다. NFC 경로에서 비앵커 `FieldRegex`로 검색하고, case 정책은 Content regex와 같다. `path.contains`와 census, 정확한 상한, E2b 오버레이 의미를 공유한다.
- 레거시 `best_regex_in`을 이식했다. NFC 원형에서 leftmost-first로 검색하고, 매치가 4,096개를 넘으면 `ResourceLimited`, 폭 0이면 `InvalidInput`이다. regex 항에는 exact-case가 없고 proximity가 꺼진다. Content, Path, Both(본문 witness를 쓰되 경로 점수가 더 높을 때만 경로로) 세 scope를 처리하고, 추가 본문 읽기는 없다.
- 수용 `VERIFIED`:
  - 레거시 단위 골든 `cached_literal_score_preserves_earlier_regex_witness_and_rejection` 재현.
  - 경로 regex `beta[.]rs`는 135, 경로 바이트 4..11(레거시 frontdoor 하이라이트 start 4, length 7).
  - `.text()`로 레거시 frontdoor regex 질의 3건의 파일과 점수를 확인했다(`sphinx regex:/quartz/`는 대소문자 무시 리터럴 exact-case +5로 205).
  - 경로 전용 regex 형태는 Native 세션 페이지로 직접 확인했다.
  - E3 결합 Both·Path 케이스(135/140, 115/120, 100/105)를 다시 확인했다.
  - 테스트는 먼저 실패하는 것을 확인했다(26건 중 8건). lexical 814건 중 기존 3건만 실패, 계약 20/20, 런타임 15/15.
- 남음: B9(regex lowering을 E4 형태로 바꾸고 레거시 경로 regex snippet 단언 복구), 09.4b 구현, 09.2b 무거운 SDK·CLI 수용(레인 A).

**11월 24일 B7 합류: 줄·excerpt Option 타입 (main·통합 기준선 `2c386d011`, 미커밋).**
- `LexicalCandidate`의 다섯 필드를 `lines: Option<LexicalLineRange>`와 `excerpt: Option<LexicalMatchExcerpt{snippet, snippet_hit_offset, highlights}>`로 바꿨다. 둘로 나눈 이유는 경로 매치가 snippet은 있지만 줄이 없어서, 하나로 묶으면 가짜 줄 `0-0`을 넣어야 하기 때문이다.
- wire: 값이 없으면 key를 생략하고, key 순서는 같다. 레거시 행(둘 다 `Some`)은 바이트 단위로 같다(손으로 쓴 골든). 그룹 일부만 있으면 decode에서 거절하고, preview는 excerpt를 요구한다. 커서의 줄 key도 값이 없으면 생략한다.
- Native `.text()`는 이제 `None`을 낸다(이전에는 0·빈 문자열). 레거시 생산자는 `Some`을 그대로 유지한다. 소비자(search-plane 정렬·DSL·hybrid seed, searchctl 렌더, harness, benchmarks)는 값을 지어내지 않고 typed 오류를 내거나 출력을 생략한다.
- 수용 `VERIFIED`: contract-base 128, contract 64, core 25, search-plane 671, searchctl 71, E2E `code_search` 6/6. 기존 결함: SDK `exact_graph_reply…`, 다른 크레이트 테스트 대상의 기존 컴파일 오류.
- 다음: B8(데몬이 검증된 바이트로 레거시 `file_candidate` ±120바이트 excerpt를 만들고, closed `code_search` per-hit wire, SDK 매핑, Path는 레거시 `set_path_preview` 형태). 09.4b 캐시 항목이 `CodeSearchMatch`를 함께 들고 있어서 캐시된 페이지도 witness를 다시 계산하지 않는다.

**11월 24일 H 단계 (헬게이트 정적 감사·보강) 착수.** 각 레인은 진행 중인 체크포인트를 끝낸 뒤 자기가 합친 코드 전체를 다시 적대적으로 정적 감사한다. 그다음 싼 단위 테스트(고정 seed property, brute-force oracle 대조)로 보강하고, 약한 기존 테스트(red를 거친 적 없음, 개수만 단언, 출력에서 기대값을 얻음)를 강화한다. 결함은 test-first로 고친다. 범위는 다음과 같다.
- H-A: 구조적 상한, 메타 필터, Literal 앵커, companion 규칙, path leaf와 오버레이, CodeSearch 점수
- H-B: CodeSearch 문법과 lowering, 세션 수명, 경로 규칙, 매치 wire와 커서, telemetry, Option wire, excerpt
- H-C: 오버레이 신원과 저장, 매치 스캔, fold·필수 리터럴, 오버레이 필터, regex 엔진 과금
- H-D: Reuse, pin 정산, 퇴역 루트, maintenance demand, reclaim, 기한 검증, 정체 감지

이와 별도로 읽기 전용 독립 감사 에이전트가 통합 기준선 diff 전체를 티켓 완료 기준과 대조한다.

**11월 24일 독립 감사 결과 (통합 기준선 `2c386d011`, 읽기 전용, 코드 대조).** 결함 10건을 레인별로 넘겼다. #1과 #2는 오케스트레이터가 코드에서 직접 확인했다.
1. (P0, 조용한 오답) `.text()` 이어 읽기가 revision에 묶이지 않는다. 커서에 revision이 없어 `feature`로 요청해도 `main` 커서의 뷰를 읽는다. → B
2. 매치 스캔 regex 과금(2MiB Temporary)이 기본 meta 엔진의 보유량 상한이 아니다. → C
3. E07 증명 공백: retention이 라운드당 scope 하나만 처리하고, 라운드가 실패하면 demand를 잃으며, 검증이 제품 JSON 경로에서만 돈다. 문서가 실제 보장보다 과장돼 있다. → D
4. public-api baseline이 손으로 고친 일부만 반영돼 있다(10.1·B7·core 신규 항목 누락). 재생성은 툴체인 승인이 필요해 `BLOCKED`이다. → B가 신규 공개 항목 목록을 정리한다.
5. 범위 없는 regex가 본문에만 매치되고 `path:regex:`가 순위를 끈다. → B9
6. 시간 기반 대기(retention 관측 `sleep` 폴링)와 출구 없는 `wait_drained_after`. → B, D12
7. 09.1d 수용이 부분적이다. → B
8. path census 비용 누수: 선택적인 형제가 있어도 census 소유자를 전부 열거한다. → A(구동 자식만 census를 쓰고 나머지는 후보별 확인)
9. 원장 불일치(10.1·10.3 체크, B4의 통합 기준선과 main 구분, `page_cost`가 결함을 고정하는 임계값). → 원장 수정, A(09.4b)
10. 새 버전 이름 `FilePathInventoryV3`와 커서 태그 `-v1`. → D, B

확인된 항목(감사): Reuse 건전성(Text·FileMap·Literal), 후보 대수와 컴파일된 앵커의 일치, 오버레이 virtual DF, 필수 리터럴 추출, 10.1 커서의 `NativeSourceIdentity` 결속, 10.3 저장 검사, D2c 정산, `FieldRegex` 과금, 비테스트 코드에 `unwrap`·`expect` 없음.

**감사 결함 처분.** 11월 24일 17:03 기준이다.

| # | 상태 | 근거 |
| --- | --- | --- |
| 1 | **통합 기준선 합류 · main 대기** | 레인 B 체크포인트를 `8270c7f89`에 합쳤다. `.text()` 이어 읽기 토큰은 `CodeSearchContinuation{revision_id, cursor}`이다. 저장소·revision이 다르거나 고정 sequence가 커서 뷰와 다르면 `QueryCursorGenerationMismatch`, 맨 커서는 `CursorInvalid`로 거절한다. 오케스트레이터가 SDK `continuation()`을 코드에서 직접 확인했다. 테스트: `continuation_is_bound_to_its_repository_revision_and_pinned_view`. `lexical().matches`는 이미 view·instance·query digest에 묶여 있고, SDK가 `selection_contains`로 확인한다. |
| 4 | **BLOCKED (툴체인)** | 재생성할 공개 항목 목록을 정리했다. contract: `NativeCodeSearchMatch`·`NativeCodeSearchExcerpt`·`NativeTextPathPageV1::code_search`·`validate_code_search`·`MAX_NATIVE_CONTENT_REGEX_LEAVES`·`CodeSearchNativeLeaf::{ContentRegex, PathRegex}`(`PathFilter` 제거)와 상수 두 개. searchd-runtime에는 baseline 파일이 없다. 손으로 고치지 않고 `nightly-2026-08-01` 승인 뒤 재생성한다. |
| 5 | **통합 기준선 합류 · main 대기** | B9에서 범위 없는 `/re/`는 `Any([Regex, path.regex])`, `content:/re/`는 `Regex`, `path:/re/`는 `path.regex`로 내린다. `file:` 필터로 내리는 경로와 `(?i)` 주입은 없앴다. `path:regex:/beta[.]rs/`는 `src/beta.rs`를 스니펫 `"src/beta.rs"`(offset 4, 길이 7)와 레거시 점수 135로 돌려준다. path regex도 공통 flag 정규화를 거친다(H-B F1: `case:yes path:regex:/(?i)x/`가 조용히 대소문자를 무시하던 결함, red→green). |
| 6 (B 부분) | **통합 기준선 합류 · main 대기** | `NativeRetentionObservation::wait_completed_after(mark)`로 retention 관측의 `sleep` 폴링을 없앴다. 반환은 완료 2라운드, 연속 미완료 2회 뒤 `Stalled`, 펌프 종료 시 `Stopped` 중 하나다. 레인 D의 회수 관측과 중복되는 상태 기계는 H-B에서 하나로 합친다. |
| 7 | **부분 · H-B·H-C 진행** | 질의 완전성 골든을 CLI `search-text`에서도 실행한다. `count:all`·`count:0`은 typed `UnsupportedCombo`(CLI exit 3)로 거절한다. 스캔 중 취소는 N번째 객체 읽기마다 취소해 거절 또는 전체 페이지만 나오는지 확인한다. 넓은 regex `/./`는 낮은 work 예산에서 typed 거절된다. 남은 것: `MAX_UNANCHORED_DOCS` 구조 규칙 단위 테스트(레인 C). |
| 10 (B 부분) | **통합 기준선 합류 · main 대기** | 통합 기준선에서 두 태그를 변경했다. 현행 Q01은 기존 `native-code-search-path-v2:`와 알려진 CBOR byte 키를 유지한다. 별도 매치 경로는 `native-text-matches:`와 정규 인코딩 검사를 사용한다. |
| 2 | **통합 기준선 합류(`889925026`) · main 대기** | 매치 스캔이 `regex::RegexBuilder`와 2×1MiB Temporary 예약 대신 `meta::Regex` + 소유 `meta::Cache`를 쓴다. 생성 중에는 `DOCUMENT_CONSTRUCTION_BYTES`를 예약하고, 생성 뒤에는 `engine_retained_bytes(regex.memory_usage())`만큼 lease를 regex 수명 동안 보유한다. 오케스트레이터가 `text_match_scan.rs`에서 확인했다. Literal 경로도 같은 엔진을 쓰므로 함께 고쳤다. red 관측: engine 2,248 + cache 2,098,848 > grant 2,097,152. 테스트: `match_engine_and_search_cache_stay_within_their_grant`, `refused_compilations_release_every_grant`, `match_scan_equals_a_line_by_line_regex_oracle`(8코퍼스 × 24질의, CRLF·Kelvin·ß·ſ·İ·ı·Σ·ς 포함). lexical의 중복 상수 `MAX_NATIVE_CONTENT_REGEX`는 지우고 contract의 `MAX_NATIVE_CONTENT_REGEX_LEAVES`로 통일했다. 구조 규칙 단위 `TextOwnerCursor::check_candidate_bound`와 4,096/4,097 경계 테스트(Term·Phrase·AND·AND NOT·AND regex·OR, 단독 NOT·regex는 `check_statistics`)도 추가했다. |
| 8 | **통합 기준선 합류 · main 대기** | 레인 A 체크포인트를 `80fa6c01b`에 합쳤다. 부정 path leaf는 census를 돌리지 않는다. Literal 앵커 상한이 4,096 이하인 질의는 census 없이 후보마다 인증된 경로로 path leaf를 판정한다. 그 밖의 census leaf는 4,096에서 멈추고, 넘으면 거절하지 않고 후보별 필터로 바뀐다. 오케스트레이터가 `text_query.rs`의 census 정책을 코드에서 확인했다. 테스트: `a_non_driving_path_leaf_filters_candidates_without_a_census`(넓은·선택적·부정 leaf 모두 census 0회). 수정 중에 생긴 회귀(path leaf가 더 작은 구동자인 `file.contains('let ') path.contains('needle_0100')`이 거절됨)도 같은 테스트로 닫았다. |
| 9 | **원장 수정 · 통합 기준선 합류** | 원장 불일치는 오케스트레이터가 고쳤다. `page_cost`의 이차 임계값을 없앴다. 대신 캐시 끔·켬 데몬의 바이트 동일성과, 이후 모든 페이지에서 `재스캔 − 캐시` 차이 D가 k=10과 k=100에서 같은 상수이며 N에 따라 증가함을 단언한다. 측정: N=22에서 D=94, N=86에서 D=345, 페이지마다 일정. N=1,000 표는 무거운 `page_cost` 레시피에서 나오며 아직 **NOT_RUN**이다. |
| 3 | **통합 기준선 합류 · main 대기** | 레인 D 체크포인트를 `cdabf4b89`에 합쳤다. 해제된 scope는 FIFO 대기열에 들어가고, `k`개는 성공한 라운드 `k`번 안에 처리된다. 실패한 라운드는 Views 차례와 demand를 유지한다. 검증은 모든 서비스 바인딩에서 돌고, `scopes × retention.interval + 2 × reclaim.interval ≤ 60s`를 강제한다. 저장소 안의 위반 설정 5곳은 고쳤다. 모듈 문서는 보장하는 것과 검증할 수 없는 전제를 나눠 적는다. red→green 테스트: `released_scopes_run_in_release_order_one_round_each`, `retention_rounds_scale_with_configured_scopes`, `failed_release_round_retries_the_views_turn`, `programmatic_retention_refuses_reclaim_rounds_below_view_roots`. |
| 6 (D 부분) | **통합 기준선 합류 · main 대기** | D12에서 `wait_drained_after(mark)`가 연속 2회 정체 pass 뒤 `Stalled { rounds }`를 반환한다. 정체 pass는 큐 처음부터 끝까지 적용한 단계가 없고 경합 키가 하나 이상인 pass다. 경합 키도 examined로 집계되므로 "examined 없음"으로는 감지할 수 없어, 기준을 pass로 바꿨다. 남은 대기 경로는 모든 회수 라운드가 자원 거절돼 라운드 기록이 없는 경우다. H-D에서 다룬다. |
| 10 (D 부분) | **통합 기준선 합류 · main 대기** | `FileMapCapability::FilePathInventoryV3`를 `FilePathInventory`로 바꿨다. wire 코드 2는 유지한다. `COMMIT_DOMAIN`의 `filemap-v3` 바이트는 저장된 root commitment의 일부라 유지한다. |

**11월 24일 독립 재감사(`2c386d011..5c159ff19`, 읽기 전용, 코드 대조).** 이전 결함 10건의 수정 주장을 코드에서 대조했다. 확인된 것은 다음과 같다.
- #2: 검색 캐시 상한
- #3: deadline 검증과 모든 바인딩 경로
- #6: 정체 규칙과 벽시계 대기 제거
- #8: census 결과가 전체 census와 같음
- #9·09.4b: 순위 캐시 키·무효화·동점 처리·바이트 동일성
- #10, C14 merge-join 정확성, 09.2b 구조 규칙
- red 정리 golden(recipe digest를 독립 재계산해 신·구 모두 재현)
- 정규 커서 토큰, `validate_code_search` 줄 규칙(CRLF·`\r`·다중 줄 오거절 없음), `"edges"` 범위

새 결함은 11건이다. 모두 레인에 넘겼다.

| ID | 등급 | 내용 | 담당 |
| --- | --- | --- | --- |
| R1 | P1 | 해제된 scope의 라운드가 계속 실패하면 대기열 0번으로 되돌아가 다른 scope 전부를 굶긴다. 예: epoch가 바뀐 뒤 옛 epoch 해제로 `SnapshotUnavailable`이 영구 반복된다. 그러면 모든 scope의 E07 60초 상한이 깨진다. #3은 **부분**이다. 오케스트레이터가 `native_retention.rs:336-341`에서 확인했다. | B · **해결, 통합 `dff4679c1`** |
| R2 | P2 | retention 대기에 출구가 없다. Declined에는 scope 고유인 `SnapshotUnavailable`도 들어 있어, 문서의 근거가 틀렸다. | B · **해결, 통합 `dff4679c1`** |
| R3 | P3 | 펌프 초기화가 실패하면 retention 관측에 `Stopped`가 오지 않는다. | B · **해결, 통합 `dff4679c1`** |
| R4 | P2 | 문서 불일치: FIFO로 바뀐 뒤에는 완료 라운드 2회가 재검토를 보장하지 않는다. | B · **해결, 통합 `dff4679c1`** |
| R1–R4 해결 내용 | — | 이 행은 위 네 결함의 처리 내역이다. 오케스트레이터가 `run_round`에서 확인했다. ① 실패한 대기 turn도 demand와 Views 단계를 유지한 채 순환의 맨 뒤로 간다. ② 해제는 대기 scope 또는 한 바퀴를 기다린 scope 뒤에 들어간다. 이로써 대기 scope는 n라운드 안에, 모든 scope는 2n라운드 안에 차례를 받는다. 감사에 없던 기아도 함께 닫혔다: 매 라운드 해제가 오면 해제되지 않은 scope가 굶던 문제다. ③ floor 단계의 `SnapshotUnavailable`은 epoch가 현행이 아니라는 뜻이라 영구적이다. 그래서 그 demand는 지우고 재시도하지 않는다. 카탈로그는 workspace epoch를 바꾸지 않으므로 catalog 변경은 필요 없었다. ④ retention 정체는 모든 scope가 한 번씩 차례를 받는 pass 단위로 판정한다. 완료 turn이 없는 pass가 2회 연속이면 `Stalled`다. ⑤ 릴리스를 기다리는 호출자는 `wait_released_after`(해제 backlog가 비는 것)를 쓴다. ⑥ close guard는 실패할 수 있는 단계보다 먼저 만든다. 남은 대기 경로: admission이 영구 거절되는 펌프는 라운드를 기록하지 않는다. reclaim과 같은 원칙이다. 검증: runtime lib 71건, `native_index_uds` 15건, `physical_mixed_pressure` 3/3 통과. |
| R7·R8·R10·R11 해결 내용 | — | 통합 `2190e3afd`에 합류했다. R7: 순위 캐시를 세션 registry의 Interactive 예약 안에 선언 몫(`text_order_memory`)으로 둔다. 설정 시점에 plan으로 검증하고, 실행 중에는 이미 예약한 바이트의 하위 원장에서만 과금하므로 다른 응답을 밀어내지 않는다. 몫이 부족하면 캐시를 두지 않고 재스캔하며, 1바이트 몫에서도 페이지 바이트와 읽기 수가 캐시 끔과 같다. **결정 필요:** 제품 설정 `read_sessions.text_order_memory_bytes`가 없으면 0(캐시 끔)이다. 몫을 기본으로 파생하면 harness 혼합 설정(`global_limit` 128)이 시작 시 거절되기 때문이다. 제품 기본값을 켤지 정해야 한다. R8: 상수를 `MAX_UNANCHORED_DOCS`에서 파생한다. R10: renewer를 읽기 전에 확보하고, 퇴역 lease 해제 실패는 페이지 결과로 만들지 않는다(daemon TTL에 맡김). ScriptedLeases 걸음에 `Lapse` 단계를 넣어 red→green을 확인했다. R11: 변이 클래스마다 정확히 단언한다(과반 임계 제거). |
| R5·R6·R9 해결 내용 | — | 레인 C 체크포인트를 통합 기준선에 합류했다.

**R5.** `lq_regex::engine_construction_bytes(config)` 하나로 생성 예약을 계산한다. text_regex와 match scan이 같이 쓰며, regex-automata 0.4.14의 NFA 빌드에서 유도했다.
- 빌드 수: 정방향 1개, hybrid이면 역방향 1개, ReverseInner 조건이면 1개를 더한다.
- peak: (빌드 수 + 1) × L + onepass. 빌드 중인 builder와 그 출력 NFA가 동시에 존재하기 때문에 1을 더한다.
- full DFA를 켠 설정은 `None`으로 거절한다.
- 측정해 보니 이전 21MiB 예약은 ReverseInner가 없어도 모자랐다. `[\x{80}-\x{10FFFF}]{11000}` 하나가 약 28.9MB였다.
- HIR 파싱도 `admit_parse`로 파싱 전에 예약한다.
- 테스트: `construction_bound_covers_every_nfa_the_engine_builds`, `pattern_parse_waits_for_its_grant`. 둘 다 red→green이다.

**정책 변경, 승인 필요.** `ENGINE_NFA_SIZE_LIMIT_BYTES`를 10MiB에서 5MiB로 낮췄다.
- 이유: 정직한 상한이 41MiB가 되면 64MiB interactive 예산에 content regex 4개가 들어가지 않는다.
- 결과: 같은 21MiB 상한이 이제 건전하다.
- 대가: NFA가 5–10MiB인 패턴은 거절된다. 이 범위에 의존하는 기존 테스트는 없다.

**R6.** 후보를 창 단위로 거른다. 조합마다 자기 probe(gram 10개 이하)로 sweep하고, sweep이 끝나면 블록을 놓는다. 따라서 동시에 보유하는 블록 수는 needle 길이와 무관하다.
- 확인: `contains` 호출 집합이 이전과 같다. 무작위 동치 테스트와 상위집합 테스트는 계속 green이다.
- 테스트: `long_needle_holds_a_bounded_number_of_posting_blocks`(4,096자 needle, 4MiB 상한), red→green.
- 읽기 수: ASCII는 그대로다. 멀티바이트 hazard 코퍼스에서는 exact +15%, fold +31%지만 probing보다는 여전히 적다.

**R9.** 주석을 고쳤다.

**검증과 남은 항목.**
- 통과: lexical lib 831/0, lq-regex 전부, `native_overlay_uds` 11건.
- **NOT_RUN:** `legacy_rank_order` 2건은 부하 830에서 IPC timeout이 나서 확인하지 못했다. |
| R5 | P1 | 매치 스캔 생성 예약이 ReverseInner의 세 번째 역방향 NFA를 빠뜨려 최대 약 31MiB가 필요한데 21MiB를 예약한다. HIR 메모리도 예약하지 않는다. #2는 **부분**이다. | C |
| R6 | P2 | C14 merge-join이 gram마다 블록을 동시에 보유한다. 4,096바이트 needle이면 약 128MiB로 거절될 수 있다. 결과는 정확하다. | C |
| R7 | P2 | 세션 순위 캐시가 Interactive 메모리를 세션 수명 동안 보유한다(최대 약 19MB). 시작 시 집계 검증이 없다. | A |
| R8 | P3 | `MAX_TEXT_RANK_ORDER`가 같은 값을 중복 정의한다. | A |
| R9 | P3 | 테스트 주석의 키 길이 설명이 틀렸다. | C |
| R10 | P2 | 기존 결함이다. 퇴역 lease 해제가 실패하면 성공한 페이지를 잃고 새 lease가 고정된다. | A |
| R11 | P3 | 커서 변조 테스트가 과반만 거절되면 통과한다. | A |

**통합 게이트(`5c159ff19`, 레인 D).**

- **회귀 테스트.** lexical lib 829/0, repomap lib 48/0, runtime lib 75/0이 통과했다. `native_index_uds`는 57건 통과, 6건 실패였다.
  - 실패 5건은 history 테스트이고 `QUANTA_INDEX_LEGACY_SEARCHD_BIN`이 없어 **BLOCKED**다.
  - 나머지 1건 `read_sessions::query::hybrid::hybrid_dense_only_original_view_partial_coverage_and_provider_refusal_keep_one_pin`은 **FAILED**다. 3/3 결정적으로 실패하며, 레인 합류 뒤 생긴 회귀다. hybrid 검색이 embedding provider에 도달하지 않는다.
- **레인이 만든 컴파일 오류.** `quanta-index-semantic --all-features`가 컴파일되지 않는다. `ann_proof.rs:466-468`이 B의 CodeSearch 합류에서 제거된 `LexicalCandidate` 필드를 쓴다.
- **스냅샷부터 있던 컴파일 red.**
  - lq-bridge 테스트 3건
  - retrieval-bench `sdk_roundtrip`
  - searchd-harness `corpus.rs` `json!` 반복 문법
  - runtime `e2e_integrity_scrub.rs` 미사용 import
- **clippy.** `-D warnings`는 스냅샷부터 있던 기초 crate(unicode, installed-artifact-trust, build.rs 3곳)에서 멈춘다. `--cap-lints warn`으로 돌리면 레인이 추가한 줄의 진단은 4건(문서 단락 3건, 불필요한 반환 1건)이다.
- **조치.** 회귀 2건, 스냅샷부터 있던 컴파일 red, 레인 줄의 clippy는 D와 A에 넘겼다. 기초 crate의 deny lint는 이번 범위 밖 후속으로 남긴다.

**H-B2 합류(`5c159ff19`, 네 레인 최종 head).**

- **관측 상태 기계 통합.** reclaim과 retention의 관측을 `NativeProgressObservation<K>` 하나로 합쳤다. 동작은 mark 뒤 목표 라운드 2회, 연속 정체 라운드가 상한에 이르면 `Stalled`, 펌프가 끝나면 `Stopped`다. 오류 타입은 `NativeProgressWaitError` 하나다.
- **retention은 정체로 판정하지 않는다.** 거절(Refused)과 예산 거절(Declined)은 다른 주체가 class나 예산을 쥔 결과이지 scope가 멈췄다는 증거가 아니다. 따라서 retention 대기는 완료 라운드 2회가 오거나 펌프가 끝날 때만 끝난다. reclaim의 자원 거절 라운드와 같은 원칙이다.
- **상태 기계 전수 검증.** 4^6개 이력 전체를 규칙과 대조했다.
- **H-B에서 새로 찾은 결함 3건.** 각각 red를 확인한 뒤 고쳤다.
  - SDK 세션 표의 lease 누수. 새 뷰의 첫 페이지가 거절되면 퇴역 lease를 해제하지 않고 반환했다. 가짜 transport로 seed 64개 × 24단계를 돌려 소유 규칙 참조 모델과 대조해 찾았다.
  - 이어 읽기 토큰의 별칭. 대문자 hex 토큰이 원래 커서로 디코드됐다. 이제 정규 재인코딩만 받는다. 오케스트레이터가 `decode_token`을 확인했다.
  - `code_search` 줄 번호 미검증. 이제 매치 안의 개행 수는 `end_line - start_line`과 같아야 하고, 시작 줄은 excerpt 안의 앞선 개행 수와 맞아야 한다. wire 변이 23건 중 구조 변이 16건이 거절된다.
- **SDK `exact_graph_reply`.** 기준선 `2c386d011`에서도 실패했다. `5452e8cdb`가 디코더 식별자에서 `"edges"`를 뺀 것이 원인이었고, graph 응답에만 허용하도록 되돌렸다.
- **검증.** contract 15건, SDK 32건, runtime lib 28건, `native_index_uds` 20건(1건 ignore)이 통과했다. `physical_mixed_pressure`는 3회 모두 통과했다.

**기존 red 정리 합류(`8d39c70aa`).** 각 레인이 "기존 실패"로 넘기던 red를 레인 A가 추적해 닫았다. 테스트 파일과 의존 코드가 스냅샷 `802ac88a2`와 바이트 동일한 것을 확인했고, 원인은 `git log -S`로 찾았다. 모두 main 커밋 `f08990c96`(path posting 도입)과 `5452e8cdb`에서 왔다.

| red | 원인 | 조치 |
| --- | --- | --- |
| lexical `excluded_change_drops_window_before_root_install` | fixture가 퇴역한 "제외 owner는 map edit 없음" 동작을 단언했다. 지금은 모든 Source owner가 path posting을 쓴다. | 독립 oracle `excluded_path_edits`가 만든 정확한 edit와 대조한다. |
| lexical `native_root_json_is_funded…` | `Root` wire에 path 필드 3개가 늘었는데 gold가 그대로였다. | gold를 현행 wire로 바꿨다. 상한·거절 단언은 그대로다. |
| lexical `read_capability_admits_only_its_frozen_text_instance` | Tree recipe digest에 `PATH_POLICY`가 들어갔는데 golden은 그대로였다. | Python으로 digest를 독립 재계산해 golden을 바꿨다. 이전 digest가 `InvalidInput`으로 거절되는지도 단언한다. |
| runtime `runtime_fast_suite` 컴파일 오류(`SymbolId`) | import 정리 누락 | import를 추가했다. 실행하면 10건 통과, 30건은 `QUANTA_INDEX_LEGACY_SEARCHD_BIN`이 없어 **BLOCKED**다. |
| repomap lib 테스트 컴파일 오류 4건과 lint 2건 | `node_key` 누락, `encode_compiled_payload` 경로 이동, `facts_epoch` 미사용 결과, 레인 D 합류 `9b0c9b9c6`이 남긴 dead helper | import 2건을 고쳤다. `facts_epoch`는 이제 `TYPED_FACTS_ROOT_EPOCH`와 같아야 한다(강화). dead helper는 삭제했다. |

결과: lexical lib 823/0, repomap lib 48/0이다. 테스트 삭제·`#[ignore]`·단언 완화는 없다. main에서는 Codex가 repomap 두 파일을 이미 같은 방향으로 고쳐서, 이 패치는 통합 기준선에만 적용했다.

**H-D 합류(`67476ab37`).**
- `physical_mixed_pressure`의 마지막 해제 테스트와 `oversized_wal_reclaim` 자동 분기에서 벽시계 대기(`SETTLE_TIMEOUT`, 단계별 deadline, `sleep`)를 모두 없앴다. 이제 펌프 관측으로 기다린다. reclaim은 `wait_drained_after`, retention은 `wait_completed_after`다. 부하 상태에서 3/3 통과했다.
- 고정 seed 64개 property 테스트 세 개를 넣었다.
  - 임의 cascade가 정확히 `ceil(S/steps)` 라운드 안에 빠지는지 본다.
  - 임의 경합에서 `Stalled`가 영구 경합 키가 있을 때만 나오고 400라운드 안에 판정되는지 본다.
  - 임의 해제·실패 순서에서 FIFO 모델과 같게 처리되는지 본다.
- 변이 M1–M5(라운드 중 재시작이 경합 무시, 재시작 비활성, LIFO 삽입, 실패 시 대기 해제, 정체 임계 1)는 모두 해당 테스트를 red로 만든다.
- 자원 거절 라운드는 아무 단계도 실행하지 않고 기록도 남기지 않는다. 거절 횟수로는 큐가 막혔음을 증명할 수 없어 typed 결과를 두지 않았고, 이 근거를 코드 문서에 적었다. 레인 B의 retention 관측은 거절을 `Stalled`로 센다. 두 관측을 하나로 합칠 때 이것이 오탐 원인이 될 수 있는지 확인한다.

B8도 `8270c7f89`에 합류했다. Path 페이지가 행마다 `code_search` 매치(본문 매치는 검증된 원문의 ±120바이트 문자 경계 excerpt, 경로 매치는 경로 범위)를 싣고, SDK `.text()`가 이를 lines·snippet·highlight로 옮긴다. 레거시 frontdoor 스니펫 단언을 복원했다. 그 과정에서 데몬 원장 결함 두 건(witness 목록 grant 수명, excerpt 단계의 종료된 메모리 scope)을 고쳤다. 검증: contract 14, SDK 29/30(실패 1건 `exact_graph_reply…`는 기존 실패 주장이고, H-B에서 기준선 재현으로 확인한다), runtime lib 9, searchctl 23, `native_index_uds` 질의 4묶음 13건 통과(`--test-threads=2`).

main 대기 사유: B8·B9 이후 레인 B 패치는 main에 없는 B4(`PENDING-main-b4`) 위에서 만들어져 함께 대기한다. Codex가 main의 `native_retention.rs`·`native_release_deadline.rs`에 같은 1a 수정(대기 scope FIFO, `scopes.len()` 항)을 독자적으로 진행 중이라 패치가 적용되지 않는다. 레인 D 패치는 `PENDING-main-d-noop-gc-symbol` 대기 패치로 보관했다. Codex 편집이 멈추면 3-way로 합친다. 검증 범위: runtime lib 60건 통과, `native_index_uds` 보존·회수 22건 통과, history 5건은 `QUANTA_INDEX_LEGACY_SEARCHD_BIN`이 없어 **BLOCKED**, `native_product_process` 회수 6/6 통과. `physical_mixed_pressure`의 마지막 해제 테스트는 부하 상태에서 1회 실패한 뒤 재실행 2/2 통과했다. 벽시계 대기라 H-D에서 결정론적으로 바꾼다.

**11월 24일 13:37 세션 재시작.** 오케스트레이터 세션이 재시작되면서 scratchpad의 레인 worktree가 삭제됐다. 합류한 작업은 main 작업 트리와 `refs/lanes/integration`(`cebabfca0`)에 모두 남아 있다. 레인 worktree는 같은 경로에 통합 기준선으로 다시 만들었다(빌드 캐시 유지). 잃은 것은 레인별 미합류 진행분(A의 E3 결합 확인 테스트와 E4, B의 B7, C의 C13 측정, D의 D11)이고, 각 레인이 다시 진행한다. B4의 main 대기 패치는 통합 이력(`457010a04`)에서 되살렸다. main을 동시에 편집하던 다른 주체는 Codex CLI(ChatGPT 앱)이며, main checkout에서 `just rust-test-native-literal-anchor`를 돌리고 있었고 이 원장과 계획 문서도 함께 갱신한다.

**11월 25일 main 합류 (통합 기준선 `1a3f51bbb`, main 작업 트리 반영·미커밋).** 통합선이 main보다 앞서 있던 상태(131파일)를 정리했다. 01:40 호스트 재부팅으로 `/private/tmp`의 레인 작업분이 지워져, 레인 worktree를 `~/Library/Caches/quanta-index-lanes/`로 옮겨 다시 만들었다.

- **재조정.** 레인 B가 main 스냅샷 `0bc9747bf`와 통합선 `bed4b6d54`을 파일별 3-way로 합쳤다. 대상 133파일 중 fast-forward 35, 깨끗한 3-way 65, 수동 해소 20, 레인 신규 12, 문서만 바뀐 fast-forward 1이다.
  - 수동 해소의 판단 기준은 다음과 같다.
    - `native_retention.rs`는 Codex 설계(해제 뷰 범위, 순수 round-robin)를 유지하고, 감사 수정 항목(`SnapshotUnavailable` 시 범위 정리, 정체 관측, 라운드 보고)을 그 위에 이식했다.
    - SDK `native_code_search.rs`는 Codex의 커서 ID 세션 수명 위에 B8 매치·excerpt, 정규 토큰 검사, 변조 테스트를 이식했다.
    - wire 번호는 Codex가 77–81과 read-session 17·18을 먼저 써서, 레인 쪽을 TextMatches 82, SearchTextMatches 19로 옮겼다.
  - 스냅샷 기준선으로 3-way를 다시 돌린 교차 확인에서, 깨끗하게 합쳐진 파일은 모두 바이트 동일했다. 유실 줄 감사에서 text-matches 연결 5파일이 빠진 것을 찾아 복원했다.
- **레인 D 합류.** 통합 `4e15a0098`, `1a3f51bbb`다. 범위는 다음과 같다.
  - semantic `ann_proof.rs`를 `LexicalLineRange`·`LexicalMatchExcerpt` 계약으로 옮겼다. semantic producer(`search.rs`)가 쓰는 값과 같다.
  - 스냅샷부터 있던 컴파일 오류를 고쳤다. lq-bridge 테스트 3건은 import를 lq-norm으로 바꿨고, retrieval `sdk_roundtrip`은 harness의 `legacy_reference`를 쓰게 했다. harness `json!` 반복 문법, `snippet.rs`의 `NativeTextPlanBacking` 비교, 미사용 import도 정리했다.
  - hybrid dense-only 테스트의 5초 `recv_timeout` 대기를 blocking 채널과 예산 만료 시각 기준 해제로 바꿨다. 기준선 `bed4b6d54`에서 3/3 실패, 수정 뒤 8/8과 3/3 통과다.
  - 레인 D가 저장한 패치는 lq-bridge 두 파일의 새 import를 빠뜨리고 있었다. 오케스트레이터가 재검증 중에 발견해 `1a3f51bbb`로 보완했다.
- **main 반영.** 재조정 결과에 레인 D를 얹고, 재조정 기준선·live main·결과로 파일별 3-way를 했다. 104파일 중 99파일은 깨끗하게 합쳐졌다. 나머지 5파일은 재조정 이후 Codex가 다시 바꾼 것이라 수동으로 합쳤다.
  - `Justfile`: Codex의 layered 레시피와 레인의 09.4b 레시피를 함께 둔다.
  - `literal_blocks.rs`·`text_literal_anchor.rs`: Codex의 `MembershipCache`(8슬롯 블록 캐시)와 레인의 `LiteralGramProbe` merge-join이 같은 문제를 각각 풀었다.
    - 제품 경로는 merge-join이다. 블록마다 최대 한 번 읽기 때문이다.
    - per-owner 경로는 테스트 oracle(`finish_anchor_probing`)로만 남았다. 그래서 Codex의 `contains_grams`와 `MembershipCache`는 `#[cfg(test)]`로 옮겼다.
  - SDK `decode_token`: Codex의 trailing CBOR 거절과 레인의 정규 토큰 거절을 모두 적용한다.
  - runtime literal_anchor 테스트: import 합집합.
  - 이전 반영에서 retrieval 테스트 3파일에 `native_source` 필드가 두 번 들어간 오류와, Codex의 상세 `SdkError::Native` 분기 뒤에 남은 도달 불가 분기도 정리했다.
  - 반영 전 main 파일 90개는 `~/Library/Caches/quanta-index-lanes/main-prelanding-backup.tar`에 보관했다.
- **검증.**
  - 재조정 트리 workspace check: `0bc9747bf` 기준선 대비 새 오류 0, `SymbolId` 오류 1건 해소. **VERIFIED**.
  - main에 반영된 crate 6개(retrieval-bench, lq-bridge, searchd-harness, lexical, sdk, semantic) `check --all-targets`: 오류 없음. **VERIFIED**.
  - main 테스트:
    - lq-bridge 78+4+25+1 통과.
    - lexical lib 842 통과, 실패 1(아래 Codex 항목), 1 ignore.
    - SDK lib 359/0 통과(별칭 거절 결정 반영 뒤).
  - Codex의 `packed_literal_membership_reuses_authenticated_blocks`는 "자금이 있으면 블록 캐시로 읽기가 절반 미만"을 단언했다. 제품 경로가 merge-join으로 바뀌어 메모리와 관계없이 9회로 같아졌으므로 전제가 사라졌다.
    - 같은 fixture로 `packed_literal_membership_reads_blocks_not_owners`로 바꿨다. 자금 유무와 관계없이 읽기 수가 같고, 일치 owner 128개보다 적어야 한다.
    - 제품 finish를 per-owner oracle로 바꾼 변이에서는 389회 대 6회로 red다. 원복 뒤 `literal_anchor::` 10/10 통과.
  - 레인 D 트리 테스트:
    - lq-bridge 78+4+25+1, semantic `--all-features` check, contract fan-out 1건, hybrid 3/3: **VERIFIED**.
    - retrieval `sdk_roundtrip` 컴파일: **VERIFIED**.
  - 재조정 트리 테스트:
    - lexical lib 836/0(1 ignore), repomap lib 49/0, SDK lib 선택 21/0: **VERIFIED**.
    - `native_index_uds` `read_sessions::query` 40 통과.
- **남은 red와 BLOCKED.**
  - main workspace의 남은 컴파일 오류는 모두 이번 반영이 건드리지 않은 Codex 진행 중 파일이다. search-plane `native_history/build.rs`, `history_source/tests.rs`, storage `native_object_namespace.rs`, runtime `ann_enospc.rs`, contract `source_only_v6.rs`다. 이 때문에 main에서는 runtime lib·search-plane lib·contract lib 테스트를 실행할 수 없다.
  - Codex 쪽 테스트 실패는 2건이 남았다(세 번째 SDK 항목은 해결).
    - `native_pump::release_cadence_tests::released_view_range_survives_single_row_pages_and_a_remaining_pin`: 손대지 않은 `0bc9747bf`에서도 같은 단언에서 실패한다.
    - lexical `layered_two_segment_mixed_phrase_and_keyword_preserves_positional_score`: `TokenMemoryScope`가 Temporary만 허용하는데, 최종 hit 버퍼의 Retained 예약(`0bc9747bf`부터 존재)에 걸린다. 테스트 본문, scope, 예약 코드가 반영 전 main과 바이트 동일하다. 코드 동일성으로 판단했고 실행으로 확인하지는 않았다(PLAUSIBLE).
    - SDK `tampered_continuation_tokens_are_refused_or_another_bound_cursor`: **해결됨.** 반영 뒤 Codex가 `decode_cursor`에 V2 byte-string 필드 식별자 허용을 넣어, 같은 커서에 대한 다른 토큰(별칭)이 통과했다.
      - 사용자 결정(11월 25일)은 별칭 거절이다. 인코더는 처음부터 text 식별자만 썼으므로, byte 식별자를 받아들인 것은 디코더의 관용일 뿐 실제로 발급된 토큰은 없다.
      - `decode_cursor`는 이제 정규 토큰 검사를 하는 `decode_token` 하나만 쓴다. 필드 visitor는 text 식별자만 받는다.
      - Codex의 `cursor_v2_wire_…` 테스트는 byte·혼합 식별자 토큰이 `CursorInvalid`로 거절되는지 단언하도록 바꿨다. 정규 map은 계속 같은 커서로 디코드된다.
      - SDK lib 359/0 통과. fmt와 변경 줄의 clippy 진단은 없다.
  - harness `tail::tests::structural_tail_selects_a_tree_query`는 **FAILED**다. `f08990c96`에서 추가된 테스트가 warm 기대값을 선언하지 않은 시나리오에 `Some(1)`을 단언한다. harness lib가 그동안 컴파일되지 않아 처음 드러났다. warm fixture의 정답은 레거시 데몬 없이 확인할 수 없어, 기대값을 추정으로 채우지 않았다.
  - `QUANTA_INDEX_LEGACY_SEARCHD_BIN`이 없어 **BLOCKED**인 것: harness ops·ui 2건, `process_readiness_owner_v1` 210건, history 7건.
  - 재부팅 뒤 Xcode 라이선스 미동의로 `/usr/bin/{cc,git,python3}`가 종료 코드 69로 실패한다. 빌드는 `DEVELOPER_DIR=/Library/Developer/CommandLineTools`로 우회했다. 영구 해결은 `sudo xcodebuild -license`이며 사용자 조치가 필요하다.

**전환 게이트.** 09.1과 09.2는 계획의 P0 전환 조건이다. [08.1](08-release-activation.md) T 후보 수용과 08.7 마감에 포함된다. 09.3은 09.3a에서 게이트로 정한 경우에만 포함되고, 09.4는 P1이라 게이트가 아니다. IDE·에이전트 기능의 공백은 [10](10-native-ide-agent-gaps.md)이 맡는다.

**11월 24일 담당 공백 확인.** 이 디렉터리와 [Native 증분 비용 원장](../../../bugbash/nov-20-native-delta-cost/tickets/), [벤더 퇴역 계획](../../nov-23-vendor-retirement/README.md)에서 `CodeSearch`·`Zoekt`는 0건이었다. 본문 질의 DF 상한은 [07](07-product-qualification.md#native-lexical-rca)에 **의도된 입장 정책**으로 기록돼 있고, 4,097문서 경계 거절은 `VERIFIED`다. 보강 계획은 같은 상한을 보완 대상으로 보므로, 정책 결정이 먼저 필요하다.

## 09.1 · SDK·CLI 기본 `.text()`를 Native에 연결

**11월 26일 Q01 인계.** 후속 집중 40건과 실제 CLI exact 1건을 로컬에서 확인했다. Open/Release 실패 정리, 기본 300초의 Runtime 재생, 보호 경로 교체 뒤 이전 리비전의 별도 CLI 프로세스 이어 읽기를 포함한다. 같은 승인 후보의 설치 수용은 V03에서 진행한다. [Q01 후속 실행](../../nov-24-native-enhancement/tickets/Q01-sdk-codesearch.md#잔여-코드와-재현-조건)

**현행 코드.** Native 연결의 기본 `.text().execute()`는 repo·revision이 가리키는 보호된 Source head를 해석하고, 그 뷰의 읽기 세션으로 파일을 조회한다. 기본 Text 인스턴스는 프로필에서 찾고, 공개 CLI는 같은 SDK 경로를 사용한다. `execute_observed()`는 동일한 Native 경로에서 실제 foreground RPC의 요청 ID와 IPC 시간을 기록한다. 최초 페이지의 해석·세션 열기·프로필·검색과 보유 세션의 다음 페이지 검색을 각각 관측한다. 갱신과 정리의 I/O는 별도다. [SDK 실행](../../../../crates/quanta-index-sdk/src/lexical.rs#L1082) · [Native 경로](../../../../crates/quanta-index-sdk/src/client/native_code_search.rs) · [CLI](../../../../crates/quanta-index-searchctl/src/native/code_search.rs)

**페이지 보유 감사.** SDK v2 커서는 revision과 SDK 커서 ID를 함께 보존한다. v1 커서는 첫 페이지부터 다시 조회해야 한다. 이미 끝난 커서를 재전송해도 다른 페이지의 보유권을 줄이지 않으며, 진행 중인 페이지는 유휴 세션 회수 대상에서 빠진다. 갱신 응답은 시작할 때의 세션 ID·revision과 현재 항목이 일치할 때만 반영한다. 현재 작업 트리의 SDK 단위 선택 `4b0457db-3107-451f-9c44-34c6de5149f3`는 **VERIFIED · 9/9 · exit 0**이다. 이 선택에는 Graph의 `edges` 허용 목록 회귀와 Native 경로 동점 페이지 회귀도 포함했다. 공개 SDK 데몬 본문은 [07](07-product-qualification.md#native-lexical-rca)에 기록했다. [SDK 커서와 세션](../../../../crates/quanta-index-sdk/src/client/native_code_search.rs) · [Graph 허용 목록](../../../../crates/quanta-index-sdk/src/native_uds/admitted_reply_v1.rs)

**갱신과 진단 후속 검증.** 갱신 응답이 유실되거나 로컬에서 취소되면 SDK는 마지막으로 ACK를 받은 revision과 진행 중인 커서를 보존한다. 다음 갱신은 그 revision의 재생을 요청한다. 서버의 `SnapshotUnavailable`이 세션과 revision에 일치할 때만 세션을 없앤다. `quanta-index-ipc`·`quanta-index-sdk` 집중 선택 `d3e027f3-a3a8-4699-b313-2f049f2e304a`에서 진단, 갱신 및 유료 History preview 회귀가 **VERIFIED · 15/15 · exit 0**이었다. [SDK 갱신 적용](../../../../crates/quanta-index-sdk/src/client/native_code_search.rs) · [IPC 관측](../../../../crates/quanta-index-ipc/src/server.rs)

실제 Native UDS 서비스에 연결한 Nextest `fe61a7ff-d32e-44e6-bf2d-ba61212e7f68`의 SDK 커서 본문은 **PASS · 16.155초**였다. 첫 페이지의 요청 ID 1–4, 다른 revision의 사전 거절, 후속 게시와 보유 뷰의 다음 페이지 요청 ID 5, 일반 실행과 같은 마지막 결과를 확인했다. 같은 선택의 고정 BM25 기준 본문도 **PASS · 4.323초**였다. [Native UDS SDK 본문](../../../../crates/quanta-index-searchd-runtime/tests/native_index_uds/read_sessions/query/code_search_frontdoor.rs#L645)

- [x] **09.1a 결정.** Native 전환 후 기본 `.text()`가 (1) CodeSearch를 Native Literal·Text·FileMap 계획으로 lowering할지, (2) 명시적 오류와 대체 경로(`native` DSL)로 퇴역할지 정한다. 퇴역이면 영향받는 호출자와 오류 형태를 [04](04-maintained-test-migration.md)·[05](05-bench-callers-hosted-ci.md)의 퇴역 목록에 기록하고 09.1b는 거절 계약 테스트로 대체한다.
- [x] **09.1b 공개 골든의 로컬 수용.** 고정 원문의 독립 파일 정답은 **11/11**이다. 세션 회수 수리 후 Nextest **18/18**에서 공개 SDK·CLI의 문법·대표 결과·마지막 페이지·명시적 오류와 실제 CLI 프로세스 간 커서를 확인했다. 승인 후보 수용은 [07](07-product-qualification.md)과 V03에서 이어간다. [Q01 실행과 수리 근거](../../nov-24-native-enhancement/tickets/Q01-sdk-codesearch.md#확장-frontdoor-실행과-세션-수리)
- [x] **09.1b-CLI 기본 진입점.** Native `searchctl code-search`는 repo·revision을 공용 SDK의 고정 뷰로 해석해 기본 CodeSearch 질의를 실행한다. CLI 문법·대표 위치·출력의 확인 범위는 Q01에 기록하고, 전체 공개 골든은 09.1b에서 인수한다. [CLI 명령](../../../../crates/quanta-index-searchctl/src/lib.rs#L474) · [Native CLI 구현](../../../../crates/quanta-index-searchctl/src/native/code_search.rs#L27) · [Q01](../../nov-24-native-enhancement/tickets/Q01-sdk-codesearch.md)
- [x] **09.1c 범위 필터.** 일반 `repo:<pattern>`과 최상위 `lang:`을 검증된 Source 메타데이터로 순위 전에 적용하거나, 메타데이터가 없는 이관·복원 뷰에서 명시적으로 거절한다. 같은 상대 경로를 가진 두 repo, 낮은 점수의 유일한 허용 언어 결과를 포함한다. [Native 필터 입구](../../../../crates/quanta-index-lexical/src/native/text_preflight.rs#L262)
- [ ] **09.1d 질의 완전성.** 기본 SDK·CLI 실행에 이어 공개 계약의 전체 골든을 인수한다. 실행 중 취소와 넓은 regex의 예산 거절을 동일 공개 입구에서 확인한다. 기존 지원 DSL의 `AND`·`OR`·`NOT`, 괄호 우선순위, 인접 토큰 phrase를 독립 골든과 대조한다. 양의 검색어가 없는 질의와 후보가 넓은 regex는 수용과 예산 거절을 각각 확인한다. `count:all`과 `count:0`은 현재 입구에서 거절되므로, 이 거절이 SDK·CLI 호출자에게 명시적 오류로 보이는지 확인한다. Native Text의 `matched`는 선택한 투영의 전체 결과 행 수이고, `top_k`·`count:N`은 반환 행만 제한한다는 점을 결과로 검증한다. 예산 초과·취소·미완료를 0건이나 완료로 표시하면 실패다. [불리언 입력](../../../../crates/quanta-index-lexical/src/native/text_preflight.rs#L149) · [count 거절](../../../../crates/quanta-index-lexical/src/native/text_preflight.rs#L262) · [Text 결과 계약](../../../../crates/quanta-index-contract/src/native_protocol.rs#L1145)

**담당** 기존 SDK·caller 오너([05](05-bench-callers-hosted-ci.md))와 TEXT planner 오너, CLI는 `quanta-index-searchctl` 오너, Root/F 공용 등록. **상태** SDK·CLI 연결, 고정 revision 커서와 범위 필터는 확인했다. 09.1b 공개 골든은 로컬에서 통과했다. 09.1d 전체 수용과 최종 후보 검증은 남았다. 최신 통합본 및 출하 수용은 [07](07-product-qualification.md)과 [08](08-release-activation.md)의 실행을 따른다.

## 09.2 · 본문 질의 DF 사전 거절의 정책 결정과 목표치

**현행 코드.** 본문 predicate는 구조적 후보 상한을 사용한다. AND는 유한한 분기의 최솟값, OR는 유한한 분기의 합으로 계산한다. 인증된 경로와 Literal 앵커도 유한 후보를 공급한다. 전체 census가 필요한 분기에는 4,096 상한을 유지한다. 선택된 posting의 검증은 끝까지 수행한다. [후보 상한](../../../../crates/quanta-index-lexical/src/native/text_query.rs) · [후보 집합](../../../../crates/quanta-index-lexical/src/native/text_universe.rs) · [07 입장 정책](07-product-qualification.md#native-lexical-rca)

Literal·경로 앵커의 합집합은 필요한 전체 공간을 계산해 한 번 예약하고, 실제 할당 용량이 예약을 넘으면 `ResourceLimited`를 반환한다. 128바이트 예산으로 두 후보 집합의 중복 제거와 예약 반환을 검사하는 단위 회귀를 추가했다. [후보 메모리 회귀](../../../../crates/quanta-index-lexical/src/native/text_universe.rs)

**수용 fixture (고정).** 전체 4,097파일. `common`은 4,096파일에, `rare`와 고유 본문 표지는 그중 1파일에만 넣는다. `rare AND common AND file.contains('::needle')` 또는 동등한 본문 regex를 사용해 후보 입장을 실제로 통과시킨다. 두 term의 DF 합은 4,097, 정답은 1파일이다. 정답은 `rg -l`로 독립 확인한다.

DF 상한 수용은 SDK `NativeUdsClient::search_text_paths`와 CLI `search-text --syntax native`에서 같은 뷰에 `rare AND common AND file.contains('::needle') select:path`를 보낸다. 기본 `.text()`와 `code-search`는 `content:rare content:common content:"::needle"`로 같은 경로를 확인한다. 기본 문법은 본문 리터럴로 lowering하므로 DF 상한과 점수의 비교는 Native LQ 요청끼리 수행한다. Tree의 기존 4,097파일 fixture에 이 공개 경로, 정규식 분기, 입력 바이트로 독립 산출한 경로, 넓은 OR의 `ResourceLimited`와 CLI 출력 부재를 추가했다. [SDK·CLI fixture](../../../../crates/quanta-index-searchd-runtime/tests/native_index_uds/read_sessions/query/literal_anchor.rs)

Segmented Full V1 evaluator 선택은 Nextest `cb110079-6a5b-403e-8388-a4c14ba00a88`에서 **VERIFIED · 1/1 · exit 0**이다. 조건식은 본문 한 건을 읽었고 상한 초과는 본문 읽기 전에 거절했다. 이 선택을 `just rust-test-native-lexical-unit`에 포함했다. 공개 fixture의 생성 규칙을 임시 디렉터리에 재현해 `rg -l -F`로 `rare` 1건, `common` 4,096건, `::needle` 1건과 교집합 `src/f0011.rs`를 확인했다. [evaluator fixture](../../../../crates/quanta-index-lexical/src/native/text/tests/conjunctive_admission.rs) · [실행 레시피](../../../../Justfile)

Tree의 공개 수용 첫 실행 `b002048a-35fb-44c1-81ae-6caa626d9fcb`는 테스트 본문에서 보호 대상 repository의 Source operation `sequence=2`가 `Unknown`으로 끝나 **FAILED · 0/1 · exit 100**이었다. 보호 뷰 fixture의 mutation 페이지와 마지막 seal에 별도 준비 기한을 주고, 재실행에서 작업 단계와 기한 상태를 출력한다. 이 변경은 fixture의 본문 게시 준비에 적용한다. [revision 게시 fixture](../../../../crates/quanta-index-searchd-runtime/tests/native_index_uds/read_sessions/query/protected_revision.rs)

준비 기한을 적용한 실행 `9c5ba8c6-65e3-4ee9-a781-c0dd6b9d5dc7`에서는 `sequence=2`가 Catalog에 `Pending`으로 기록됐다. Native의 120초 dispatch 상한과 새 fence 재개 경로를 대조했고, 같은 FULL이 반복돼 고유 staged object가 1,061개에 머무는 상태를 확인했다. 이 실행은 검색 전에 명시적으로 SIGINT 종료했다. 후속 fixture는 256파일씩 Native Source를 게시해 최종 보호 뷰에 같은 4,097파일을 노출한다. 각 operation의 조정에는 하나의 전체 기한을 사용한다. [Native dispatch 정책](../../../../crates/quanta-index-searchd-runtime/src/native.rs#L132) · [게시 fixture](../../../../crates/quanta-index-searchd-runtime/tests/native_index_uds/read_sessions/query/protected_revision.rs)

분할 fixture의 첫 빌드는 Storage의 `ScratchAllocation for Arc<FileScratchAllocation>`에서 E0117로 **FAILED · compile exit 101 · 본문 0건**이었다. Storage가 소유한 로컬 핸들이 `Arc<FileScratchAllocation>`을 보유하도록 고쳤다. Nextest `ea08a76c-db7f-4200-b59b-ed90514af719`에서 기존 예약 회귀와 namespace lease가 살아 있는 동안 경로·예약을 유지했다가 마지막 lease 해제에서 정리하는 회귀가 **VERIFIED · 2/2 · exit 0**이었다. [Storage 구현과 회귀](../../../../crates/quanta-index-storage/src/scratch.rs)

수정 후 `native_index_uds` 빌드는 완료됐다. Nextest `1c8dd14b-2b06-400b-bab7-8c26787e678a`에서 4파일 레거시 순위 회귀의 세 번째 질의 `sphinx regex:/quartz/`가 IPC 읽기 timeout으로 **FAILED · exit 100**이었다. 먼저 실행한 두 정규식의 고정 점수 단언을 지난 뒤 발생했다. Fail-fast로 나머지 순위 회귀와 4,097파일 공개 수용은 **NOT_RUN**이었다. 링크한 동일 바이너리에서 세 본문을 각각 끝까지 실행하도록 다시 접수했다.

동일 바이너리에서 `sphinx regex:/quartz/`의 timeout을 재현했고, 뒤이어 `alpha case:yes` 첫 요청도 같은 IPC 오류를 반환했다. 서버 샘플에서 첫 folded Literal 앵커가 요청 스레드의 `OnceLock` 안에서 Unicode scalar 전체를 `BTreeMap`으로 만들고 `retain`하는 것을 확인했다. 같은 normalizer의 NFC·fold 역상을 빌드 단계에서 생성하고, 런타임은 예외·모호성·자기 자신으로 변환되는 문자만 정적 조회하도록 바꿨다. 양쪽 case의 전체 Unicode scalar 역상과 모호성, Kelvin·점 있는 대문자 I·결합 문자의 기존 규칙을 검증한다. [빌드 생성기](../../../../crates/quanta-index-lexical/build.rs) · [Literal 앵커와 회귀](../../../../crates/quanta-index-lexical/src/native/text_literal_anchor.rs)

원래 바이너리의 4,097파일 케이스는 분할 게시 진행을 확인한 뒤 새 빌드에 레인을 넘기면서 중단했다. 수정 후 기존 정규화 라이브러리와 생성기·조회 코드를 연결한 분리 진단은 고정 Unicode 사례와 두 모드의 전체 scalar 역상·모호성 **2/2 · exit 0**이었다. 첫 프로젝트 빌드는 `-D unused-results`에서 거절돼 반환값을 처리한 후 재접수했다. `just rust-test-native-lexical-unit`에 역상 회귀를 포함했다. 수정 후 공개 SDK·CLI 본문 결과를 별도로 판정한다.

프로젝트 Nextest `deb82ed6-f1be-4318-b48d-90b6da9cd4da`에서 Segmented Full V1 후보 입장, 고정 Unicode 사례, 두 모드 전체 역상·모호성의 **3/3 · exit 0**을 확인했다. `./scripts/cargow --lane test-workspace-lane nextest run --locked --offline --no-default-features -p quanta-index-lexical --lib`에 두 모듈의 필터를 적용했다. 공개 UDS·CLI와 레거시 점수는 별도 Nextest 실행으로 접수했다.

후속 공개 빌드는 `InputStagingPort::staged_source_submission_ingress_v1`의 `SourceCut` import 누락으로 **FAILED · compile exit 101 · 본문 0건**이었다. 공용 포트의 import를 고치고 동일한 3개 테스트를 다시 접수했다. [공용 포트](../../../../crates/quanta-index-core/src/index/ports.rs)

공용 포트 수정 뒤 첫 재시도는 레인 입장 대기 2,400초가 만료돼 **NOT_RUN · 입장 exit 124 · 본문 0건**이었다. 동일 3개 선택을 기존 레인에 재접수했고 대기 예산은 5,400초다.

다음 빌드는 DSL seal readback과 Semantic Lance Scanner의 `-D unused-results`에서 **FAILED · compile exit 101 · 본문 0건**이었다. 반환값 처리 수정 후 공개 3개 선택을 같은 레인에 다시 접수했다. [공통 컴파일 수정](../../../bugbash/nov-20-native-delta-cost/tickets/INC-01-source-closure.md#남은-실행-검증)

그다음 빌드는 Semantic의 ANN과 vector index의 Scanner 반환값 8건에서 **FAILED · compile exit 101 · 본문 0건**이었다. 현재 소스에서 모든 반환값 처리를 확인했다. 같은 Runtime test 타깃을 사용하는 History 실행은 이후 이 컴파일 경계를 넘어 본문에 진입했다. 공개 3개 선택을 기존 레인에 다시 접수했다.

후속 Nextest `11207f9c-633f-4021-ba24-f0bfe142f248`은 **FAILED · 2/3 · exit 100**이다. `regex_terms_order_files_by_the_legacy_code_search_score`는 11.468초, `sdk_text_orders_files_by_the_legacy_code_search_score`는 10.569초로 통과했다. 이 시간에는 각 데몬 fixture의 준비가 포함된다. 4,097파일 본문은 게시 뷰 sequence 16까지 확인됐고 1,041.214초 후 `ipc Read timed out after 29999 ms`로 종료했다. 기존 출력으로는 어느 RPC인지 확정할 수 없다. Source staging·route seal·공개 질의의 오류에 단계 정보를 붙이고 기존 요청 기한과 fixture를 유지해 이 본문만 재접수했다.

단계 진단을 포함한 Nextest `ba9643a7-9c3f-4b7a-878f-11b3650ff697`은 **0/1 `FAILED`·exit 100**이었다. 4,097파일 봉인과 Native SDK `file.contains`·term-only·regex 비교를 통과했다. 공개 SDK `content:rare content:common content:"::needle"`에서 `ResourceLimited`가 반환됐고 CLI 호출에 도달하지 못했다. 거절 축은 미확정이다. [후속 실행 로그](/Users/songmin/Library/Caches/quanta-index/native-case-inverse-public-context.kryEoj/public.log) · [Q03 비용 경로](../../nov-24-native-enhancement/tickets/Q03-regex-budget.md#작업-범위)

실패한 CodeSearch의 세 조건은 모두 본문 substring이며, 기존 경로는 공통 anchor를 포함한 후보들을 만들고 합집합으로 순회했다. 최초 후보 제한 수정의 집중 회귀는 구조적 AND **1 PASS**, Literal anchor **1 FAIL · ResourceLimited · exit 100**이었다. 현행 순수 본문 AND는 최대 후보 수를 작은 값부터 늘려 각 gram의 포스팅 수를 비교하고 선택된 anchor 하나만 펼친다. 그 후보의 원문에서 모든 조건을 검사한다. 두 항 순서 각각에서 한 후보와 세 후보를 512 read 한도로 검사하는 집중 회귀와 4,097파일 공개 SDK·CLI 회귀를 접수했다. [후보 계획](../../../../crates/quanta-index-lexical/src/native/text_literal_anchor.rs) · [최초 집중 실행](/Users/songmin/Library/Caches/quanta-index/native-selective-body-conjunction.NR7Ovdwe/unit.log) · [후속 집중 실행](/Users/songmin/Library/Caches/quanta-index/native-conjunctive-planner-current.g0rbkQq5/unit.log)

후속 Nextest `78193425-522b-4364-a280-1cd880f6343c`은 **VERIFIED · 2/2 · exit 0**이다. 구조적 후보 상한과 4,097파일의 512 read 한도 단언을 각각 실행했다. 공개 Runtime 재실행은 Legacy History 전수 조회의 Catalog export가 빠져 **FAILED · compile exit 101 · 본문 0/1**였다. 현행 Catalog에는 두 export가 추가됐다. 공개 SDK·CLI의 수정 후 본문은 **NOT_RUN**이다. [집중 실행 로그](/Users/songmin/Library/Caches/quanta-index/native-conjunctive-planner-current.g0rbkQq5/unit.log) · [공개의 이전 실행](/Users/songmin/Library/Caches/quanta-index/native-4097-code-search-denial.6c8wY8/nextest.log) · [공개 후속 실행](/Users/songmin/Library/Caches/quanta-index/native-4097-code-search-current.T165XM/nextest.log)

이후 공개 컴파일은 Catalog export를 넘고 Semantic의 미사용 control-file 재내보내기에서 **FAILED · exit 101 · 본문 0/1**이었다. 재내보내기를 제거했다. 호출자는 계속 `control_file::product`를 통해 같은 측정 함수를 사용한다. 공개 본문과 요청 한도는 유지해 재접수했다. [수정](/Users/songmin/Documents/code-new/quanta-index/crates/quanta-index-semantic/src/control_file.rs:16) · [후속 실행](/Users/songmin/Library/Caches/quanta-index/native-4097-code-search-control-file.FU4ca0HH/nextest.log)

후보 owner 버퍼 후속 Nextest `c06b594e-7ca8-430e-9182-e66e224e0094`는 **VERIFIED · 5/5 · exit 0**이다. 빈 anchor의 메모리 요청 부재, 4,097파일의 희소 AND, Unicode 정규화와 Literal sibling Source 결속을 실행했다. 후속 메모리 보강은 Unicode 확장의 window·조합·gram 벡터를 할당 전에 예약하고 그 생존 기간에 맞춰 lease를 보유한다. 고정 `kkk`의 folded 확장은 64 B 요청 상한에서 거절되고 충분한 상한에서는 27개 조합을 생성해야 한다. 이 새 회귀는 `rust-test-native-lexical-unit`에 추가했고 위 5개와 별도 실행에 귀속한다. [owner 버퍼 실행](/Users/songmin/Library/Caches/quanta-index/native-literal-anchor-buffer.s9WVjR/nextest.log) · [메모리 회귀](/Users/songmin/Documents/code-new/quanta-index/crates/quanta-index-lexical/src/native/text_literal_anchor.rs:754) · [메모리 실행](/Users/songmin/Library/Caches/quanta-index/native-literal-expansion-memory.kJlvJ6jY/nextest.log)

공개 4,097파일과 History 응답 유실 재실행은 Restore Forward의 Legacy RepoMap 영수증 대조에서 각각 **FAILED · compile exit 101 · 본문 0/1**이었다. `RepoId`와 `RevisionId`는 영수증과 활성화 양쪽의 `as_str()`를 대조한다. 수정 후 두 실행은 해당 오류를 넘겼고 미연결 History 측정 함수에서 컴파일이 멈췄다. V7 후보 BOOT 검증이 History 측정과 보존 터미널 비교를 연결한 뒤 두 본문을 같은 Nextest 요청으로 재접수했다. [공개 빌드](/Users/songmin/Library/Caches/quanta-index/native-lexical-history-typed-restore.Viu7YXd4/public-4097.log) · [History 빌드](/Users/songmin/Library/Caches/quanta-index/native-lexical-history-typed-restore.Viu7YXd4/history-replay.log) · [후속 실행](/Users/songmin/Library/Caches/quanta-index/native-public-history-v7-linked.g1Mu242b/nextest.log)

Unicode 고정 역상, 두 정책의 전체 scalar 역상, 조합 메모리 입장 회귀는 후속 Nextest에서 **VERIFIED · 3/3 · exit 0**이었다. [Lexical 실행](/Users/songmin/Library/Caches/quanta-index/native-literal-expansion-memory.kJlvJ6jY/nextest.log)

Nextest `2551dbb4-92c8-45fa-9a10-dffd56a01292`는 Runtime 컴파일 후 **FAILED · 1/2 · exit 100**이었다. History 응답 유실 재제출은 **PASS · 60.141초**였다. 4,097파일 공개 케이스는 **673.920초**에 Source 게시 10번째 operation이 32회 재조정 뒤에도 터미널에 이르지 못했다. `NativeSourceMapApply` 거절 로그에는 객체 배리어 직전 deadline, scratch SQLite checkpoint deadline, 객체 설치 정산 중 cancellation이 남았다. 해당 checkpoint만으로 지연의 지배 구간을 확정할 수 없다. 이번 실행은 공개 질의 전에 끝났다. 검색 계획 후속 수정의 공개 수용은 **NOT_RUN**이다. [실패 및 경계 진단](/Users/songmin/Library/Caches/quanta-index/native-public-history-v7-linked.g1Mu242b/nextest.log)

Kernel은 모든 대상 경로의 child base가 비어 있고 repository claim이 있는 페이지를 하나의 ordinal map stream으로 설치한다. 페이지당 최대 128개의 root를 순서대로 받아 건수를 대조한 뒤 기존 scratch 트랜잭션으로 저장한다. retained base가 있는 페이지는 기존 경로를 사용한다. 공개 fixture의 256파일 게시 크기와 질의 한도를 유지했다. `cargow check -p quanta-index-kernel`는 **exit 0**이었다. 집중 Nextest `71dac771-b5f8-4eef-8d48-0c1e78d88637`에서 두 경로의 정렬된 ordinal과 독립 값 대조가 **1/1 PASS · exit 0**이었다. 4,097파일 공개 본문은 이 수정으로 다시 접수했다. [Kernel 게시](/Users/songmin/Documents/code-new/quanta-index/crates/quanta-index-kernel/src/source.rs:853) · [공개 실행](/Users/songmin/Library/Caches/quanta-index/native-4097-source-map-batch.8CEEz4/nextest.log)

Nextest `750f6196-fdc0-4850-b664-6011ce5e3e91`은 **FAILED · 0/1 · exit 100**이었다. 동일한 256파일 게시 한도에서 Source 최대 sequence 18과 protected seal을 확인했다. 첫 Native SDK `rare AND common AND file.contains('::needle') select:path`는 **29,985 ms**에 읽기 기한을 넘겼다. 데몬의 최초 관측은 `ordinary_search`의 `query_execution`에서 `Cancelled`였다. 공개 검색의 정답·정규식·CLI·넓은 요청 거절은 확인 전이다. 다음 진단은 30초 요청에서 앵커 계획과 실제 게시 목록 소비를 분리해 관측한다. [실제 종료](/Users/songmin/Library/Caches/quanta-index/native-4097-source-map-batch.8CEEz4/nextest.log:71)

동일한 Native 작업 진단 설정으로 프로세스당 초기 8개 텍스트 질의에서 단계 시간, posting 호출·hit 수, 후보와 Source 본문 확인 수를 남긴다. 취소 결과도 기록한다. 질의 기한과 결과·검증 계약은 유지한다. `cargow check --locked --offline -p quanta-index-lexical`는 **exit 0**이었다. 같은 4,097파일 fixture와 30초 질의 한도를 사용하는 선행 실행에 Native 진단 설정이 활성화된 것을 확인했다. 후발 중복은 admission 전에 중단했다. [질의 계측](/Users/songmin/Documents/code-new/quanta-index/crates/quanta-index-lexical/src/native/text_query.rs:306) · [선행 공개 실행](/Users/songmin/Library/Caches/quanta-index/native-4097-query-phase.UtJmZqDH/nextest.log)

재부팅 후 Nextest `7ecd9eb5-0769-4121-9352-cdd636d1fb46`는 **FAILED · 1/2**였다. 4,097파일의 Layered V2 공개 SDK·CLI 회귀는 **PASS · 251.677초**였다. Tree에서는 초기 SDK 희소 질의가 본문 1건을 확인해 성공했고, 뒤이은 CLI 후보 질의가 **29,994 ms**에 타임아웃됐다. 해당 CLI 질의에서 후보 1,829건과 posting 호출 1,832건을 기록했다. 후보 순회 29.803초 중 29.663초가 posting 읽기였고 Source 본문 확인은 1건이었다. Tree의 30초 한도와 fixture를 유지한 채 posting 비용을 줄여야 한다. [실행 및 단계 계측](/Users/songmin/Library/Caches/quanta-index/native-4097-post-reboot.uD6iOd5T/nextest.log:61)

Packed Literal membership 후속 Nextest `95159e0d-b4f7-44ca-8c7f-8b74a5be5db9`는 **VERIFIED · 3/3 · exit 0**이었다. 큰 view의 희소 리터럴, 대소문자·정규화, 인증된 블록 재사용을 각각 확인했다. Tree 공개 SDK·CLI 재실행은 별도로 빌드 입장을 통과했다. [집중 테스트](/Users/songmin/Library/Caches/quanta-index/native-q03-packed-anchor.Iov6S5Hq/nextest.log:25) · [공개 재실행](/Users/songmin/Library/Caches/quanta-index/native-4097-anchor-fix.E5gjIal7/nextest.log)

후속 Nextest `138289d5-0a2a-427e-b911-a36e98cd07d9`는 posting 세부 계측을 포함해 컴파일을 마쳤다. Layered V2 4,097파일 케이스는 Source sequence 8·9·12의 `BuildPrepare` deadline 거절 뒤 질의에 진입했다. CLI 후보 질의가 29,999 ms에 타임아웃되어 **FAILED · 1본문 · 1,339.099초**였다. 취소 시 후보 1,910건과 postings 호출 1,913건, posting 검증 28.926초를 기록했다. Tree는 Source fixture 게시를 마친 뒤 첫 SDK 희소 질의에서 29,965 ms에 타임아웃됐다. 취소 시 후보 1,683건, posting 호출 1,686회, posting 검증 28.265초였다. 세부 시간은 SourceRecord 10.207초, contribution 9.610초, occurrence 6.340초, page 1.591초, inventory 0.063초였다. 최종 **FAILED · 0/2 · exit 100**이다. 기존 비후보 posting 손상 거절을 유지하면서 객체 읽기 비용을 줄여야 한다. [실행 로그](/Users/songmin/Library/Caches/quanta-index/native-4097-anchor-fix.E5gjIal7/nextest.log:67)

Native reader의 close 뒤 pin을 반납하는 잠금에서 회수 대기 여부를 함께 읽는다. tombstone의 마지막 pin이면 같은 잠금에서 회수를 표시하고, 회수 작업이 없으면 추가 잠금을 생략한다. 성공한 open도 기존 opening 예약 한 건을 같은 잠금에서 pin 한 건으로 전환한다. 새 회귀는 같은 opening의 재완료를 거절하고, 실제 FD close 전에는 슬롯이 점유되며 close 뒤에는 다시 입장하는지 검사한다. 기존 집중 실행은 공통 빌드 입장을 3,600초 기다린 뒤 **exit 124 · 본문 0건**으로 종료했다. 새 회귀와 4,097파일의 종단 지연은 **NOT_RUN**이다. [Native reader 회귀](../../../../crates/quanta-index-searchd-runtime/src/native_object_read_io.rs:395) · [입장 종료](/Users/songmin/Library/Caches/quanta-index/native-reader-pin-atomic.tBLtQsjp/nextest.log)

512바이트 이하인 저장 객체는 고정 버퍼에서 전체 wire를 한 번에 읽고 EOF·digest·형식을 검사한다. 512/513바이트 경계, child 참조, 손상 거절 회귀를 추가했다. 작은 객체의 데이터 read 호출 수를 검사하는 단위 회귀도 추가했다. 새 opening 전환 회귀와 저장소 경계 회귀는 기존 빌드 입장 레인에서 대기 중이며 4,097파일 성능 본문은 **NOT_RUN**이다. [작은 객체 읽기](../../../../crates/quanta-index-storage/src/object/read_io.rs:167) · [opening 전환 실행](/Users/songmin/Library/Caches/quanta-index/native-reader-opening-transfer.PY6L1efD/nextest.log)

Native 객체 reader가 admission에 사용한 부모 FD로 `openat`한다. 절대 경로를 다시 해석하는 비용을 줄이고, open 전 정상 파일이 FIFO로 바뀌어도 nonblocking open 뒤 손상으로 거절한다. 열린 FD를 현재 경로·retained inode에 대조하는 단계는 유지한다. FIFO 교체와 opening 종료 뒤 재입장 회귀를 같은 기존 reader 선택에서 실행한다. `rustfmt` 파싱과 대상 diff 검사를 통과했고, 집중 본문과 4,097파일 성능은 **NOT_RUN**이다. [reader 경계](../../../../crates/quanta-index-searchd-runtime/src/native_object_read_io.rs) · [집중 실행](/Users/songmin/Library/Caches/quanta-index/native-reader-opening-transfer.PY6L1efD/nextest.log)

관리형 저장소는 namespace 락의 실제 FD와 처음 확인한 inode를 함께 보유한다. 각 객체 읽기에서 현재 이름의 device·inode와 owner·mode·link count를 확인하며, 같은 FD의 별도 `fstat` 한 번을 줄인다. 락 권한 변경과 같은 권한의 다른 inode 교체를 모두 거절하고 원본 복구 뒤 조회가 재개되는 회귀를 추가했다. 집중 실행과 공개 4,097파일 지연은 **NOT_RUN**이다. [namespace 코드](../../../../crates/quanta-index-storage/src/object/namespace.rs) · [집중 실행](/Users/songmin/Library/Caches/quanta-index/native-namespace-held-lock.farrKzYs/nextest.log)

Native reader의 부모 경로 탐색은 보유 root FD 아래 디렉터리만 연다. reader admission과 pin 확정에서 각각 탐색 뒤 현재 root·락을 인증하고 나서 leaf를 검사한다. 이 순서로 read 전용 탐색마다 중복 root 검사를 한 번 줄인다. 이미 admission한 reader도 락 권한이 바뀌면 pin을 거절하고, 권한 복구와 예약 반납 뒤 새 읽기는 다시 허용해야 한다. 동일 reader 선택의 본문은 **NOT_RUN**이다. [권한 변경 회귀](../../../../crates/quanta-index-searchd-runtime/src/native_object_read_io.rs:422)

reader 집중 실행은 기존 Q01 컴파일이 7,200초 한도에 도달해 락을 반환한 뒤 입장했다. 공용 읽기 helper 변경 뒤 남은 `std::io::Read` import가 `-D unused-imports`에 걸려 **FAILED · Rust 본문 0건**이었다. 불필요한 import를 제거했다. [Q01 종료](/Users/songmin/Library/Caches/quanta-index/native-q01-cursor-codec.4YQ5DCTH/nextest.log) · [reader 컴파일](/Users/songmin/Library/Caches/quanta-index/native-reader-opening-transfer.PY6L1efD/nextest.log)

동일 reader selector 재실행은 기존 요청의 오류 종료를 확인한 뒤 공용 빌드 큐에 들어간다. [후속 실행 상태](/Users/songmin/Library/Caches/quanta-index/native-reader-after-import-fix.f6hqnMhG/stage)

일반 owned reader와 Native reader는 같은 pin 등록을 사용한다. 파일 추적 상태를 확인하고 opening 예약을 pin으로 넘기는 작업을 한 잠금에서 처리한다. 기존 한 슬롯을 일반 owned reader가 보유하는 동안 Native reader가 입장하지 못하고, 실제 close 뒤에는 다시 입장하는 회귀를 추가했다. [공유 pin 등록](../../../../crates/quanta-index-searchd-runtime/src/publication_file_capacity.rs:1877) · [한 슬롯 회귀](../../../../crates/quanta-index-searchd-runtime/src/native_object_read_io.rs:382)

storage `small_` lib 실행은 5,400초 빌드 입장 초과로 **BLOCKED · exit 124 · 본문 0건**이었다. storage import 수정 뒤 같은 테스트 범위를 기존 큐에 10,800초 대기로 재접수했다. [storage 재접수](/Users/songmin/Library/Caches/quanta-index/native-storage-small-wire-retry.8aVPVb7Z/nextest.log)

공개 후속 실행 `102caa2c-34d2-493d-b8b5-fbd0c475f353`은 **FAILED · 0/2**였다. 두 레이아웃 모두 Source 게시와 봉인 뒤 첫 SDK 희소 AND에서 타임아웃됐다. Layered V2는 **29,985ms**, 후보 3,542건, posting 3,545회·27.846초였다. Tree V1은 **29,979ms**, 후보 3,923건, posting 3,926회·29.345초였다. Tree의 contribution 객체 읽기는 10.652초, SourceRecord 객체 읽기는 10.935초였다. 두 실행 모두 Source 원문 확인은 1건이다. [공개 회귀 로그](/Users/songmin/Library/Caches/quanta-index/native-q03-posting-stream-product-retry.kolavLZb/nextest.log:33)

같은 Layered 본문의 SDK 질의 전 Source 게시 구간을 3초 샘플링했다. worker 표본 67개 중 51개가 path validation의 기존 `base.paths` 단건 lookup에 있었고, 디스어셈블리의 `PathScratch::page` 호출로 분기를 확인했다. 후속 변경은 중첩 경로의 첫 디렉터리를 기존 root FD에서 열고, 객체 digest의 hex 이름과 shard를 스택에서 생성해 완성된 경로만 할당한다. 대상 inode·현재 root·락 확인과 digest·EOF 검증을 유지한다. 이 변경들의 실행 검증은 **NOT_RUN**이다. [게시 단계 스택](/Users/songmin/Library/Caches/quanta-index/q03-live-layered-sample.I9cdvc6g/layered.txt:153) · [경로 탐색](../../../../crates/quanta-index-searchd-runtime/src/publication_file_capacity.rs:1568) · [객체 경로](../../../../crates/quanta-index-storage/src/object/read_io.rs:84)

Layered posting 계측은 partition 로드, Source owner map, SourceRecord 객체 읽기·디코드·검증과 occurrence를 구분한다. 후속 Nextest `39d63d00-3ea0-4480-ad2d-cb6324f4d3ff`의 4,097파일 Layered 본문은 **VERIFIED · 1/1 · 325.285초**였다. SDK·CLI의 정확한 한 경로와 점수 비트 일치, 넓은 OR의 `ResourceLimited`를 확인했다. 첫 SDK 희소 AND의 계측 구간 합은 8.578초, posting 4,099회·8.015초, SourceRecord 객체 읽기 6.082초, 실제 Source 본문 확인 1건이다. 이전 실패와 호스트·소스가 달라 비교 벤치 수치로 사용하지 않는다. 손상 입력 집중 실행은 기존 큐에 있다. [Layered posting 경계](../../../../crates/quanta-index-lexical/src/native/text_segment/read/read_v2.rs) · [Layered 실행](/Users/songmin/Library/Caches/quanta-index/native-q03-layered-source-probe.T0Pa5h/nextest.log:22) · [Tree 재실행](/Users/songmin/Library/Caches/quanta-index/native-q03-tree-path-witness.MvZe5b/nextest.log)

후속 Nextest `8cf8f44b-f762-47b5-b601-3150f7778cc2`에서 같은 공개 fixture의 4,097파일 Tree V1 본문이 **VERIFIED · 1/1 · 222.031초**였다. SDK·CLI에서 정확한 한 경로와 점수 비트 일치, 넓은 OR의 typed `ResourceLimited`를 확인했다. 첫 SDK 희소 AND의 계측 구간 합은 19.220초, posting 4,099회·18.675초, 실제 Source 본문 확인 1건이다. Tree와 Layered 결과는 각 실행에 사용된 입력에 결속된다. [Tree 터미널](/Users/songmin/Library/Caches/quanta-index/native-q03-tree-path-witness.MvZe5b/nextest.log:19) · [공용 공개 검증](../../../../crates/quanta-index-searchd-runtime/tests/native_index_uds/read_sessions/query/literal_anchor.rs#L69)

Tree 빌드 위치의 바이너리를 inode·mtime 검사 뒤 복사하고 `cmp`로 대조한 후, 공용 빌드·테스트 입장 아래에서 Layered 공개 selector를 직접 실행했다. **VERIFIED · 1/1 · 172.61초**였다. Tree Nextest 원로그가 바이너리 ID를 남기지 않아 그 실행과 복사본의 동일성은 빌드 위치와 시각에 따른 추론이다. [Layered 직접 실행](/Users/songmin/Library/Caches/quanta-index/native-q03-layered-same-artifact.aFWMwQ/libtest.log:5)

P01이 반영된 main 작업 트리의 Nextest `6e744a03-b008-4a22-9a1f-ddc6d98339af`에서 두 4,097파일 selector를 한 `native_index_uds` 바이너리로 순차 실행해 **VERIFIED · 2/2 · exit 0**이었다. Layered V2 전체 fixture는 136.955초, Tree V1은 213.500초였다. 첫 SDK 질의의 계측 구간 합은 각각 5.419초·8.579초, posting은 양쪽 4,099회, Source 원문 확인은 각각 1건이었다. 공개 SDK·CLI가 같은 경로·점수 비트를 반환하고 넓은 OR의 typed 거절을 확인했다. 실행 중 확인한 바이너리 SHA-256은 `ede3b3e9cfb3cf0bb0545145aecad44a185eb0566df5c57e5e21a40a20703b39`다. [두 레이아웃 실행](/Users/songmin/Library/Caches/quanta-index/q03-p01-dual-layout-clt.rVPXWf/nextest.log:7)

같은 Layered 실행의 3초 표본은 검색 전 Source 게시 중 `apply_stream`에 있었으며 775개 worker 스택 중 580개가 reachable object 설치에 있었다. no-replace rename의 부모 경로 재검증 스택은 첫 호출 112개, 마지막 호출 15개였다. 실행 파일의 두 호출 주소를 디스어셈블리에서 확인했다. 후속 코드는 held FD·leaf 확인 뒤 rename 직전에 한 번만 live parent를 확인한다. 부모가 교체되면 원본 임시 파일과 charge를 유지하고 게시를 거절하는 회귀를 추가했다. Layered 통과 바이너리의 컴파일 이후 반영됐다. 후속 Nextest `2fa6c54a-a615-4fb9-9c9c-744398294d5f`는 **VERIFIED · 4/4**였다. 분리된 부모에서 게시 거절, no-replace 경쟁 패자의 독립 정리, fsync 뒤 원래 shard 복원 거절을 확인했다. 게시 지연 효과는 미계측이다. [집중 실행](/Users/songmin/Library/Caches/quanta-index/native-writer-parent-clt.gZGDfGLi/nextest.log:12) [표본 스택](/Users/songmin/Library/Caches/quanta-index/q03-layered-current-stage.gQ6VNNxs/layered.txt:261) · [no-replace 확인](/Users/songmin/Documents/code-new/quanta-index/crates/quanta-index-searchd-runtime/src/publication_file_capacity.rs:4488) · [출력 회귀](/Users/songmin/Library/Caches/quanta-index/native-writer-parent-clt.gZGDfGLi/nextest.log)

관리형 객체 reader는 Storage가 이미 검사한 leaf metadata를 기대 inode·크기·link 수로 전달받는다. opening 슬롯을 먼저 예약하고 실제 FD와 현재 anchored leaf, root·락 및 retained ledger를 확인해 pin으로 전환한다. 같은 leaf의 사전 stat 및 retained 등록을 한 번 줄이며, digest·EOF 검증과 보통 reader adapter의 동작을 유지한다. 같은 크기의 다른 inode가 들어온 경우 거절하고 한 슬롯으로 즉시 재입장하는 회귀를 추가했다. Rust 구문과 변경 검사를 통과했다. 집중 본문과 지연 효과는 **NOT_RUN**이다. [객체 open 경계](../../../../crates/quanta-index-storage/src/object/read_io.rs:579) · [stale inode 회귀](../../../../crates/quanta-index-searchd-runtime/src/native_object_read_io.rs:405)

reader의 이전 집중 실행은 macOS에서 Linux XFS 분기에만 쓰는 `writer` 인자가 unused로 처리돼 Runtime lib 테스트 컴파일 **exit 101 · 본문 0건**으로 끝났다. fixture 인자를 고쳐 Linux의 실제 candidate FD 검사와 macOS volume pressure 동작을 유지한다. 설치된 Command Line Tools로 같은 선택자를 기존 빌드 큐에 재접수했다. [컴파일 진단](/Users/songmin/Library/Caches/quanta-index/native-reader-after-import-fix.f6hqnMhG/nextest.log:23) · [fixture 코드](../../../../crates/quanta-index-searchd-runtime/src/native_object_write_io/ann_enospc.rs:542) · [reader 재접수](/Users/songmin/Library/Caches/quanta-index/native-reader-clt.QTJZ2vjR/nextest.log)

Storage의 별도 read adapter가 없는 경우에도 실제 객체 open에 `NOFOLLOW | NONBLOCK`을 사용한다. pathname stat 뒤 FIFO가 들어와도 open이 멈추지 않고 반환된 FD에서 기존 type·inode 검사를 실행한다. FD flag 회귀와 기존 custom adapter의 동일 내용·다른 inode 거절 회귀를 같은 저장소 선택에 접수했다. `rustfmt --check`와 `git diff --check`는 통과했고 본문은 대기 중이다. [Storage open](../../../../crates/quanta-index-storage/src/object/read_io.rs:71) · [저장소 실행](/Users/songmin/Library/Caches/quanta-index/native-storage-reader-clt.32a8GzvO/nextest.log)

Source rename·delete는 이전 경로 reverse entry를 path scratch 검증에서 한 번 인증한다. 이전·다음 경로가 같은 변경은 기존 owner 단계에서 검사한다. scoped claim은 기존 repository child owner를 확인하며 legacy marker가 repository edit를 허가하지 않는다. 어느 쪽도 영속 map 설치 전에 검증을 끝낸다. Nextest `03f4dd64-c4ab-4049-81bd-29ae0c23cb18`은 **VERIFIED · 3/3**이다. 129경로 본문은 기존 repository child 객체를 임시 제거했을 때 `Unknown`과 최신 뷰 유지를 확인했다. 복원 뒤 같은 operation과 반복 재개가 동일한 게시 결과를 냈고 독립 FULL과 owner·경로·bucket이 일치했다. 나머지 두 본문은 fresh child root 순서와 repository scope 변경 거절을 확인했다. [Source 검증](../../../../crates/quanta-index-kernel/src/source.rs:1257) · [129경로 회귀](/Users/songmin/Documents/code-new/quanta-index/crates/quanta-index-kernel/tests/native_lifecycle.rs:869) · [실행 결과](/Users/songmin/Library/Caches/quanta-index/native-q03-mixed-source-clt.gdQzFN/nextest.log:35)

이전 posting membership 집중 실행은 호스트 Xcode 라이선스 오류로 Cargo 링크 **exit 101 · 본문 0건**이었다. 같은 회귀 범위를 설치된 Command Line Tools SDK로 기존 입장 큐에 다시 접수했다. [링크 실패](/Users/songmin/Library/Caches/quanta-index/native-q03-postings-membership-admission-retry.MilxIB8P/nextest.log) · [CLT 재접수](/Users/songmin/Library/Caches/quanta-index/native-q03-membership-clt.3SL77tHe/nextest.log)

- [x] **09.2a 결정.** 다음 중 하나를 [07](07-product-qualification.md)의 입장 정책에 기록한다.
  - **A · 거절 유지.** fixture는 `ResourceLimited`로 거절된다. SDK·CLI 오류에 거절 사유와 대안(경로·repo 제한)을 노출하고, 지원 코퍼스 규모와 이 제한을 호출자 문서에 적는다.
  - **B · 실제 검증량 예산.** repo·path·lang 제한, 필수 리터럴, 안전한 AND 교집합을 먼저 적용하고, 교집합 이후 실제로 원문을 검증하는 파일 수에 예산을 건다. 목표 코퍼스 규모(파일 수·바이트)와 검증량 예산 수치를 결정 기록에 고정한다. 늦은 페이지 누락 방지 보장은 그대로 유지한다.
- [x] **09.2b 로컬 고정 fixture 수용.** 어느 쪽이든 fixture를 공개 SDK·CLI로 실행한다. A면 `ResourceLimited`이고, 0건이나 완료로 표시되면 실패다. B면 정확히 1파일이고, 목표 규모 코퍼스에서 선택도가 낮은 질의는 예산 거절 또는 취소 가능한 순회와 명시적 완료 신호를 낸다. 기존 4,097문서 경계·허위 DF·부분 성공 거절 회귀는 결정에 맞춰 갱신하고 다시 통과해야 한다.
  - 11월 25일 로컬 TreeV1 Nextest **1/1 · exit 0**과 SegmentedLayeredV2 libtest exact **1/1 · exit 0**을 확인했다. 실행 경로는 Native SDK, SDK `.text()`, CLI `code-search`와 `search-text`다. 본문 리터럴·regex를 결합한 희소 AND는 1파일로 완료했고 넓은 OR는 `ResourceLimited`와 빈 CLI stdout으로 거절했다. 집중 Lexical 14/14와 혼합 Source 3/3은 Q03 원장에서 인수한다. 승인 Linux 설치본은 V03 범위다. [Q03 원장](../../nov-24-native-enhancement/tickets/Q03-regex-budget.md)

Command Line Tools 후속 실행에서 Runtime 객체 reader는 **VERIFIED · 9/9**, Storage는 **VERIFIED · 5/5**였다. Runtime은 stale leaf metadata·FIFO 교체·root lock 교체 거절, opening의 단일 pin 전환과 슬롯 반환을 확인했다. Storage는 nonblocking open, 다른 inode의 대체 거절, 작은 wire의 data·EOF 읽기 수와 digest 경계를 확인했다. [Runtime `158e17c0`](/Users/songmin/Library/Caches/quanta-index/native-reader-clt.QTJZ2vjR/nextest.log:10) · [Storage `e74e0d9b`](/Users/songmin/Library/Caches/quanta-index/native-storage-reader-clt.32a8GzvO/nextest.log:6)

**담당** 기존 TEXT 오너, 결정 기록은 [07](07-product-qualification.md) 오너. **상태** 09.2a는 구조적 후보 상한으로 결정했다. evaluator 단위 검증은 위 레인 결과를 인수하며 09.2b 공개 SDK·CLI는 4,097파일 Tree V1과 Layered V2에서 각각 **VERIFIED**다. 고정 입력은 4,097파일·97,240바이트, 원문 후보 상한은 4,096파일이다. P01을 포함한 현재 로컬 실행은 한 Nextest 바이너리에서 두 레이아웃 **2/2**를 확인했다. [실행](/Users/songmin/Library/Caches/quanta-index/q03-p01-dual-layout-clt.rVPXWf/nextest.log:7)

Layered posting 집중 실행은 **FAILED · 9/10**이었다. 혼합 phrase/keyword 회귀가 임시 메모리만 허용하는 테스트 원장을 사용해, 최종 반환 hit의 정상적인 Retained 예약에서 거절됐다. 테스트는 기존 혼합 수명 원장을 사용하고 결과만 Retained 예약을 보유하며 결과 drop 뒤 전량 반환하는지 검사한다. 같은 Layered 혼합 phrase/keyword와 이웃 overlay·regex·continuation 회귀는 후속 Nextest `7892ea99-a89e-4d63-87ea-5ce9c496d587`에서 **VERIFIED · 2/2**였다. [실패 본문](/Users/songmin/Library/Caches/quanta-index/native-q03-membership-clt.3SL77tHe/nextest.log:35) · [수정한 회귀](../../../../crates/quanta-index-lexical/src/native/text.rs#L1952) · [재실행](/Users/songmin/Library/Caches/quanta-index/layered-funding-clt.6ENufE/nextest.log)

## 09.3 · Native↔Zoekt 동일 조건 비교의 입력과 합격선

**현행 계약.** 출하 비교 기준은 구 서명 릴리스 대비 `AB`/`BA` 5쌍, p95 비율 중앙값 `≤1.05`다. Zoekt 비교는 DoD와 원장에 기준이 없다. [고정 성능 기준](../../nov-12-native-cutover/DOD.md#기존-고정-성능-및-자원-기준) · [07 비교 입력](07-product-qualification.md#출하-벤치와-비교-입력)

- [ ] **09.3a 게이트 여부.** Zoekt 비교가 출하 게이트인지 먼저 정한다. 기본안은 **비게이트 참고 지표**다. 결과는 P2 비용 개선의 우선순위 근거로 쓰고, 공통 질의의 정확성 불일치만 해당 결함 티켓으로 넘긴다. 게이트로 정하면 아래 수치를 DoD에 추가한다.
- [ ] **09.3b 입력 고정.** 실행 전에 기록한다: Zoekt `b55ce727df8c`와 빌드·인덱서 옵션, `SearchOptions`의 shard·전체·표시 한도, 같은 Linux·SSD, 코퍼스 commit과 파일 manifest(양쪽 제외 규칙의 차이 포함), 질의 대응표(대소문자, regex 방언, 파일 단위 vs line/chunk 결과). 대응표에 없는 질의는 비교하지 않는다.
  - **입력 초안 (11월 24일, 승인 전).** 코퍼스는 07.3과 같은 Caddy 590파일·etcd 1,421파일과 07.4 Large 4,096파일을 쓴다. Zoekt는 `b55ce727df8c`를 `go build`하고 `zoekt-git-index`로 같은 commit을 색인한다. 크기 상한 등 제외 규칙의 차이는 양쪽 파일 manifest를 비교해 기록한다. `SearchOptions`의 `ShardMaxMatchCount`·`TotalMaxMatchCount`·`MaxDocDisplayCount`는 코퍼스 파일 수 이상으로 두고, `Stats`의 `ShardsSkipped`·`FilesSkipped`가 0이 아니면 잘린 결과로 분류한다.
  - **질의 대응 초안.** 양쪽 모두 대소문자를 명시한다(`case:no`/`case:yes`). Native CodeSearch 리터럴 `foo` ↔ Zoekt 이스케이프한 리터럴(Zoekt는 검색어를 기본적으로 regex로 해석), 인용문 `"a b"` ↔ `"a b"`, `regex:/r/` ↔ `r`, `path:p` ↔ `file:p`. 언어 필터는 Native SDK `.language_any_of([language])` 또는 CLI `--language CODE` ↔ Zoekt `lang:LANG`으로 적용한다. Native CodeSearch 문자열의 `lang:`은 거절되므로 언어별 독립 파일 집합을 양쪽에 같은 범위로 적용했는지 대조한다. 가장 가까운 공통 정확성 단위는 파일 집합이다. [SDK 제약](../../../../crates/quanta-index-sdk/src/lexical.rs#L1014) · [CLI 인자](../../../../crates/quanta-index-searchctl/src/native/code_search.rs#L103) · [문자열 문법](../../../../crates/quanta-index-contract/src/query/code_search.rs#L555)
- [ ] **09.3c 정확성 선행.** 대응표의 공통 질의마다 양쪽 파일 집합이 `rg` 독립 정답과 일치해야 수치 비교에 넣는다. 거절·timeout·잘림은 분모에서 빼지 않고 따로 집계한다.
- [ ] **09.3d 측정과 합격선.** FULL 구축 시간, 한 파일 반복 편집·delete·rename의 게시→검색 가능 시각, 읽기·게시 동시 부하의 검색 p95/p99, 쓰기 바이트·peak 디스크·RSS를 `AB`/`BA` 5쌍으로 잰다. 비율 합격선은 **실행 전에** 이 체크박스에 적고, 실행 뒤에는 바꾸지 않는다. Native 고유의 원자 게시·ACK 재생·고정 뷰 계약은 비율이 아니라 절대 통과로 따로 판정한다.

**담당** 기존 Bench 오너([05](05-bench-callers-hosted-ci.md)·[07](07-product-qualification.md)), F 인수. **상태** 09.3a 결정은 열려 있다. 로컬 Zoekt etcd 1,421파일·정확성 4/4를 인수한다. Linux 후보의 입력 고정과 짝 비교는 `NOT_RUN`이다. [V02 실행 근거](../../nov-24-native-enhancement/tickets/V02-zoekt-comparison.md)

## 09.4 · 고정 뷰 페이지 순회의 반복 비용

**11월 26일 코드 대조.** Q `28784da26` HEAD는 아직 페이지마다 재순위한다. 같은 main 작업 트리의 P01 변경은 세션·뷰·질의·artifact에 묶인 순위를 재사용하며 `page_reuse` 4/4와 단독 재확인 1/1을 통과했다. 기존 TEXT·Runtime 담당의 변경 합류와 최종 후보의 1,000파일 비용·공개 커서 수용이 남았다. [P01 현황](../../nov-24-native-enhancement/tickets/P01-ranked-pagination.md) · [합류 인계](../../nov-26-remaining/tickets/05-lexical-ranked-page-integration.md)

- [x] **09.4a 계측.** 고정 뷰에서 첫 페이지부터 마지막 페이지까지 읽을 때 페이지별 posting·본문 방문 수, Work 사용량, 지연을 기록한다. 결과 수(100·1,000·10,000)와 페이지 크기를 따로 바꾼다.
- [x] **09.4b 작업 트리 구현.** 같은 view·query에서 확정한 전체 건수와 순위 경계를 커서에 결속하고 선택적으로 재사용한다. 기존 실행에서 캐시 활성·비활성 결과를 대조했다.
- [ ] **09.4b 합류와 제품 수용.** 기존 변경을 합류하고, 결과·순위·`matched`·마지막 페이지를 09.1b 골든과 대조한다. 1,000파일 `page_cost`로 전체 순회 비용과 보유 메모리를 확인한다.
  - [P01 작업 트리 감사](../../nov-24-native-enhancement/tickets/P01-ranked-pagination.md)에서 선택적 순위 수집의 work 상한과 메모리 예약 오류를 확인했다. 낮은 한도에서 캐시 활성·비활성 검색의 동등성과 예약 원장 오류 전파를 09.4b 수용에 포함한다.
  - **11월 24일 상태: 통합 기준선 `80fa6c01b` 합류, main 대기.**
    - 첫 페이지 스캔 한 번이 `(점수, owner, CodeSearchMatch)` 순위 전체를 투영 없이 잡는다. 상한은 `MAX_UNANCHORED_DOCS`와 같은 4,096이고, 단위 테스트로 결속했다.
    - 세션은 `(view, instance, query digest, path)` 키로 순위 하나를 보유한다. 이후 페이지는 이진 탐색으로 커서 위치를 찾고, 행마다 인증된 원문 하나만 읽는다.
    - 메모리 예약이 거절되면 캐시를 포기하고 재스캔한다. work 예산 과금 오류는 그대로 전파한다.
    - 캐시는 release와 maintenance release에서 지운다.
    - 수용 근거는 `page_reuse.rs`다. 캐시 끔·켬 데몬의 `NativeTextPathPageV1` 직렬화 바이트(행·`code_search`·excerpt)가 같고, 모든 행이 `validate_code_search`를 통과한다.
    - 남은 것은 두 가지다. 무거운 `page_cost` N=1,000 표(**NOT_RUN**)와 main 합류다.

**11월 25일 09.4b 격리 후보.** 현재 Q 입력을 복제한 `9c0a2dedf` 위에 순위 재사용을 이식한 `08fc64167`을 고정했다. 순위 수집은 세션에 할당된 메모리를 사용하고 보유할 수 없으면 일반 조회를 계속한다. 현행 SDK v2 커서를 유지한다. 제품 기본값은 보유 메모리 0이며, 집중 회귀는 양수로 설정한 경우와 0인 경우의 페이지를 대조한다. 이 후보의 Rust 구문과 diff 검사는 통과했다. `native_index_uds`의 두 `page_reuse` 본문은 빌드 입장 대기 중이며 Q main 합류는 미완료다. [격리 구현](/Users/songmin/Library/Caches/quanta-index/p01-rebase.IIPHENv8/crates/quanta-index-searchd-runtime/src/native_read_session_transport/page.rs) · [집중 테스트](/Users/songmin/Library/Caches/quanta-index/p01-rebase-nextest.6DomP9CN/nextest.log)

**11월 24일 09.4a 측정 (레인 B6, 통합 기준선 `6e441c512`, 합류 준비 중).** Tree 레이아웃, 모든 Source가 일치하는 `alpha` 질의를 유지 세션에서 마지막 페이지까지 읽었다. 수치는 `WorkLimits`가 강제하는 요청 `WorkUsage`다.

| N | 페이지 크기 | 페이지 수 | 페이지당 reads | 총 reads | 총 bytes |
| --- | --- | --- | --- | --- | --- |
| 100 | 10 | 10 | 353–363 | 3,546 | 6.36MB |
| 100 | 100 | 1 | 639 | 639 | 3.00MB |
| 1,000 | 10 | 100 | 3,289–3,309 | 330,435 | 393MB |
| 1,000 | 100 | 10 | 3,776–3,939 | 38,925 | 86.5MB |

- 모든 페이지가 `matched = N`을 다시 계산한다. 페이지 비용은 위치와 무관하게 S(N)+h·행이고, S는 문서당 약 3.2 reads, h는 행당 약 3–7 reads다. 그래서 끝까지 읽는 비용은 (N/k)·S(N)으로 N에 대해 이차다. N=100→1,000에서 총비용이 k=10이면 93배, k=100이면 61배다(한 번만 스캔하면 약 10배).
- 테스트는 이 모델의 귀결만 단언한다: 페이지 수와 `matched`, 같은 시퀀스 안 페이지 비용 편차 10% 이하, 페이지 비용의 N 선형성, 총비용 50배 이상. 정확한 상수는 Tree map 노드 배치에 달려 있어 단언하지 않는다.
- **09.4b 계약 (E3 뒤 구현).** 세션·뷰·인스턴스·질의 digest에 묶은 세션별 캐시에는 전체 스캔이 이미 계산한 순위 `(score, owner)`, `matched`, `artifact`만 둔다. 행 projection은 페이지마다 한다(첫 페이지 비용을 늘리지 않는다). 캐시 범위를 넘으면 `rank_after`로 한 번 다시 스캔한다. 메모리는 세션에 과금하고 세션과 함께 해제하며, 예약에 실패하면 지금처럼 다시 스캔한다. 캐시 적중 페이지는 캐시를 끈 페이지와 바이트 단위로 같아야 한다.

- **09.4a 합류 (main·통합 기준선 `5ab828919`, 미커밋).**
  - `page_cost` 테스트는 무거운 테스트로 표시했고(165초), `just rust-test-native-page-cost`로만 실행한다.
  - 운영 telemetry API `NativeIndexService::observe_retained_read_work`를 추가했다. 유지 세션 질의 요청마다 `WorkLimits`가 강제하는 query·custody `WorkUsage`를 결과가 확정된 뒤 한 번 보고한다. 읽기 전용이고, observer는 하나만 둘 수 있다(두 번째는 `Conflict`).
  - 기존 진단 경로는 표본 추출이라 모든 페이지를 담지 못하고, trace는 stage·schema·도구 전반을 바꿔야 해서 쓰지 않았다.

**담당** 기존 TEXT·Runtime 세션 오너. 09.4a 기준선 계측과 09.4b 작업 트리 집중 본문은 `VERIFIED`다. 09.4b의 HEAD 합류와 최종 후보 수용은 열려 있다.
