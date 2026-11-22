//! Structural trait fixture for the bounded-query regressions.

use anyhow::Result as AnyResult;
use quanta_index_contract::lex::ParseTreeRecord;

pub(super) fn structural_trait_record(content: &str) -> AnyResult<ParseTreeRecord> {
    use quanta_index_contract::lex::compute_parse_tree_source_hash;
    use quanta_index_contract::lex::{LanguageCode, ParseNode};

    let identifier = "chaos_structural_trait";
    let identifier_start = content.find(identifier).ok_or_else(|| {
        anyhow::anyhow!("e2e chaos trait identifier `{identifier}` missing from `{content}`")
    })?;
    let identifier_end = identifier_start.saturating_add(identifier.len());
    let byte_end = u32::try_from(content.len())
        .map_err(|err| anyhow::anyhow!("structural trait content overflow: {err}"))?;
    let identifier_start = u32::try_from(identifier_start)
        .map_err(|err| anyhow::anyhow!("structural trait identifier start overflow: {err}"))?;
    let identifier_end = u32::try_from(identifier_end)
        .map_err(|err| anyhow::anyhow!("structural trait identifier end overflow: {err}"))?;
    let block_start = byte_end.saturating_sub(2);
    Ok(ParseTreeRecord {
        wire_version: 1,
        lang: LanguageCode::new("rust")
            .map_err(|err| anyhow::anyhow!("invalid rust lang code: {err}"))?,
        root: ParseNode {
            kind: "trait_item".to_string().into_boxed_str(),
            byte_start: 0,
            byte_end,
            children: vec![
                ParseNode {
                    kind: "identifier".to_string().into_boxed_str(),
                    byte_start: identifier_start,
                    byte_end: identifier_end,
                    children: Vec::new(),
                },
                ParseNode {
                    kind: "block".to_string().into_boxed_str(),
                    byte_start: block_start,
                    byte_end,
                    children: Vec::new(),
                },
            ],
        },
        source_hash: compute_parse_tree_source_hash(content),
        role_tag_schema_version: 1,
        role_tags: Vec::new(),
    })
}
