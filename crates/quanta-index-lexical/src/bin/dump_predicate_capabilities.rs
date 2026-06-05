use std::collections::BTreeMap;

use serde::Serialize;

#[expect(
    dead_code,
    reason = "dev tool includes the full predicate_registry source via #[path] but reads only the capability tables"
)]
#[path = "../predicate_registry.rs"]
mod predicate_registry_dump;

#[derive(Serialize)]
struct PredicateCapabilitiesDump<'a> {
    canonical_predicates: Vec<&'a str>,
    aliases: BTreeMap<&'a str, &'a str>,
}

#[expect(
    clippy::print_stdout,
    reason = "capability-dump CLI writes the JSON capability table to stdout by design"
)]
fn main() -> Result<(), Box<dyn std::error::Error>> {
    let dump = PredicateCapabilitiesDump {
        canonical_predicates: predicate_registry_dump::PREDICATE_REGISTRY
            .iter()
            .map(|spec| spec.name)
            .collect(),
        aliases: predicate_registry_dump::PREDICATE_ALIASES
            .iter()
            .map(|spec| (spec.alias, spec.canonical))
            .collect(),
    };
    let json = serde_json::to_string_pretty(&dump)?;
    println!("{json}");
    Ok(())
}
