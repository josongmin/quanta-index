# SEP-21 Lane Handoffs

Lane 실행 결과는 `lane-handoff.schema.json`에 맞는 `<LANE>.json`으로 남긴다. 계획 작성 시점에는 결과 JSON을
미리 만들지 않는다. source-bound proof가 없는 placeholder handoff는 downstream gate로 사용할 수 없다.

P02A/P02B 통합 owner는 두 lane commit을 합친 clean HEAD에서 proof를 다시 실행하고 `P02I.json`을 만든다.
