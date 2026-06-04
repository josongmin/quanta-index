use std::collections::BTreeMap;

use serde::Serialize;

#[allow(dead_code, unreachable_pub)]
#[path = "../predicate_registry.rs"]
mod predicate_registry_dump;

#[derive(Serialize)]
struct PredicateCapabilitiesDump<'a> {
    canonical_predicates: Vec<&'a str>,
    aliases: BTreeMap<&'a str, &'a str>,
}

fn main() {
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
    println!(
        "{}",
        serde_json::to_string_pretty(&dump).expect("predicate capability dump must serialize")
    );
}
