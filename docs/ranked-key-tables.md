# Ranked lexical keys

Lexical generation manifest format 8 commits one `ranked-keys-<segment-id>.bin`
file per Tantivy segment. The seal derives each file from the segment's
`repo_id`, `repo_relative_path`, and `candidate_id` dictionaries after the
final index commit. A delta hard-links tables for unchanged segments and
builds tables for new or merged segments. Retired segment tables are removed.

Activation validation and query open read every table, check its committed
length and SHA-256, bind its segment ID to the committed index, and validate
term counts, offsets, UTF-8, and strict key order. A missing, extra, malformed,
or changed table refuses the generation. Grouping keeps segment-local
ordinals; ranked pages compare borrowed keys before retaining a row. The
collector reserves retained strings against its request collection budget.
It does not call Tantivy's SSTable ordinal-to-string decoder during a query.

The existing index-segment policy still checks segment length at open and
content at seal/scrub. A same-length mutation of Tantivy fast-field bytes
between scrubs is outside the new table's digest and retains that existing
integrity window.

The table's raw bytes are resident in the opened searcher and included once
in its reported generation size. The total committed table size is capped at
64 MiB per generation; sealing or opening a larger generation refuses it.
The snapshot registry separately admits the complete generation estimate,
including Tantivy and authority data. Sidecar construction at seal still uses
Tantivy's public dictionary stream and can allocate while preparing a new
segment; the request budget does not govern seal work or index open. This
change does not claim a process-wide RSS bound.

Format 7 generations require a rebuild: their manifest does not commit the
table that format 8 queries require. The private `tantivy-sstable` patch and
its CI credential are no longer part of the build. The old private repository
may be retained as an archive; no runtime path depends on it.
