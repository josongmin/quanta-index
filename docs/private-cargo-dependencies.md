# Private Cargo dependencies

The `tantivy-sstable` 0.3.0 memory-budget patch lives in
<https://github.com/josongmin/quanta-tantivy-sstable>. `Cargo.toml` pins a full
commit and `Cargo.lock` records the resolved Git source. Its Rust sources were
extracted unchanged from `vendor/tantivy-sstable` at quanta-index commit
`5571132655a83824731e7909b0e310951edad52b`. Patch documentation, upstream
provenance, authors and license are maintained in the separate repository.

## Local development

Use a GitHub account with read access to both repositories. Configure Git's
HTTPS credential helper (for GitHub CLI users, `gh auth setup-git`). Cargo uses
the Git CLI through `.cargo/config.toml`; credentials are never stored in the
manifest or lockfile. A separate sibling checkout is not required.

Run `./scripts/cargow metadata --locked --format-version 1` to verify access and
resolution. An offline build requires the pinned revision to be cached first.

## GitHub Actions

The `QUANTA_TANTIVY_SSTABLE_READ_KEY` repository secret holds a deploy key with
read-only access to the SSTable repository. The local
`.github/actions/private-cargo-dependencies` action checks out the exact revision
without retaining credentials, verifies HEAD, and redirects that repository's
Cargo fetches to the checkout. Other Git dependencies are unaffected.

Jobs that resolve Cargo dependencies must invoke that action after checking out
quanta-index. Jobs without access to the secret cannot fetch this private
dependency; fork and Dependabot workflows need an explicitly authorized source
of credentials. No unauthenticated or upstream fallback is used.

For an upgrade, push the reviewed patch to the separate repository, update the
full `rev` in `Cargo.toml`, regenerate the lockfile, and run the lexical tests.
Historical receipts that bound the old vendor path do not verify the new source.
