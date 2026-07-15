use std::collections::BTreeMap;

use quanta_index_contract::{LqLeaf, LqStructuralBlock};
use serde::ser::SerializeStruct;
use serde::{Serialize, Serializer};

#[expect(
    dead_code,
    unreachable_pub,
    reason = "dev tool includes the full lowering source via #[path] but reads only the structural-leaf-verdict matrix; the included module exposes pub items that are unreachable in this bin"
)]
#[path = "../lowering.rs"]
mod lowering_dump;

struct StructuralLegalityDump<'a> {
    verdicts: BTreeMap<&'a str, &'a str>,
}

impl Serialize for StructuralLegalityDump<'_> {
    fn serialize<S>(&self, serializer: S) -> Result<S::Ok, S::Error>
    where
        S: Serializer,
    {
        let mut state = serializer.serialize_struct("StructuralLegalityDump", 1)?;
        state.serialize_field("verdicts", &self.verdicts)?;
        state.end()
    }
}

fn verdict_name(verdict: &lowering_dump::StructuralLeafVerdict<'_>) -> &'static str {
    match verdict {
        lowering_dump::StructuralLeafVerdict::PreserveLexical => "PreserveLexical",
        lowering_dump::StructuralLeafVerdict::LowerPhraseBody(_) => "LowerPhraseBody",
        lowering_dump::StructuralLeafVerdict::LowerRegexBody(_) => "LowerRegexBody",
        lowering_dump::StructuralLeafVerdict::TypedFail => "TypedFail",
    }
}

#[expect(
    clippy::print_stdout,
    reason = "structural-legality dump CLI writes the JSON verdict matrix to stdout by design"
)]
fn main() -> Result<(), Box<dyn std::error::Error>> {
    let leaves = [
        ("Keyword", LqLeaf::Keyword("keyword".to_string())),
        ("RawString", LqLeaf::RawString("raw".to_string())),
        ("Phrase", LqLeaf::Phrase("phrase".to_string())),
        ("Regex", LqLeaf::Regex("regex".to_string())),
        (
            "StructuralBlock",
            LqLeaf::StructuralBlock(LqStructuralBlock {
                lang: None,
                nodes: Vec::new(),
                exprs: Vec::new(),
            }),
        ),
        (
            "Predicate",
            LqLeaf::Predicate {
                name: "repo.has.file".to_string(),
                args: Vec::new(),
            },
        ),
    ];
    let dump = StructuralLegalityDump {
        verdicts: leaves
            .into_iter()
            .map(|(name, leaf)| {
                (
                    name,
                    verdict_name(&lowering_dump::structural_leaf_verdict(&leaf)),
                )
            })
            .collect(),
    };
    let json = serde_json::to_string_pretty(&dump)?;
    println!("{json}");
    Ok(())
}
