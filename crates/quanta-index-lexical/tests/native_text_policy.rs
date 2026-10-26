use quanta_index_contract::index::Digest;
use quanta_index_lexical::native::TextRecipe;

/// The cross-process SDK fixture uses the same fixed recipe without linking
/// this SQLite-backed adapter into the producer's separate `SQLite` graph.
#[test]
fn native_full_process_fixture_text_policy_matches_recipe_contract() {
    assert_eq!(
        TextRecipe::new(Digest([31; 32])).output_policy(),
        Digest([
            164, 36, 226, 152, 206, 74, 120, 3, 184, 125, 242, 187, 242, 4, 45, 6, 126, 3, 205, 20,
            74, 254, 220, 28, 122, 154, 163, 174, 38, 83, 119, 46,
        ])
    );
}
