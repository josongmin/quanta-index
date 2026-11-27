# Unicode 17 lowercase reference

`lowercase.json` is reusable benchmark oracle data. It contains 1,488 nonidentity
per-character default lowercase mappings from the Unicode 17.0.0 UCD, with the
upstream URLs and input SHA-256 values embedded in the file. The Unicode License
V3 is retained in `LICENSE`.

Regeneration rule: read UnicodeData.txt field 13 as the simple lowercase mapping;
override with the lowercase field of unconditional SpecialCasing.txt entries
(the condition field must be empty). Omit identity mappings. Serialize uppercase
hexadecimal scalars, at least four digits, in scalar order using UTF-8 JSON with
an indentation of two spaces and a final newline. This excludes contextual and
locale casing; it is not full Unicode case folding.

The Python replay oracle checks the table SHA-256 and uses these mappings rather
than the host CPython Unicode version. A Rust owner test compares every valid
Unicode scalar's real text-normalizer output with this independent reference.
A Unicode/toolchain upgrade must preserve agreement or explicitly revise the
normalizer and benchmark contracts.
