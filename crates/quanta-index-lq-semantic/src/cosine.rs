//! Cosine similarity kernel.
//!
//! Closed form: `cosine(a, b) = dot(a, b) / (norm(a) * norm(b))`. This
//! file is the authoritative kernel; the executor in [`crate::query`]
//! calls it, never reimplements the formula.
//!
//! Validation gates (all returning typed [`SemanticError`] — no silent
//! NaN propagation per SEM-01 spec §5.3):
//!
//! 1. dim mismatch → [`SemanticErrorCode::SemDimMismatch`],
//! 2. NaN / `+inf` / `-inf` in either input → [`SemanticErrorCode::SemInvalidVector`],
//! 3. zero-norm input → [`SemanticErrorCode::SemInvalidVector`] (cosine
//!    is undefined for the zero vector).

use crate::errors::{SemanticError, SemanticErrorCode};

/// Compute the cosine similarity of two equal-dimension f32 slices.
///
/// Returns a value in `[-1.0, 1.0]` (modulo `f32` rounding). The result
/// may very slightly exceed the closed interval due to floating-point
/// rounding; callers that need a strict bound should clamp at the call
/// site.
pub fn cosine_similarity(a: &[f32], b: &[f32]) -> Result<f32, SemanticError> {
    if a.len() != b.len() {
        return Err(SemanticError::new(
            SemanticErrorCode::SemDimMismatch,
            format!(
                "cosine dim mismatch: a.len()={} vs b.len()={}",
                a.len(),
                b.len()
            ),
        ));
    }
    if a.is_empty() {
        return Err(SemanticError::new(
            SemanticErrorCode::SemInvalidVector,
            "cosine on zero-length vector",
        ));
    }

    // f64 accumulators bound rounding error on the dot and norms.
    let mut dot: f64 = 0.0;
    let mut norm_a: f64 = 0.0;
    let mut norm_b: f64 = 0.0;
    for (i, (av, bv)) in a.iter().zip(b.iter()).enumerate() {
        if !av.is_finite() {
            return Err(SemanticError::new(
                SemanticErrorCode::SemInvalidVector,
                format!("cosine: a[{i}] is non-finite: {av}"),
            ));
        }
        if !bv.is_finite() {
            return Err(SemanticError::new(
                SemanticErrorCode::SemInvalidVector,
                format!("cosine: b[{i}] is non-finite: {bv}"),
            ));
        }
        let av64 = f64::from(*av);
        let bv64 = f64::from(*bv);
        dot += av64 * bv64;
        norm_a += av64 * av64;
        norm_b += bv64 * bv64;
    }

    if norm_a <= 0.0 {
        return Err(SemanticError::new(
            SemanticErrorCode::SemInvalidVector,
            "cosine: left vector has zero norm",
        ));
    }
    if norm_b <= 0.0 {
        return Err(SemanticError::new(
            SemanticErrorCode::SemInvalidVector,
            "cosine: right vector has zero norm",
        ));
    }

    let denom = norm_a.sqrt() * norm_b.sqrt();
    if !denom.is_finite() || denom <= 0.0 {
        return Err(SemanticError::new(
            SemanticErrorCode::SemInvalidVector,
            "cosine: norm product underflowed to non-finite or zero",
        ));
    }

    let sim_f64 = dot / denom;
    // Down-cast to f32. The result is bounded by [-1.0, 1.0] modulo
    // rounding, so the cast is lossless in magnitude terms; we still
    // round-trip through `as` only via the `f64::from`-style direction,
    // not the other way. Use try-narrow guard.
    let sim_f32 = narrow_f64_to_f32(sim_f64)?;
    Ok(sim_f32)
}

/// Narrow an `f64` to `f32`, failing closed if the result would be
/// non-finite. The cosine kernel above only produces finite `f64` values
/// when both norms are positive, so this guard is defense-in-depth.
fn narrow_f64_to_f32(v: f64) -> Result<f32, SemanticError> {
    if !v.is_finite() {
        return Err(SemanticError::new(
            SemanticErrorCode::SemInvalidVector,
            format!("cosine: narrowed value is non-finite: {v}"),
        ));
    }
    // Manual round-via-format guard: convert via string parse, which is
    // lossy but bounded. Using `as` here would trip clippy::as_conversions;
    // we instead use the explicit `From<f32>`-inverse via `f32::from_bits`
    // round-trip would require unsafe-like reinterpretation. Use string
    // parse for a clippy-clean, fallible narrow. The performance hit is
    // negligible because this fires once per (query, doc) pair, and the
    // dominant cost is the dot/norm loop.
    let s = format!("{v}");
    match s.parse::<f32>() {
        Ok(f) if f.is_finite() => Ok(f),
        Ok(_) => Err(SemanticError::new(
            SemanticErrorCode::SemInvalidVector,
            format!("cosine: f64->f32 narrow produced non-finite: {v}"),
        )),
        Err(e) => Err(SemanticError::new(
            SemanticErrorCode::SemInvalidVector,
            format!("cosine: f64->f32 narrow failed: {e}"),
        )),
    }
}

#[cfg(test)]
mod tests {
    use super::cosine_similarity;
    use crate::errors::SemanticErrorCode;

    fn approx_eq(a: f32, b: f32, eps: f32) -> bool {
        (a - b).abs() <= eps
    }

    #[test]
    fn identical_unit_vectors_yield_one() {
        let v = [1.0_f32, 0.0_f32, 0.0_f32];
        let s = match cosine_similarity(&v, &v) {
            Ok(s) => s,
            Err(e) => {
                assert!(false, "{e}");
                return;
            }
        };
        assert!(approx_eq(s, 1.0, 1e-6), "expected 1.0, got {s}");
    }

    #[test]
    fn orthogonal_vectors_yield_zero() {
        let a = [1.0_f32, 0.0_f32];
        let b = [0.0_f32, 1.0_f32];
        let s = match cosine_similarity(&a, &b) {
            Ok(s) => s,
            Err(e) => {
                assert!(false, "{e}");
                return;
            }
        };
        assert!(approx_eq(s, 0.0, 1e-6), "expected 0.0, got {s}");
    }

    #[test]
    fn opposite_vectors_yield_minus_one() {
        let a = [1.0_f32, 0.0_f32];
        let b = [-1.0_f32, 0.0_f32];
        let s = match cosine_similarity(&a, &b) {
            Ok(s) => s,
            Err(e) => {
                assert!(false, "{e}");
                return;
            }
        };
        assert!(approx_eq(s, -1.0, 1e-6), "expected -1.0, got {s}");
    }

    #[test]
    fn dim_mismatch_errors() {
        let a = [1.0_f32, 0.0_f32];
        let b = [1.0_f32];
        match cosine_similarity(&a, &b) {
            Ok(_) => assert!(false, "must reject mismatched dim"),
            Err(e) => assert_eq!(e.code, SemanticErrorCode::SemDimMismatch),
        }
    }

    #[test]
    fn nan_left_errors() {
        let a = [1.0_f32, f32::NAN];
        let b = [1.0_f32, 0.0_f32];
        match cosine_similarity(&a, &b) {
            Ok(_) => assert!(false, "must reject NaN"),
            Err(e) => assert_eq!(e.code, SemanticErrorCode::SemInvalidVector),
        }
    }

    #[test]
    fn nan_right_errors() {
        let a = [1.0_f32, 0.0_f32];
        let b = [1.0_f32, f32::NAN];
        match cosine_similarity(&a, &b) {
            Ok(_) => assert!(false, "must reject NaN"),
            Err(e) => assert_eq!(e.code, SemanticErrorCode::SemInvalidVector),
        }
    }

    #[test]
    fn inf_errors() {
        let a = [1.0_f32, f32::INFINITY];
        let b = [1.0_f32, 0.0_f32];
        match cosine_similarity(&a, &b) {
            Ok(_) => assert!(false, "must reject +inf"),
            Err(e) => assert_eq!(e.code, SemanticErrorCode::SemInvalidVector),
        }
    }

    #[test]
    fn zero_norm_left_errors() {
        let a = [0.0_f32, 0.0_f32];
        let b = [1.0_f32, 0.0_f32];
        match cosine_similarity(&a, &b) {
            Ok(_) => assert!(false, "must reject zero-norm left"),
            Err(e) => assert_eq!(e.code, SemanticErrorCode::SemInvalidVector),
        }
    }

    #[test]
    fn zero_norm_right_errors() {
        let a = [1.0_f32, 0.0_f32];
        let b = [0.0_f32, 0.0_f32];
        match cosine_similarity(&a, &b) {
            Ok(_) => assert!(false, "must reject zero-norm right"),
            Err(e) => assert_eq!(e.code, SemanticErrorCode::SemInvalidVector),
        }
    }

    #[test]
    fn empty_inputs_error_as_invalid_vector() {
        let a: [f32; 0] = [];
        let b: [f32; 0] = [];
        match cosine_similarity(&a, &b) {
            Ok(_) => assert!(false, "must reject empty"),
            Err(e) => assert_eq!(e.code, SemanticErrorCode::SemInvalidVector),
        }
    }

    #[test]
    fn symmetry_holds_for_simple_pair() {
        let a = [3.0_f32, 4.0_f32];
        let b = [4.0_f32, 3.0_f32];
        let lhs = match cosine_similarity(&a, &b) {
            Ok(v) => v,
            Err(e) => {
                assert!(false, "{e}");
                return;
            }
        };
        let rhs = match cosine_similarity(&b, &a) {
            Ok(v) => v,
            Err(e) => {
                assert!(false, "{e}");
                return;
            }
        };
        assert!(approx_eq(lhs, rhs, 1e-7), "cosine should be symmetric");
    }
}
