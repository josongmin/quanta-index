//! Bounded-input limits enforced by the LQ parser, normalizer, and regex
//! guard.
//!
//! Source of truth is the RFC (`docs/plans/may-24-lexical-indexing-sorucegraph/rfc.md`)
//! § Capacity and SLO Targets, restated in `dsl.md` §13. Every limit here is
//! a hard parser/normalizer rejection threshold; exceeding any of them yields
//! a typed [`crate::errors::LqParseErrorCode`] variant — never a silent clip.

/// 16 KiB — maximum raw input length before tokenization. RFC rule 7.
pub const MAX_INPUT_BYTES: usize = 16 * 1024;

/// 32 — maximum nesting depth of the boolean AST during recursive descent.
pub const MAX_AST_DEPTH: u32 = 32;

/// 64 — maximum number of immediate children per `LqExpr::All` / `LqExpr::Any`
/// node after parser-level n-ary collapse.
pub const MAX_FANOUT_PER_NODE: usize = 64;

/// `100_000` — pre-compile upper bound on NFA states for `regexp` patterns.
pub const MAX_NFA_STATES: u32 = 100_000;

/// 256 — maximum number of structural-pattern nodes inside a `match { ... }`
/// body before the structural guard rejects.
pub const MAX_STRUCTURAL_NODES: u32 = 256;

#[cfg(test)]
mod tests {
    use super::{
        MAX_AST_DEPTH, MAX_FANOUT_PER_NODE, MAX_INPUT_BYTES, MAX_NFA_STATES, MAX_STRUCTURAL_NODES,
    };

    #[test]
    fn input_bytes_is_16_kib() {
        assert_eq!(MAX_INPUT_BYTES, 16_384);
    }

    #[test]
    fn ast_depth_is_thirty_two() {
        assert_eq!(MAX_AST_DEPTH, 32);
    }

    #[test]
    fn fanout_per_node_is_sixty_four() {
        assert_eq!(MAX_FANOUT_PER_NODE, 64);
    }

    #[test]
    fn nfa_states_is_one_hundred_thousand() {
        assert_eq!(MAX_NFA_STATES, 100_000);
    }

    #[test]
    fn structural_nodes_is_two_hundred_fifty_six() {
        assert_eq!(MAX_STRUCTURAL_NODES, 256);
    }
}
