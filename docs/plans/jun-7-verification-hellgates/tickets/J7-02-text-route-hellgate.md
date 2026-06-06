# J7-02 — Text Route Hellgate

Status: `landed`

Goal:

- add a small runtime rail for promoted text-route surfaces

Owner seam:

- `crates/quanta-index-searchd-runtime/tests/e2e_text_route_hellgate.rs`

Covered families:

- bench-owned text subset from `SCENARIOS`
- `repo:has.meta` regex family
- `repo:has.description`
- `repo:has.file(path:... content:...)`
- `repo:has.topic(...)`
- `repo:has.commit.after(...)` / `repo:contains.commit.after(...)`
- legacy Sourcegraph directive carrier rails
  - `index:no` executes on active lexical rail
  - `boost:` executes on active lexical rail and changes score magnitude
- scoped `file:contains(...)` / `file:has.content(...)`
  - `name:` scope
  - boolean `OR`
  - boolean `NOT`
- `file:has.owner`
- `file:has.contributor`
- `select:file.owners`
- `rev:at.time`

DoD:

- positive, miss, and typed-fail branches exist where applicable
