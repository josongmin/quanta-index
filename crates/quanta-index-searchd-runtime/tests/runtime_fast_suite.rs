#![forbid(unsafe_code)]

#[path = "common/fail_closed_wait.rs"]
mod fail_closed_wait;
#[path = "common/frontdoor_scenarios.rs"]
mod frontdoor_scenarios;
#[path = "common/sdk_frontdoor_lexical_fixture.rs"]
mod sdk_frontdoor_lexical_fixture;

#[path = "end_to_end.rs"]
mod end_to_end;
#[path = "sdk_frontdoor.rs"]
mod sdk_frontdoor;
