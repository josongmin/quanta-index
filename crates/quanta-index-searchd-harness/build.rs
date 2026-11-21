use std::collections::BTreeSet;
use std::env;
use std::error::Error;
use std::fmt::Write as _;
use std::fs;
use std::io::{self, Write as _};
use std::path::PathBuf;

fn main() -> Result<(), Box<dyn Error>> {
    const JUDGMENTS: &str = "src/relevance/semantic_judgments.tsv";
    const HEADER: &str = "id\tintent_kind\tintent\tquery\ton_topic_path\thard_negative_path";
    writeln!(io::stdout().lock(), "cargo:rerun-if-changed={JUDGMENTS}")?;
    let data = fs::read_to_string(JUDGMENTS)?;
    let mut lines = data.lines();
    if lines.next() != Some(HEADER) {
        return Err("semantic judgment columns differ".into());
    }
    let mut source = String::from("&[\n");
    let mut ids = BTreeSet::new();
    let mut exact = false;
    for (index, line) in lines.enumerate() {
        let columns = line.split('\t').collect::<Vec<_>>();
        let [id, kind, intent, query, on_topic_path, hard_negative_path] = columns.as_slice()
        else {
            return Err(format!("semantic judgment row {index} has the wrong columns").into());
        };
        let variant = match *kind {
            "exact_token" if !exact && id.ends_with(".exact") => {
                exact = true;
                "ExactToken"
            }
            "paraphrase" if id.ends_with(".paraphrase") => "Paraphrase",
            _ => return Err(format!("semantic judgment {id} has an invalid intent kind").into()),
        };
        if !id.starts_with("sem.")
            || !ids.insert(*id)
            || columns.iter().any(|value| value.trim().is_empty())
            || on_topic_path == hard_negative_path
        {
            return Err(format!("invalid semantic judgment {id}").into());
        }
        writeln!(
            source,
            "SemanticJudgedQuery {{ id: {id:?}, intent_kind: SemanticIntentKind::{variant}, \
             intent: {intent:?}, query: {query:?}, on_topic_path: {on_topic_path:?}, \
             hard_negative_path: {hard_negative_path:?} }},"
        )?;
    }
    if ids.len() != 13 || !exact {
        return Err("semantic benchmark requires one exact and twelve paraphrase judgments".into());
    }
    source.push_str("]\n");
    let out = PathBuf::from(env::var_os("OUT_DIR").ok_or("missing Cargo OUT_DIR")?);
    fs::write(out.join("semantic_judgments.rs"), source)?;
    Ok(())
}
