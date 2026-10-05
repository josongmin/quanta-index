# Semantica 선택적 Index 통합

Authoritative 문서는 `semantica-codegraph-v2` 저장소의
`docs/plans/oct-4-index-semantica-integration/README.md`다.
2026-10-04 source 기반 설계이며 제품 구현·설치 qualification 완료를 뜻하지 않는다.

Index owner 범위는 기존 corpus/source revision/window/cursor/retained read/IPC 계약의 확인된 부족과 producer–consumer migration이다.
Canonical analysis fact resolution·join·최종 completion은 Semantica가 소유한다.
구체적 변경·oracle·명령은 같은 Semantica packet의 `ACTIONS.md`에 둔다.
Index의 현재 read-view/SDK 계약은 [Accepted ADR](../../adr/OCT-05-003-active-query-and-runtime-lifecycle.md)을 따른다.
