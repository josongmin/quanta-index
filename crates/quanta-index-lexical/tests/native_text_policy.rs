use quanta_index_contract::index::Digest;
use quanta_index_lexical::native::TextRecipe;

/// The cross-process SDK fixture uses the same fixed recipe without linking
/// this SQLite-backed adapter into the producer's separate `SQLite` graph.
/// Fixed SHA-256 includes the Small v2 layout bytes after the occurrence layout.
#[test]
fn native_full_process_fixture_text_policy_matches_recipe_contract() {
    assert_eq!(
        TextRecipe::new(Digest([31; 32])).output_policy(),
        Digest([
            11, 34, 232, 218, 194, 139, 191, 146, 78, 100, 117, 64, 140, 141, 169, 115, 202, 243,
            45, 220, 179, 43, 208, 223, 133, 230, 168, 17, 185, 192, 175, 197,
        ])
    );
}
