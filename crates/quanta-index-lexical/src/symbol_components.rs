//! ASCII camel/snake components used by the explicit symbol-component route.

/// Split one local name according to `camel-snake-v1`.
/// Non-ASCII names are outside this contract.
pub(crate) fn name_components(name: &str) -> Vec<String> {
    if !name.is_ascii() {
        return Vec::new();
    }
    let bytes = name.as_bytes();
    let mut result = Vec::new();
    let mut index = 0;
    while index < bytes.len() {
        let start = index;
        if bytes.get(index).is_some_and(u8::is_ascii_uppercase) {
            while bytes.get(index).is_some_and(u8::is_ascii_uppercase) {
                index = index.saturating_add(1);
            }
            if bytes.get(index).is_some_and(u8::is_ascii_lowercase) {
                if index.saturating_sub(start) > 1 {
                    index = index.saturating_sub(1);
                } else {
                    while bytes.get(index).is_some_and(u8::is_ascii_lowercase) {
                        index = index.saturating_add(1);
                    }
                }
            }
        } else if bytes.get(index).is_some_and(u8::is_ascii_lowercase) {
            while bytes.get(index).is_some_and(u8::is_ascii_lowercase) {
                index = index.saturating_add(1);
            }
        } else if bytes.get(index).is_some_and(u8::is_ascii_digit) {
            while bytes.get(index).is_some_and(u8::is_ascii_digit) {
                index = index.saturating_add(1);
            }
        } else {
            index = index.saturating_add(1);
            continue;
        }
        // The ASCII gate makes every byte boundary a character boundary.
        let Some(component) = name.get(start..index) else {
            return Vec::new();
        };
        result.push(component.to_ascii_lowercase());
    }
    result
}

/// Query components must already be canonical lower-case words.
pub(crate) fn query_components(query: &str) -> Option<Vec<String>> {
    if !quanta_index_contract::valid_code_search_component_query(query) {
        return None;
    }
    Some(query.split(' ').map(str::to_owned).collect())
}

pub(crate) fn contains_ordered_components(have: &[String], wanted: &[String]) -> bool {
    have.windows(wanted.len()).any(|window| window == wanted)
}

#[cfg(test)]
mod tests {
    use super::{contains_ordered_components, name_components, query_components};

    #[test]
    fn matches_independent_camel_snake_contract_examples() {
        for (name, expected) in [
            ("cleanUp", vec!["clean", "up"]),
            ("VersionInfo", vec!["version", "info"]),
            ("home_unix", vec!["home", "unix"]),
            (
                "TestUpdateAvailable_NoCurrentVersion",
                vec!["test", "update", "available", "no", "current", "version"],
            ),
            ("URLParser2", vec!["url", "parser", "2"]),
            ("r#match", vec!["r", "match"]),
            ("Méthode", vec![]),
        ] {
            assert_eq!(name_components(name), expected, "{name}");
        }
        let wanted = query_components("update available no").expect("valid query");
        assert!(contains_ordered_components(
            &name_components("TestUpdateAvailable_NoCurrentVersion"),
            &wanted
        ));
        assert!(!contains_ordered_components(
            &name_components("UpdateNoAvailable"),
            &wanted
        ));
    }

    #[test]
    fn rejects_noncanonical_and_ambiguous_component_requests() {
        for invalid in [
            "clean",
            "clean  up",
            "Clean up",
            "clean_up",
            "clean up ",
            "clean\tup",
            "café up",
        ] {
            assert!(query_components(invalid).is_none(), "{invalid:?}");
        }
    }
}
