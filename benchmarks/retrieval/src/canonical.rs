//! Canonical JSON authority shared by batch digests and runner records.

use serde_json::Value;

use crate::{BenchError, BenchResult};

/// Canonical JSON: sorted keys, no whitespace, raw UTF-8, no floats.
/// Matches `evaluator.canonical` for the pack's string/int-only domain;
/// floats are refused rather than format-guessed.
pub fn canonical_json(value: &Value) -> BenchResult<String> {
    fn render(value: &Value, out: &mut String) -> BenchResult<()> {
        match value {
            Value::Null => out.push_str("null"),
            Value::Bool(true) => out.push_str("true"),
            Value::Bool(false) => out.push_str("false"),
            Value::Number(number) => {
                if number.is_f64() {
                    return Err(BenchError::Protocol(
                        "canonical JSON refuses floats (Python float formatting would diverge)"
                            .to_string(),
                    ));
                }
                out.push_str(&number.to_string());
            }
            Value::String(text) => {
                let rendered = serde_json::to_string(text).map_err(|err| BenchError::Json {
                    path: "<pack>".to_string(),
                    message: err.to_string(),
                })?;
                out.push_str(&rendered);
            }
            Value::Array(items) => {
                out.push('[');
                for (index, item) in items.iter().enumerate() {
                    if index > 0 {
                        out.push(',');
                    }
                    render(item, out)?;
                }
                out.push(']');
            }
            Value::Object(object) => {
                out.push('{');
                let mut keys: Vec<&String> = object.keys().collect();
                keys.sort();
                for (index, key) in keys.iter().enumerate() {
                    if index > 0 {
                        out.push(',');
                    }
                    let rendered_key =
                        serde_json::to_string(key).map_err(|err| BenchError::Json {
                            path: "<pack>".to_string(),
                            message: err.to_string(),
                        })?;
                    out.push_str(&rendered_key);
                    out.push(':');
                    if let Some(child) = object.get(key.as_str()) {
                        render(child, out)?;
                    }
                }
                out.push('}');
            }
        }
        Ok(())
    }
    let mut out = String::new();
    render(value, &mut out)?;
    Ok(out)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn canonical_form_sorts_keys_and_holds_utf8() {
        let value: Value = serde_json::from_str(r#"{"b":1,"a":[2,{"z":null,"m":"héllo"}]}"#)
            .expect("fixture parses");
        let rendered = canonical_json(&value).expect("canonical renders");
        assert_eq!(rendered, r#"{"a":[2,{"m":"héllo","z":null}],"b":1}"#);
    }

    #[test]
    fn canonical_form_refuses_floats() {
        let value: Value = serde_json::from_str(r#"{"a":1.5}"#).expect("fixture parses");
        assert!(canonical_json(&value).is_err());
    }
}
