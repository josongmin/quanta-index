use std::collections::{BTreeMap, BTreeSet};
use std::env;
use std::fmt::{self, Write as _};
use std::fs;
use std::path::PathBuf;

use quanta_index_lq_text_normalizer::{CaseMode, apply_case, nfc};
use unicode_normalization::char::{canonical_combining_class, decompose_canonical};

fn stable_starter(scalar: char) -> bool {
    let mut parts = 0;
    decompose_canonical(scalar, |_| parts += 1);
    parts == 1 && canonical_combining_class(scalar) == 0
}

fn scalar(raw: char) -> String {
    format!("'\\u{{{:X}}}'", u32::from(raw))
}

fn write_set(output: &mut String, name: &str, set: &BTreeSet<char>) -> fmt::Result {
    writeln!(output, "static {name}: &[char] = &[")?;
    for &character in set {
        writeln!(output, "    {},", scalar(character))?;
    }
    writeln!(output, "];\n")
}

fn write_variants(
    output: &mut String,
    name: &str,
    variants: &BTreeMap<char, Vec<char>>,
) -> fmt::Result {
    writeln!(output, "static {name}: &[(char, &[char])] = &[")?;
    for (&character, raws) in variants {
        write!(output, "    ({}, &[", scalar(character))?;
        for &raw in raws {
            write!(output, "{}, ", scalar(raw))?;
        }
        writeln!(output, "]),")?;
    }
    writeln!(output, "];\n")
}

fn generate_case(output: &mut String, prefix: &str, case: CaseMode) -> fmt::Result {
    let mut exceptions: BTreeMap<char, Vec<char>> = BTreeMap::new();
    let mut unsafe_chars = BTreeSet::new();
    let mut non_identity = BTreeSet::new();
    let mut buffer = [0_u8; 4];

    // This is the query verifier's exact scalar policy, evaluated at compile
    // time. Ordinary identity mappings are implicit in the generated table.
    for raw in (0..=u32::from(char::MAX)).filter_map(char::from_u32) {
        let normalized = nfc(raw.encode_utf8(&mut buffer));
        let image = apply_case(normalized.as_ref(), case);
        let mut normalized_chars = normalized.chars();
        let mut image_chars = image.chars();
        let safe_image = match (
            normalized_chars.next(),
            normalized_chars.next(),
            image_chars.next(),
            image_chars.next(),
        ) {
            (Some(scalar), None, Some(character), None) if stable_starter(scalar) => {
                Some(character)
            }
            _ => None,
        };
        if safe_image != Some(raw) {
            let _inserted = non_identity.insert(raw);
        }
        match safe_image {
            Some(character) if character != raw => {
                exceptions.entry(character).or_default().push(raw);
            }
            Some(_) => {}
            None => unsafe_chars.extend(image.chars()),
        }
    }

    exceptions.retain(|character, _| !unsafe_chars.contains(character));
    non_identity.retain(|character| !unsafe_chars.contains(character));
    for (&character, raws) in &mut exceptions {
        if !non_identity.contains(&character) {
            raws.push(character);
            raws.sort_unstable();
        }
    }
    write_variants(output, &format!("{prefix}_VARIANTS"), &exceptions)?;
    write_set(output, &format!("{prefix}_UNSAFE"), &unsafe_chars)?;
    write_set(output, &format!("{prefix}_NO_IDENTITY"), &non_identity)?;
    writeln!(
        output,
        "static {prefix}: CaseInverse = CaseInverse {{\n    variants: {prefix}_VARIANTS,\n    unsafe_chars: {prefix}_UNSAFE,\n    no_identity: {prefix}_NO_IDENTITY,\n}};\n"
    )
}

// Cargo directives are the build script's required stdout protocol.
#[allow(clippy::print_stdout)]
fn main() -> Result<(), Box<dyn std::error::Error>> {
    println!("cargo:rerun-if-changed=build.rs");
    println!("cargo:rerun-if-changed=../quanta-index-lq-text-normalizer/src");
    let mut output = String::from("// Generated from the verifier's Unicode policy by build.rs.\n");
    generate_case(&mut output, "SENSITIVE", CaseMode::Sensitive)?;
    generate_case(&mut output, "FOLDED", CaseMode::Folded)?;
    let destination = PathBuf::from(env::var("OUT_DIR")?).join("literal_case_inverse.rs");
    fs::write(destination, output)?;
    Ok(())
}
