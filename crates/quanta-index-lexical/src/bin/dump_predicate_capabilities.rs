use std::collections::BTreeMap;

use serde::ser::SerializeStruct;
use serde::{Serialize, Serializer};

#[expect(
    dead_code,
    reason = "dev tool includes the full predicate_registry source via #[path] but reads only the capability tables"
)]
#[path = "../predicate_registry.rs"]
mod predicate_registry_dump;

struct PredicateCapabilitiesDump<'a> {
    canonical_predicates: Vec<&'a str>,
    aliases: BTreeMap<&'a str, &'a str>,
}

impl Serialize for PredicateCapabilitiesDump<'_> {
    fn serialize<S>(&self, serializer: S) -> Result<S::Ok, S::Error>
    where
        S: Serializer,
    {
        let mut state = serializer.serialize_struct("PredicateCapabilitiesDump", 2)?;
        state.serialize_field("canonical_predicates", &self.canonical_predicates)?;
        state.serialize_field("aliases", &self.aliases)?;
        state.end()
    }
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
