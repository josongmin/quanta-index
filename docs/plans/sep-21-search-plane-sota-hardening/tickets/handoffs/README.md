# SEP-21 Lane Handoffs

Lane 실행 결과는 `lane-handoff.schema.json`에 맞춰 `artifacts/sep-21/handoffs/<LANE>.json`으로 남긴다. 결과
artifact는 result commit 뒤 생성하고 source dirty digest에서 제외한다. tracked source에 result commit SHA를
자기참조로 기록하지 않는다. source-bound proof가 없는 placeholder handoff는 downstream gate로 사용할 수 없다.

P02A/P02B 통합 owner는 두 lane commit을 합친 clean HEAD에서 proof를 다시 실행하고 `P02I.json`을 만든다.
