use quanta_index_contract::index::Digest;
use quanta_index_lexical::native::TextRecipe;

/// Freeze the Tree policy used by cross-process producers that cannot link
/// this SQLite-backed adapter. The digest includes the term dictionary and
/// path postings layouts.
#[test]
fn native_full_process_fixture_text_policy_matches_recipe_contract() {
    assert_eq!(
        TextRecipe::new(Digest([31; 32])).output_policy(),
        Digest([
            178, 98, 80, 7, 6, 191, 13, 132, 28, 161, 200, 151, 240, 0, 81, 192, 11, 103, 195, 202,
            27, 213, 160, 212, 193, 212, 210, 3, 107, 248, 216, 173,
        ])
    );
}
