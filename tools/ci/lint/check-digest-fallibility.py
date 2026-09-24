#!/usr/bin/env python3
"""Force public functions returning a digest shape to be fallible by signature.

Structural template borrowed from `tools/ci/lint/check-error-shape.py` (same
workspace-scan + line-based parser shape — no syntax-tree dependency).

A `pub fn ... -> [u8; N]` where `N` is one of the common digest sizes
(16, 20, 32, 48, 64) is a soft contract violation: when the internal codec
step the function depends on (ciborium encode, sha2 update, …) fails, the
implementer is forced into one of:

  * `panic!()` / `unwrap()` — banned by repo policy.
  * Heuristic fallback (e.g. zero-pad on encode failure) — not reliably
    covered by a general Semgrep pattern; this signature guard prevents the
    fallible codec path from requiring that shape.
  * Silent-default fallback — same prohibition family.

The fix is to lift the return type to `Result<[u8; N], _>` so the typed Err
can propagate through `?`. This exact regression landed in
`crates/quanta-index-lq-ranker/src/weights.rs::weights_hash` v0 (originally
`-> [u8; 32]` with a heuristic fallback) and was rewritten to
`-> Result<[u8; 32], RankerError>`. This lint prevents the v0 anti-pattern
from re-landing.

Rule (authoritative):

  Any `pub fn` (including `pub(crate)` / `pub(super)`) in
  `crates/*/src/**/*.rs` whose return type is the bare array shape
  `[u8; N]` for `N ∈ {16, 20, 32, 48, 64}` MUST satisfy one of:

    (a) return `Result<[u8; N], E>` — detected by the surrounding `Result<`
        token preceding the array literal in the return-type expression;

    (b) carry a `///` doc-comment block immediately above the function that
        contains the literal substring `infallible by construction`
        (case-insensitive).

Skipped:

  * `#[cfg(test)]` and `#[test]` items (function-level attribute lookup).
  * `mod tests { ... }` blocks (brace-depth tracked).
  * `mod <name> { ... }` blocks whose parent line carries `#[cfg(test)]`.
  * Lines inside `///` doc comments, `//!` inner-doc comments, and
    `/* ... */` block comments.

Out-of-scope (acceptable false negatives):

  * Const-named digest sizes: `[u8; DIGEST_LEN]` — we cannot resolve the
    const value without a real Rust parser. Flag literal-N only.
"""

from __future__ import annotations

import re
import sys
from dataclasses import dataclass
from pathlib import Path

try:
    import tomllib
except ModuleNotFoundError:  # pragma: no cover - Python < 3.11
    import tomli as tomllib  # type: ignore[no-redef]


ROOT = Path(__file__).resolve().parents[3]
WORKSPACE_TOML = ROOT / "Cargo.toml"
CRATES_DIR = ROOT / "crates"

DIGEST_SIZES: frozenset[int] = frozenset({16, 20, 32, 48, 64})

# Match a `pub fn` declaration start. Captures the visibility form so we know
# we're on a candidate line. We do NOT try to parse the return type from this
# regex — we re-scan the joined-signature string after stitching multi-line
# signatures.
PUB_FN_DECL_RE = re.compile(
    r"^\s*pub(?:\s*\([^)]*\))?\s+(?:async\s+|const\s+|unsafe\s+|extern\s+(?:\"[^\"]*\"\s+)?)*fn\b"
)

# Match a `[u8; N]` literal where N is one of the digest sizes. Whitespace is
# tolerated around the semicolon and inside the brackets.
DIGEST_ARRAY_RE = re.compile(r"\[\s*u8\s*;\s*(?P<n>\d+)\s*\]")


@dataclass(frozen=True)
class Violation:
    path: Path
    line: int
    message: str


@dataclass(frozen=True)
class DigestSite:
    """A pub fn that returns a digest-shaped array."""

    path: Path
    line: int  # 1-based line number of the `pub fn` declaration


def workspace_members() -> list[Path]:
    data = tomllib.loads(WORKSPACE_TOML.read_text(encoding="utf-8"))
    return [ROOT / m for m in data.get("workspace", {}).get("members", [])]


def crate_source_files() -> list[Path]:
    files: list[Path] = []
    for member in workspace_members():
        src = member / "src"
        if not src.is_dir():
            continue
        for rs in sorted(src.rglob("*.rs")):
            if any(part == "tests" for part in rs.parts):
                continue
            files.append(rs)
    return files


def strip_line_comment(line: str) -> str:
    """Remove a `//`-trailing comment from a line if present.

    Does not attempt to handle `//` appearing inside a string literal — the
    digest-shape patterns are syntactic enough that the false-positive risk
    is negligible. Doc-comment lines (`///` / `//!`) are filtered separately
    before this is called.
    """
    idx = line.find("//")
    if idx < 0:
        return line
    return line[:idx]


def collect_signature(lines: list[str], start: int) -> tuple[str, int]:
    """Stitch a (possibly multi-line) `pub fn` signature into a single string.

    Returns `(joined_signature_text, end_index)` where `end_index` is the
    index of the last line consumed (inclusive). Termination triggers:
      * a `{` at brace depth 0 outside of `<...>` (function body start), or
      * a `;` at brace depth 0 (trait method declaration / extern fn).
    Generic angle-brackets `< >` are tracked separately so that bounds like
    `Result<[u8; 32], E>` do not prematurely terminate.
    """
    parts: list[str] = []
    brace_depth = 0
    angle_depth = 0
    bracket_depth = 0
    paren_depth = 0
    end_idx = start

    for i in range(start, len(lines)):
        raw = lines[i]
        # Strip line comments so `// foo {` doesn't trigger termination.
        stripped_line = strip_line_comment(raw)
        parts.append(stripped_line)
        end_idx = i

        for ch in stripped_line:
            if ch == "(":
                paren_depth += 1
            elif ch == ")":
                paren_depth -= 1
            elif ch == "[":
                bracket_depth += 1
            elif ch == "]":
                bracket_depth -= 1
            elif ch == "<":
                angle_depth += 1
            elif ch == ">":
                if angle_depth > 0:
                    angle_depth -= 1
            elif ch == "{":
                if angle_depth == 0 and bracket_depth == 0 and paren_depth == 0:
                    brace_depth += 1
                    return " ".join(parts), end_idx
            elif ch == ";":
                if (
                    angle_depth == 0
                    and bracket_depth == 0
                    and paren_depth == 0
                    and brace_depth == 0
                ):
                    return " ".join(parts), end_idx

    return " ".join(parts), end_idx


def extract_return_type(signature: str) -> str | None:
    """Return the substring of `signature` after the `->` arrow and before the
    function body or trailing semicolon. Returns None if no arrow is found.
    """
    arrow = signature.find("->")
    if arrow < 0:
        return None
    after = signature[arrow + 2 :]

    # Trim trailing `{` / `;` / `where` clause. We want just the type
    # expression — where-clauses can contain `Result<...>` tokens that would
    # confuse the Result-wrap detector, so cut them off.
    cut_idx = len(after)
    # Find earliest of: `{`, `;`, or ` where ` token at top angle/bracket depth.
    angle_depth = 0
    bracket_depth = 0
    paren_depth = 0
    i = 0
    while i < len(after):
        ch = after[i]
        if ch == "<":
            angle_depth += 1
        elif ch == ">":
            if angle_depth > 0:
                angle_depth -= 1
        elif ch == "[":
            bracket_depth += 1
        elif ch == "]":
            bracket_depth -= 1
        elif ch == "(":
            paren_depth += 1
        elif ch == ")":
            paren_depth -= 1
        elif ch in "{;" and angle_depth == 0 and bracket_depth == 0 and paren_depth == 0:
            cut_idx = i
            break
        elif (
            angle_depth == 0
            and bracket_depth == 0
            and paren_depth == 0
            and after[i : i + 7] == " where "
        ):
            cut_idx = i
            break
        i += 1

    return after[:cut_idx].strip()


def return_type_has_digest(return_type: str) -> int | None:
    """If `return_type` is the owned digest shape `[u8; N]` (top-level or
    wrapped in `Result<...>`) with N in DIGEST_SIZES, return N. Otherwise
    return None.

    We INTENTIONALLY exclude:
      * reference return types like `&[u8; 32]` / `&'a [u8; 32]` — borrowing
        out an existing buffer cannot fail at the codec layer;
      * compound shapes where `[u8; N]` is buried as a generic argument
        of another container (`&BTreeMap<[u8; 32], _>`, `Vec<[u8; 32]>`,
        `(u32, [u8; 32])`, etc.) — these returns are not the digest-producing
        shape this lint targets;
      * `Option<[u8; N]>` — same reasoning; the Option already gives the
        caller a failure channel via `None`.

    Acceptable shapes (the function PRODUCES a digest as its result):
      * `[u8; N]`
      * `Result<[u8; N], _>`  (this is the form the lint is *steering* toward;
                               it still counts as a digest return site so the
                               site-count metric is honest)
    """
    rt = return_type.strip()
    # Reject reference returns: they cannot fail at the codec layer.
    if rt.startswith("&"):
        return None

    # Direct owned digest: `[u8; N]`.
    direct = DIGEST_ARRAY_RE.fullmatch(rt)
    if direct is not None:
        n = int(direct.group("n"))
        return n if n in DIGEST_SIZES else None

    # `Result< ... [u8; N] ..., E >` form: accept if the OUTER wrapper is
    # exactly `Result<...>` and the first generic arg is `[u8; N]`.
    if rt.startswith("Result<") and rt.endswith(">"):
        inner = rt[len("Result<") : -1]
        first_arg = _split_top_level_comma_first(inner).strip()
        m = DIGEST_ARRAY_RE.fullmatch(first_arg)
        if m is not None:
            n = int(m.group("n"))
            return n if n in DIGEST_SIZES else None

    return None


def _split_top_level_comma_first(s: str) -> str:
    """Return the substring of `s` up to the first top-level `,` (depth 0 in
    `<>`, `[]`, `()`), or the whole string if none exists.
    """
    angle = bracket = paren = 0
    for i, ch in enumerate(s):
        if ch == "<":
            angle += 1
        elif ch == ">":
            if angle > 0:
                angle -= 1
        elif ch == "[":
            bracket += 1
        elif ch == "]":
            bracket -= 1
        elif ch == "(":
            paren += 1
        elif ch == ")":
            paren -= 1
        elif ch == "," and angle == 0 and bracket == 0 and paren == 0:
            return s[:i]
    return s


def return_type_is_result_wrapped(return_type: str) -> bool:
    """True iff the outermost wrapper of `return_type` is `Result<...>` and
    the first generic argument is the digest array literal.

    The narrow shape check is intentional: we don't want
    `(_, [u8; 32])` or `Vec<[u8; 32]>` to count as "Result-wrapped".
    """
    rt = return_type.strip()
    if not (rt.startswith("Result<") and rt.endswith(">")):
        return False
    inner = rt[len("Result<") : -1]
    first_arg = _split_top_level_comma_first(inner).strip()
    return DIGEST_ARRAY_RE.fullmatch(first_arg) is not None


def has_infallible_doc(lines: list[str], decl_idx: int) -> bool:
    """Walk back from the declaration line over the contiguous `///` doc
    block (and tolerate `#[...]` attributes interleaved) looking for the
    case-insensitive substring `infallible by construction`.
    """
    needle = "infallible by construction"
    for offset in range(1, 200):  # generous block scan
        idx = decl_idx - offset
        if idx < 0:
            break
        line = lines[idx]
        stripped = line.strip()
        if stripped.startswith("///") or stripped.startswith("//!"):
            if needle in stripped.lower():
                return True
            continue
        if stripped.startswith("#["):
            # Attribute line — keep walking up.
            continue
        if not stripped:
            # Blank line — keep walking up, doc blocks can have blanks above.
            continue
        # Anything else terminates the contiguous doc-block scan.
        break
    return False


def is_test_context(lines: list[str], decl_idx: int, mod_test_depth_at: list[int]) -> bool:
    """Return True iff the declaration at `decl_idx` is inside a test context.

    Two channels:
      * an immediately-preceding `#[cfg(test)]` or `#[test]` attribute on the
        fn itself (scanned up over contiguous `#[...]` lines);
      * an enclosing `mod tests { ... }` block (precomputed test-mod brace
        depth provided via `mod_test_depth_at`).
    """
    if mod_test_depth_at[decl_idx] > 0:
        return True
    for offset in range(1, 8):
        idx = decl_idx - offset
        if idx < 0:
            break
        stripped = lines[idx].strip()
        if not stripped:
            continue
        if stripped.startswith("///") or stripped.startswith("//!") or stripped.startswith("//"):
            continue
        if stripped.startswith("#["):
            low = stripped.lower()
            if "cfg(test)" in low or low.startswith("#[test]") or low.startswith("#[ test ]"):
                return True
            continue
        break
    return False


def compute_test_mod_depth(lines: list[str]) -> list[int]:
    """For each line index, return the open-brace depth of enclosing
    `#[cfg(test)] mod <name> { ... }` blocks (or `mod tests { ... }` blocks).

    Implementation: walk file linearly, tracking a stack of test-mod brace
    depths. When we see a `mod <name> {` line whose immediately preceding
    non-blank/non-comment line is `#[cfg(test)]`, OR the mod name is exactly
    `tests`, push the current brace depth + 1 as the entry depth and mark
    every subsequent line as inside the test mod until we leave that depth.
    """
    n = len(lines)
    depth_at = [0] * n
    brace_depth = 0
    test_mod_stack: list[int] = []  # entry depth of each open test mod

    # Detect `mod <ident> {` (possibly with pub modifier).
    mod_open_re = re.compile(r"^\s*(?:pub(?:\s*\([^)]*\))?\s+)?mod\s+(?P<name>[A-Za-z_][\w]*)\s*\{")
    cfg_test_re = re.compile(r"#\[\s*cfg\s*\(\s*test\s*\)\s*\]")

    for i, raw in enumerate(lines):
        line = strip_line_comment(raw)
        # Record depth BEFORE we consume this line's braces — declarations on
        # the same line as `}` should still see themselves as "inside".
        depth_at[i] = sum(1 for _ in test_mod_stack)

        # Detect mod opener.
        mod_match = mod_open_re.match(line)
        is_test_mod_open = False
        if mod_match:
            mod_name = mod_match.group("name")
            if mod_name == "tests":
                is_test_mod_open = True
            else:
                # Look up to 6 non-blank lines for #[cfg(test)].
                seen = 0
                for back in range(1, 12):
                    j = i - back
                    if j < 0:
                        break
                    s = lines[j].strip()
                    if not s:
                        continue
                    seen += 1
                    if seen > 6:
                        break
                    if cfg_test_re.search(s):
                        is_test_mod_open = True
                        break
                    if s.startswith("//") or s.startswith("#["):
                        continue
                    break

        # Now update brace depth from this line.
        for ch in line:
            if ch == "{":
                brace_depth += 1
                if is_test_mod_open:
                    test_mod_stack.append(brace_depth)
                    is_test_mod_open = False  # only the first { counts
            elif ch == "}":
                if test_mod_stack and brace_depth == test_mod_stack[-1]:
                    test_mod_stack.pop()
                brace_depth -= 1

        # If after consuming braces we're still inside a test mod, mark line.
        depth_at[i] = len(test_mod_stack) + (1 if is_test_mod_open else 0)
        # Re-correct: depth_at[i] should reflect the state *during* this line.
        # If we opened a test mod on this very line, the open brace and the
        # `pub fn` (if any after it on same line) is *inside* the new mod.
        # The recomputation above captures that.

    return depth_at


def strip_block_comments(lines: list[str]) -> list[str]:
    """Replace `/* ... */` block-comment spans with whitespace of equal length
    so line numbers are preserved. Doc-comment lines (`///` / `//!`) are left
    intact — the infallibility-doc scanner needs them.
    """
    out: list[str] = []
    in_block = False
    for raw in lines:
        line_chars: list[str] = []
        i = 0
        while i < len(raw):
            if in_block:
                if raw[i : i + 2] == "*/":
                    line_chars.append("  ")
                    i += 2
                    in_block = False
                else:
                    line_chars.append(" ")
                    i += 1
            else:
                if raw[i : i + 2] == "/*":
                    line_chars.append("  ")
                    i += 2
                    in_block = True
                else:
                    line_chars.append(raw[i])
                    i += 1
        out.append("".join(line_chars))
    return out


def audit_file(path: Path) -> tuple[list[DigestSite], list[Violation]]:
    """Return (digest_return_sites, violations) for this file."""
    text = path.read_text(encoding="utf-8")
    # Every reported site needs both a public declaration and a literal digest
    # array. Avoid the character-by-character comment and brace scans otherwise.
    if "pub" not in text or "u8" not in text or DIGEST_ARRAY_RE.search(text) is None:
        return [], []
    raw_lines = text.splitlines()
    # Block comments stripped for parser scanning; the doc-comment scanner
    # uses the raw `lines` too so it can still see `///` lines verbatim.
    lines = strip_block_comments(raw_lines)
    sites: list[DigestSite] = []
    findings: list[Violation] = []

    test_depth = compute_test_mod_depth(lines)

    i = 0
    while i < len(lines):
        line = lines[i]
        if not PUB_FN_DECL_RE.match(line):
            i += 1
            continue

        # Stitch the full signature.
        signature, end_idx = collect_signature(lines, i)
        return_type = extract_return_type(signature)
        if return_type is None:
            i = end_idx + 1
            continue

        n = return_type_has_digest(return_type)
        if n is None:
            i = end_idx + 1
            continue

        # Found a digest-shape return. Record the site and check rules.
        site_line = i + 1
        sites.append(DigestSite(path, site_line))

        if is_test_context(raw_lines, i, test_depth):
            i = end_idx + 1
            continue

        result_wrapped = return_type_is_result_wrapped(return_type)
        if result_wrapped:
            i = end_idx + 1
            continue

        if has_infallible_doc(raw_lines, i):
            i = end_idx + 1
            continue

        findings.append(
            Violation(
                path,
                site_line,
                f"pub fn returning [u8; {n}] must return Result<[u8; {n}], _> "
                'or be documented "infallible by construction"',
            )
        )
        i = end_idx + 1

    return sites, findings


def main() -> int:
    files = crate_source_files()
    all_sites: list[DigestSite] = []
    all_violations: list[Violation] = []
    for path in files:
        sites, violations = audit_file(path)
        all_sites.extend(sites)
        all_violations.extend(violations)

    for v in all_violations:
        rel = v.path.relative_to(ROOT) if v.path.is_absolute() else v.path
        print(f"{rel}:{v.line}: {v.message}", file=sys.stderr)

    print(
        f"Scanned {len(files)} rs files, found {len(all_sites)} digest-shape "
        f"return sites, {len(all_violations)} violations."
    )
    return 1 if all_violations else 0


if __name__ == "__main__":
    raise SystemExit(main())
