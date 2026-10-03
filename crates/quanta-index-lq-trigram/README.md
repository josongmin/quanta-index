# Trigram index

Pure per-generation byte-trigram postings used to admit substring and regex
candidates before exact verification. This crate does not establish an
answer from a prefilter hit alone.

Start with [builder](src/builder.rs), [index](src/index.rs), [query](src/query.rs),
and [regex prefilter](src/regex_prefilter.rs). Query ownership is routed by
the [living documentation index](../../docs/ssot/README.md).
