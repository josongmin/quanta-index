# Architecture Decision Records

이 디렉터리는 현재 product architecture decision의 canonical owner다. Historical probe와 campaign-local
결정 기록은 원래 위치에 남기되, 현재 구현을 구속하는 결정은 여기의 `Accepted` ADR만 권위로 사용한다.

상태:

- `Proposed`: 구현 시작을 허용하지 않는 초안
- `Accepted`: blocking consumer가 구현할 수 있는 frozen decision
- `Superseded`: 후속 ADR 링크가 필수인 폐기 결정

ADR 변경은 decision을 바꾸는 breaking change다. 같은 commit에서 downstream contract, inventory, migration
class와 proof authority를 갱신한다. optional compatibility field나 dual live decoder로 decision drift를 숨기지 않는다.

SEP-21 accepted set:

- [Canonical identity and digest domains](SEP-21-001-canonical-identity-and-digest-domains.md)
- [Durable authority and operation lifecycle](SEP-21-002-durable-authority-and-operation-lifecycle.md)
- [Read view, continuation and provider policy](SEP-21-003-read-view-continuation-and-provider-policy.md)
- [Process supervision, state cutover and proof](SEP-21-004-process-supervision-state-cutover-and-proof.md)
- [Decision registry](SEP-21-DECISION-REGISTRY.md)
