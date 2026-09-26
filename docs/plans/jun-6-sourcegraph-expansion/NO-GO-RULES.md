# NO-GO-RULES

> Archive status: `Historical program record`. Current architecture: [JUN-06-001](../../adr/JUN-06-001-sourcegraph-compatibility-boundary.md). Archive map: [Completed Plan Archive](../ARCHIVE-INDEX.md).


Do not do any of the following in this packet.

- do not reopen `jun-4-sourcegraph-parity` or `jun-5-sourcegraph-tail-gaps` as
  if they were incomplete
- do not reopen already-landed `jun-6` cells as fake backlog
  - `repo:has.file(path:... content:...)`
  - `repo:has.description(...)`
  - `repo:has.meta(key)`
  - `repo:has.meta(tag:)`
  - `repo:has.meta(/key/)`
  - `repo:has.meta(/key/:)`
  - `repo:has.meta(key:/value/)`
  - `repo:has.meta(/key/:value)`
  - `repo:has.meta(/key/:/value/)`
  - `file:has.contributor(<name-or-email regex>)`
  - SG structural direct lexical `Phrase` sibling
  - SG structural direct lexical `Regex` sibling
- do not relabel current explicit unsupported cells as regressions without a new
  owner seam
- do not claim support from parser admission alone
- do not claim structural support from lowering preserve alone
- do not fake contributor regex support by matching against one opaque canonical
  string
- do not remove negative rails or machine-checked unsupported inventory before
  replacement proof exists
- do not leave a cell in ambiguous `부분 지원` state
- do not report clean preflight when the persona/session scripts are absent

If a widening cannot be implemented on the real owner seam, keep the surface
explicit `미지원`.
