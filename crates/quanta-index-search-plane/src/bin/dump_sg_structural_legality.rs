use std::collections::BTreeMap;

use quanta_index_contract::{LqLeaf, LqStructuralBlock};
use serde::Serialize;

#[allow(dead_code, unreachable_pub)]
#[path = "../lowering.rs"]
mod lowering_dump;

#[derive(Serialize)]
struct StructuralLegalityDump<'a> {
    verdicts: BTreeMap<&'a str, &'a str>,
}

fn verdict_name(verdict: lowering_dump::StructuralLeafVerdict<'_>) -> &'static str {
    match verdict {
        lowering_dump::StructuralLeafVerdict::PreserveLexical => "PreserveLexical",
        lowering_dump::StructuralLeafVerdict::LowerPhraseBody(_) => "LowerPhraseBody",
        lowering_dump::StructuralLeafVerdict::LowerRegexBody(_) => "LowerRegexBody",
        lowering_dump::StructuralLeafVerdict::TypedFail => "TypedFail",
    }
}

fn main() {
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
                    verdict_name(lowering_dump::structural_leaf_verdict(&leaf)),
                )
            })
            .collect(),
    };
    println!(
        "{}",
        serde_json::to_string_pretty(&dump).expect("structural legality dump must serialize")
    );
}
