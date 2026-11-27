//! Daemon acceptance for Literal-anchored `file.contains`.
//!
//! The service is composed like the product: the Text reader names the
//! Literal storage format as its sibling, and the registry requires exactly
//! one Literal instance beside every Text instance. CodeSearch `.text()`
//! lowers a bare term to `file.contains`; over more than 4,096 files that is
//! answered from the Literal grams. Every non-matching file body is deleted
//! before the query, so a successful answer proves only matching bodies were
//! verified.

use super::protected_revision::{
    AnchoredTextLayout, ProtectedRevision, anchored_service, anchored_service_with_layout,
    publish_and_seal, publish_and_seal_batches,
};
use super::*;
use quanta_index_contract::native_protocol::NativeTextPathResultV1;
use quanta_index_contract::{RepoId, RevisionId, TextQueryResponse};
use quanta_index_core::index::{
    SourcePathKey, SourcePathOwners, SourcePathRepositoryKey, SourceRecordOwner, owner_map_key,
    ports::PersistentMapPort,
};
use quanta_index_sdk::{ConnectOptions, QuantaIndex};
use std::collections::BTreeSet;

const REVISION: ProtectedRevision<'static> = ProtectedRevision {
    repo: "repo-anchor",
    revision: "rev-anchor",
    slot: 43,
    owner_tag: 0x61,
};
const FILES: usize = 4_097;
const FILES_PER_PUBLICATION: usize = 256;
const NEEDLE_FILES: [usize; 3] = [11, 2_000, 4_050];
const RARE_ORDINAL: usize = NEEDLE_FILES[0];
const CANDIDATE_QUERY: &str = "rare AND common AND file.contains('::needle') select:path";

fn body(ordinal: usize) -> String {
    if ordinal == RARE_ORDINAL {
        "let rare common = ::needle;\n".into()
    } else if NEEDLE_FILES.contains(&ordinal) {
        format!("let haystack{ordinal} = needle common;\n")
    } else if ordinal == FILES - 1 {
        "let value = filler;\n".into()
    } else {
        format!("let value{ordinal} = common;\n")
    }
}

fn path(ordinal: usize) -> String {
    format!("src/f{ordinal:04}.rs")
}

fn files() -> Vec<(String, Vec<u8>)> {
    (0..FILES)
        .map(|ordinal| (path(ordinal), body(ordinal).into_bytes()))
        .collect()
}

fn hex(bytes: impl IntoIterator<Item = u8>) -> String {
    bytes
        .into_iter()
        .map(|byte| format!("{byte:02x}"))
        .collect::<Vec<_>>()
        .join("")
}

/// Run the same Native LQ over the public SDK and CLI on the sealed view.
/// All nonmatching body objects have already been removed from the store.
fn public_4097_candidate_acceptance(
    socket: &std::path::Path,
    layout: AnchoredTextLayout,
) -> TestResult {
    let corpus = files();
    let common = corpus
        .iter()
        .filter(|(_, bytes)| bytes.windows(b"common".len()).any(|part| part == b"common"))
        .count();
    let rare = corpus
        .iter()
        .filter(|(_, bytes)| bytes.windows(b"rare".len()).any(|part| part == b"rare"))
        .count();
    let marker = corpus
        .iter()
        .filter(|(_, bytes)| {
            bytes
                .windows(b"::needle".len())
                .any(|part| part == b"::needle")
        })
        .count();
    let independently_matched = corpus
        .iter()
        .filter(|(_, bytes)| {
            [
                b"rare".as_slice(),
                b"common".as_slice(),
                b"::needle".as_slice(),
            ]
            .into_iter()
            .all(|needle| bytes.windows(needle.len()).any(|part| part == needle))
        })
        .map(|(path, _)| path.as_str())
        .collect::<Vec<_>>();
    assert_eq!(common, 4_096);
    assert_eq!(rare, 1);
    assert_eq!(marker, 1);
    assert_eq!(independently_matched, ["src/f0011.rs"]);

    let selection = ViewSelection::Latest {
        workspace: REVISION.source_scope(),
        epoch: 1,
    };
    let instance = layout.text_spec().instance;
    let sdk = NativeUdsClient::new(socket);
    let expected = sdk
        .search_text_paths(
            selection,
            instance,
            native_text_request(CANDIDATE_QUERY, 4),
            &budget(),
        )
        .map_err(|error| format!("public SDK Native literal-predicate query: {error}"))?;
    let ranking = &expected.ranked.ranking;
    assert!(ranking.exhaustive);
    assert_eq!(ranking.matched, 1);
    let [hit] = ranking.hits.as_slice() else {
        return Err("rare/common source predicate did not return one file".into());
    };
    assert_eq!(hit.path, independently_matched[0]);
    assert_eq!(expected.ranked.repositories, [RepoId::new(REVISION.repo)?]);
    let term_only = sdk
        .search_text_paths(
            selection,
            instance,
            native_text_request("rare AND common select:path", 4),
            &budget(),
        )
        .map_err(|error| format!("public SDK Native term-only reference query: {error}"))?;
    let [reference] = term_only.ranked.ranking.hits.as_slice() else {
        return Err("rare/common term reference did not return one file".into());
    };
    assert_eq!(reference.path, hit.path);
    assert_eq!(reference.score.get().to_bits(), hit.score.get().to_bits());
    // A regex has no Literal-gram anchor. This separately forces the two
    // posting terms to bound source verification to their one-file AND.
    let regex = sdk
        .search_text_paths(
            selection,
            instance,
            native_text_request("rare AND common AND /::needle/ select:path", 4),
            &budget(),
        )
        .map_err(|error| format!("public SDK Native regex-predicate query: {error}"))?;
    assert_eq!(regex.ranked.ranking.matched, 1);
    assert!(regex.ranked.ranking.exhaustive);
    let [regex_hit] = regex.ranked.ranking.hits.as_slice() else {
        return Err("rare/common regex did not return one file".into());
    };
    assert_eq!(regex_hit.path, hit.path);
    assert_eq!(regex_hit.score.get().to_bits(), hit.score.get().to_bits());

    // CodeSearch has a separate product grammar: these are three body
    // substrings, while the Native LQ above has two posting terms.
    let product_query = "content:rare content:common content:\"::needle\"";
    let product = QuantaIndex::connect(
        ConnectOptions::from_native_socket(socket.to_path_buf(), selection)
            .with_native_text_instance(instance),
    )
    .map_err(|error| format!("open protected CodeSearch SDK client: {error}"))?;
    let search_product = |query: &str| -> TestResult<TextQueryResponse> {
        product
            .lexical()
            .query()
            .text(query)
            .active(
                RepoId::new(REVISION.repo)?,
                RevisionId::new(REVISION.revision)?,
            )
            .top_k(4)
            .execute()
            .map_err(|error| {
                format!("public CodeSearch SDK anchored query {query:?}: {error}").into()
            })
    };
    let product_result = search_product(product_query)?;
    let common_first = search_product("content:common content:rare content:\"::needle\"")?;
    for response in [&product_result, &common_first] {
        assert_eq!(response.results.len(), 1);
        assert_eq!(response.results[0].repo_relative_path.as_str(), hit.path);
        assert_eq!(response.window.returned(), 1);
        assert!(response.window.outcome().is_exhausted());
        assert_eq!(response.window.has_more(), Some(false));
        assert!(response.next_cursor.is_none());
    }

    let scope = REVISION.source_scope();
    let workspace = hex([
        scope.tenant.0,
        scope.collection.0,
        scope.worktree.0,
        scope.slot.0,
    ]
    .into_iter()
    .flatten());
    let instance = hex(instance.0);
    let socket = socket.to_str().ok_or("socket path is not UTF-8")?;
    let mut stdout = Vec::new();
    let mut stderr = Vec::new();
    let exit = quanta_index_searchctl::run(
        [
            "search-text",
            "--socket",
            socket,
            "--workspace",
            &workspace,
            "--epoch",
            "1",
            "--instance",
            &instance,
            "--syntax",
            "native",
            "--query-text",
            CANDIDATE_QUERY,
            "--top-k",
            "4",
            "--output",
            "json",
        ],
        &mut stdout,
        &mut stderr,
    );
    if exit != std::process::ExitCode::SUCCESS {
        return Err(format!(
            "Native CLI candidate query failed: {}",
            String::from_utf8(stderr)?
        )
        .into());
    }
    let actual: NativeTextPathResultV1 = serde_json::from_slice(&stdout)?;
    assert_eq!(actual.ranked.ranking.view, ranking.view);
    assert_eq!(actual.ranked.ranking.matched, 1);
    assert!(actual.ranked.ranking.exhaustive);
    let [cli_hit] = actual.ranked.ranking.hits.as_slice() else {
        return Err("Native CLI candidate query did not return one file".into());
    };
    assert_eq!(cli_hit.path, hit.path);
    assert_eq!(cli_hit.score.get().to_bits(), hit.score.get().to_bits());
    assert_eq!(actual.ranked.repositories, expected.ranked.repositories);

    stdout.clear();
    stderr.clear();
    let exit = quanta_index_searchctl::run(
        [
            "code-search",
            "--socket",
            socket,
            "--repo",
            REVISION.repo,
            "--revision",
            REVISION.revision,
            "--query-text",
            product_query,
            "--top-k",
            "4",
            "--output",
            "json",
        ],
        &mut stdout,
        &mut stderr,
    );
    if exit != std::process::ExitCode::SUCCESS {
        return Err(format!(
            "CodeSearch CLI candidate query failed: {}",
            String::from_utf8(stderr)?
        )
        .into());
    }
    let product_cli: TextQueryResponse = serde_json::from_slice(&stdout)?;
    assert_eq!(product_cli, product_result);

    // The union has 4,097 possible source bodies and must refuse before a
    // partial result is rendered, even though all but three bodies are gone.
    let broad = "file.contains('common') OR file.contains('filler') select:path";
    let refused = sdk.search_text_paths(
        selection,
        layout.text_spec().instance,
        native_text_request(broad, 4),
        &budget(),
    );
    assert!(
        matches!(&refused, Err(NativeUdsError::Refused(error)) if error.code == NativeIndexErrorCode::ResourceLimited),
        "{refused:?}"
    );
    stdout.clear();
    stderr.clear();
    let exit = quanta_index_searchctl::run(
        [
            "search-text",
            "--socket",
            socket,
            "--workspace",
            &workspace,
            "--epoch",
            "1",
            "--instance",
            &instance,
            "--syntax",
            "native",
            "--query-text",
            broad,
            "--top-k",
            "4",
            "--output",
            "json",
        ],
        &mut stdout,
        &mut stderr,
    );
    assert_ne!(exit, std::process::ExitCode::SUCCESS);
    assert!(
        stdout.is_empty(),
        "rejected CLI query printed a partial result"
    );
    assert!(String::from_utf8(stderr)?.contains("ResourceLimited"));

    // A selective supplier need not be a singleton. Put the 4,096-hit term
    // first and require all three files from the narrower term's posting.
    let multiple_hits = search_product("content:common content:needle")?;
    let found = multiple_hits
        .results
        .iter()
        .map(|candidate| candidate.repo_relative_path.as_str())
        .collect::<BTreeSet<_>>();
    let expected = NEEDLE_FILES.map(path);
    assert_eq!(
        found,
        expected.iter().map(String::as_str).collect::<BTreeSet<_>>()
    );
    assert_eq!(multiple_hits.window.returned(), 3);
    assert!(multiple_hits.window.outcome().is_exhausted());
    assert!(multiple_hits.next_cursor.is_none());
    Ok(())
}

/// The companion rule refuses a Text profile without its Literal sibling at
/// staging, before any view can be published.
fn text_profile_without_literal_is_refused<P: AsRef<std::path::Path>>(
    client: &NativeUdsClient<P>,
    layout: AnchoredTextLayout,
) -> TestResult {
    let scope = WorkspaceKey {
        slot: SlotId([44; 16]),
        ..workspace()
    };
    let NativeIndexResponse::Created(_genesis) = client
        .request(
            NativeIndexRequest::CreateWorkspace {
                workspace: scope,
                epoch: 1,
            },
            &budget(),
        )
        .map_err(|error| format!("create no-Literal profile refusal workspace: {error}"))?
    else {
        return Err("refusal workspace was not created".into());
    };
    let mut key = operation(90);
    key.workspace = scope;
    let ingress = client
        .begin_input(key, &budget())
        .map_err(|error| format!("begin no-Literal profile refusal: {error}"))?;
    let refused = client.stage_profile(
        ingress,
        IndexProfile {
            instances: vec![layout.text_spec()],
        },
        &budget(),
    );
    assert!(
        matches!(&refused, Err(NativeUdsError::Refused(error))
            if error.code == NativeIndexErrorCode::InvalidInput),
        "{refused:?}"
    );
    client
        .discard_input(ingress, &budget())
        .map_err(|error| format!("discard no-Literal profile refusal ingress: {error}"))?;
    Ok(())
}

fn expected_owner(ordinal: usize) -> TestResult<OwnerKey> {
    let [high, low] = u16::try_from(ordinal)?.to_be_bytes();
    let mut stable = [REVISION.owner_tag; 16];
    stable[1] = high;
    stable[2] = low;
    Ok(OwnerKey {
        producer: operation(2).producer,
        stable_id: StableOwnerId(stable),
    })
}

/// Verify a plain term that matches every file, with the Source bodies intact.
fn verify_common_term_top_k(
    client: &NativeUdsClient<&std::path::Path>,
    view: quanta_index_contract::index::ViewRef,
    layout: AnchoredTextLayout,
) -> TestResult {
    const TOP_K: u32 = 16;
    let result = client
        .search_text_paths(
            ViewSelection::Exact(view),
            layout.text_spec().instance,
            native_text_request("let select:path", TOP_K),
            &budget(),
        )
        .map_err(|error| format!("4,097-file public Native common-term Top-K: {error}"))?;
    let ranking = &result.ranked.ranking;
    assert!(ranking.exhaustive);
    assert_eq!(ranking.matched, u64::try_from(FILES)?);

    // Every document contains `let` once. The needle rows have four tokens,
    // the others have three; the shorter documents tie and sort by path.
    let expected_paths: Vec<_> = (0..=usize::try_from(TOP_K)?)
        .filter(|&ordinal| ordinal != RARE_ORDINAL)
        .map(path)
        .collect();
    let observed_paths: Vec<_> = ranking.hits.iter().map(|hit| hit.path.as_str()).collect();
    assert_eq!(
        observed_paths,
        expected_paths
            .iter()
            .map(String::as_str)
            .collect::<Vec<_>>()
    );
    let documents = f64::from(u32::try_from(FILES)?);
    let avg_doc_length = (3.0 * documents + 3.0) / documents;
    let idf = (0.5 / (documents + 0.5)).ln_1p();
    let norm = 0.25 + 0.75 * 3.0 / avg_doc_length;
    let reciprocal_k1 = 1.2_f64.recip();
    let expected_score = idf * ((1.0 + reciprocal_k1) / (reciprocal_k1 + norm));
    for hit in &ranking.hits {
        assert_eq!(
            hit.score.get().to_bits(),
            expected_score.to_bits(),
            "{}",
            hit.path
        );
    }
    Ok(())
}

/// Resolve the sealed route and check unfiltered rank before removing bodies.
fn resolve_published_source_cut(
    root: &std::path::Path,
    socket: &std::path::Path,
    layout: AnchoredTextLayout,
) -> TestResult<quanta_index_contract::index::SourceCut> {
    let running = NativeServiceGuard::start(anchored_service_with_layout(root, socket, layout)?);
    let checked = (|| -> TestResult<_> {
        let client = NativeUdsClient::new(socket);
        let NativeIndexResponse::RootScopedView(resolved) = client.request(
            NativeIndexRequest::ResolveProtectedSourceHead {
                repo_id: RepoId::new(REVISION.repo)?,
                revision_id: RevisionId::new(REVISION.revision)?,
            },
            &budget(),
        )?
        else {
            return Err("protected Source route did not resolve".into());
        };
        let latest = client.read_view(
            ViewSelection::Latest {
                workspace: REVISION.source_scope(),
                epoch: 1,
            },
            &budget(),
        )?;
        assert_eq!(
            resolved.view, latest,
            "protected route differs from final Source cut"
        );
        verify_common_term_top_k(&client, latest.identity, layout)?;

        Ok(resolved.view.source)
    })();
    let stopped = running.stop();
    let source = checked?;
    stopped?;
    Ok(source)
}

/// Check every published owner, path, and SourceRecord before removing bodies
/// that cannot match the needle query.
fn verify_inventory_and_delete_non_matching_bodies(
    root: &std::path::Path,
    sources: &[ObjectRef],
    source: quanta_index_contract::index::SourceCut,
) -> TestResult {
    let store = FileObjectStore::open(root.join("native-objects"))?;
    let mut work = WorkBudget::with_request(
        work_limits(),
        RequestBudgetV1::for_duration(Duration::from_secs(180)),
    );
    let repository = RepoId::new(REVISION.repo)?;
    let repository_key = SourcePathRepositoryKey::new(Some(&repository))?;
    let mut after_owner: Option<Vec<u8>> = None;
    let mut owner_ordinal = 0;
    loop {
        let page = store.scan(source.owners, after_owner.as_deref(), 128, &mut work)?;
        assert!(
            !page.entries.is_empty(),
            "published owner map has an empty page"
        );
        for (key, reference) in &page.entries {
            assert!(
                owner_ordinal < sources.len(),
                "published extra Source owner"
            );
            assert_eq!(
                key.as_slice(),
                owner_map_key(expected_owner(owner_ordinal)?).as_slice()
            );
            assert_eq!(*reference.as_object_ref()?, sources[owner_ordinal]);
            owner_ordinal += 1;
        }
        if !page.has_more {
            break;
        }
        after_owner = page.entries.last().map(|(key, _)| key.clone());
    }
    assert_eq!(owner_ordinal, sources.len(), "published Source owner count");
    let mut after: Option<Vec<u8>> = None;
    let mut ordinal = 0;
    loop {
        let page = store.scan(source.paths, after.as_deref(), 128, &mut work)?;
        assert!(
            !page.entries.is_empty(),
            "published path map has an empty page"
        );
        for (key, marker) in &page.entries {
            assert!(ordinal < sources.len(), "published extra Source path");
            let expected_path = path(ordinal);
            let expected_key = SourcePathKey::copy(&expected_path, &work)?;
            assert_eq!(key.as_slice(), expected_key.as_slice());
            let SourcePathOwners::Repositories(bucket) = SourcePathOwners::decode(*marker, &work)?
            else {
                return Err("repository Source path has no repository bucket".into());
            };
            let repository_page = store.scan(MapRoot(Some(bucket)), None, 2, &mut work)?;
            assert_eq!(
                repository_page.entries.len(),
                1,
                "published Source path has an unexpected repository"
            );
            assert!(!repository_page.has_more);
            let (repository_path, repository_owner) = &repository_page.entries[0];
            assert_eq!(repository_path.as_slice(), repository_key.as_slice()?);
            let SourcePathOwners::LegacyOwner(owner) =
                SourcePathOwners::decode(*repository_owner, &work)?
            else {
                return Err("repository Source path has a nested bucket".into());
            };
            assert_eq!(owner, expected_owner(ordinal)?);
            let stored = store.get(sources[ordinal], &mut work)?;
            let record = SourceRecordOwner::decode_json(
                &stored.payload,
                &mut work,
                MemoryLifetime::Retained,
            )?;
            assert_eq!(record.path, expected_path);
            assert_eq!(record.source_repo_id.as_ref(), Some(&repository));
            let expected_body = body(ordinal);
            let expected_body_ref = quanta_index_contract::index::native_immutable_object_ref_v1(
                expected_body.as_bytes(),
                &[],
            )
            .ok_or("fixture body has no immutable ref")?;
            assert_eq!(record.bytes, expected_body_ref);
            assert_eq!(record.byte_len, u64::try_from(expected_body.len())?);
            if !NEEDLE_FILES.contains(&ordinal) {
                std::fs::remove_file(native_object_file(root, record.bytes)?)?;
            }
            ordinal += 1;
        }
        if !page.has_more {
            break;
        }
        after = page.entries.last().map(|(key, _)| key.clone());
    }
    assert_eq!(ordinal, sources.len(), "published Source path count");
    Ok(())
}

#[test]
#[ignore = "heavy: a sealed 4,097-file revision; run with `just rust-test-native-literal-anchor`"]
fn sdk_text_over_large_view_reads_only_literal_anchored_bodies() -> TestResult {
    assert_large_view_literal_anchor(AnchoredTextLayout::Tree)
}

#[test]
#[ignore = "heavy: a sealed 4,097-file LayeredV2 revision; run with `just rust-test-native-layered-literal-anchor`"]
fn sdk_text_over_large_layered_view_reads_only_literal_anchored_bodies() -> TestResult {
    assert_large_view_literal_anchor(AnchoredTextLayout::SegmentedLayeredV2)
}

fn assert_large_view_literal_anchor(layout: AnchoredTextLayout) -> TestResult {
    let root = private_root()?;
    let socket = root.path().join("native.sock");
    // Assemble the full query corpus without requiring a debug FULL to
    // finish in one server dispatch. The sealed revision exposes every file.
    let sources = publish_and_seal_batches(
        root.path(),
        &socket,
        &REVISION,
        &files(),
        FILES_PER_PUBLICATION,
        layout,
    )
    .map_err(|error| format!("prepare sealed 4,097-file source revision: {error}"))?;
    assert_eq!(sources.len(), FILES);
    let source = resolve_published_source_cut(root.path(), &socket, layout)?;
    verify_inventory_and_delete_non_matching_bodies(root.path(), &sources, source)?;

    let running =
        NativeServiceGuard::start(anchored_service_with_layout(root.path(), &socket, layout)?);
    if std::env::var_os("QUANTA_INDEX_NATIVE_WORK_DIAGNOSTICS_V1").as_deref()
        == Some(std::ffi::OsStr::new("1"))
    {
        let layout = match layout {
            AnchoredTextLayout::Tree => "tree_v1",
            AnchoredTextLayout::SegmentedLayeredV2 => "segmented_layered_v2",
        };
        eprintln!("native_4097_query_ready layout={layout}");
    }
    let primary = (|| -> TestResult {
        public_4097_candidate_acceptance(&socket, layout)?;
        text_profile_without_literal_is_refused(&NativeUdsClient::new(&socket), layout)
            .map_err(|error| format!("public query missing Literal sibling: {error}"))?;
        let client = QuantaIndex::connect(
            ConnectOptions::from_native_socket(
                socket.clone(),
                ViewSelection::Latest {
                    workspace: REVISION.source_scope(),
                    epoch: 1,
                },
            )
            .with_native_text_instance(layout.text_spec().instance),
        )
        .map_err(|error| format!("open protected needle-query SDK client: {error}"))?;
        let response = client
            .lexical()
            .query()
            .text("needle")
            .active(
                RepoId::new(REVISION.repo)?,
                RevisionId::new(REVISION.revision)?,
            )
            .top_k(10)
            .execute()
            .map_err(|error| format!("public SDK three-hit needle query: {error}"))?;
        let found: BTreeSet<&str> = response
            .results
            .iter()
            .map(|candidate| candidate.repo_relative_path.as_str())
            .collect();
        let expected: Vec<String> = NEEDLE_FILES.iter().map(|ordinal| path(*ordinal)).collect();
        assert_eq!(
            found,
            expected.iter().map(String::as_str).collect::<BTreeSet<_>>()
        );
        // A two-byte needle has no gram, so the census bound refuses it
        // explicitly before any body is read.
        let short = client
            .lexical()
            .query()
            .text("ne")
            .active(
                RepoId::new(REVISION.repo)?,
                RevisionId::new(REVISION.revision)?,
            )
            .top_k(10)
            .execute();
        assert!(
            short
                .as_ref()
                .is_err_and(|error| format!("{error:?}").contains("ResourceLimited")),
            "{short:?}"
        );
        Ok(())
    })();
    let stopped = running.stop();
    primary?;
    stopped
}

/// 09.2: `common` is in 4,096 files and `rare` in one of them.
const BOUND: ProtectedRevision<'static> = ProtectedRevision {
    repo: "repo-bound",
    revision: "rev-bound",
    slot: 47,
    owner_tag: 0x62,
};
const RARE: usize = 1_234;

fn bound_files() -> Vec<(String, Vec<u8>)> {
    (0..FILES)
        .map(|ordinal| {
            let body = if ordinal == RARE {
                "let rare = common;\n".to_owned()
            } else if ordinal < FILES - 1 {
                format!("let value{ordinal} = common;\n")
            } else {
                format!("let value{ordinal} = other;\n")
            };
            (format!("src/b{ordinal:04}.rs"), body.into_bytes())
        })
        .collect()
}

/// Delete every body but the `rare` one.
fn delete_all_but_rare(root: &std::path::Path, sources: &[ObjectRef]) -> TestResult {
    let store = FileObjectStore::open(root.join("native-objects"))?;
    let mut work = WorkBudget::with_request(work_limits(), budget());
    for (ordinal, source) in sources.iter().enumerate() {
        if ordinal == RARE {
            continue;
        }
        let stored = store.get(*source, &mut work)?;
        let record =
            SourceRecordOwner::decode_json(&stored.payload, &mut work, MemoryLifetime::Retained)?;
        std::fs::remove_file(native_object_file(root, record.bytes)?)?;
    }
    Ok(())
}

/// The 09.2 structural bound through the public SDK and CLI.
///
/// `rare common` is bounded by the 1-file `rare` anchor and answered with
/// exactly that file. `rare OR common` is bounded by 4,097 > 4,096 and
/// refused before any body read: every other body is deleted, so a read
/// would fail otherwise.
#[test]
#[ignore = "heavy: a 4,097-file FULL publication; run with `just rust-test-native-literal-anchor`"]
fn structural_bound_answers_rare_and_common_and_refuses_rare_or_common() -> TestResult {
    let root = private_root()?;
    let socket = root.path().join("native.sock");
    let sources = publish_and_seal(root.path(), &socket, &BOUND, &bound_files())?;
    assert_eq!(sources.len(), FILES);
    delete_all_but_rare(root.path(), &sources)?;

    let running = NativeServiceGuard::start(anchored_service(root.path(), &socket)?);
    let primary = (|| -> TestResult {
        let rare_path = format!("src/b{RARE:04}.rs");
        let selection = ViewSelection::Latest {
            workspace: BOUND.source_scope(),
            epoch: 1,
        };
        let instance = native_text_spec().instance;
        // SDK `.text()` (CodeSearch): both terms, each body OR path.
        let client = QuantaIndex::connect(
            ConnectOptions::from_native_socket(socket.clone(), selection)
                .with_native_text_instance(instance),
        )?;
        let response = client
            .lexical()
            .query()
            .text("rare common")
            .active(RepoId::new(BOUND.repo)?, RevisionId::new(BOUND.revision)?)
            .top_k(10)
            .execute()?;
        let found: Vec<&str> = response
            .results
            .iter()
            .map(|candidate| candidate.repo_relative_path.as_str())
            .collect();
        assert_eq!(found, [rare_path.as_str()]);
        // SDK Native text: the same conjunction, and the refused union.
        let uds = NativeUdsClient::new(&socket);
        let answered = uds.search_text(
            selection,
            instance,
            native_text_request("file.contains('rare') file.contains('common')", 10),
            &budget(),
        )?;
        assert_eq!(
            answered
                .hits
                .iter()
                .map(|hit| hit.path.as_str())
                .collect::<Vec<_>>(),
            [rare_path.as_str()]
        );
        assert_eq!(answered.matched, 1);
        let refused = uds.search_text(
            selection,
            instance,
            native_text_request("file.contains('rare') OR file.contains('common')", 10),
            &budget(),
        );
        assert!(
            matches!(&refused, Err(NativeUdsError::Refused(error))
                if error.code == NativeIndexErrorCode::ResourceLimited),
            "{refused:?}"
        );
        // CLI: `code-search` and `native search-text`.
        let socket_text = socket.to_str().ok_or("socket path is not UTF-8")?;
        let run = |args: &[&str]| -> (std::process::ExitCode, Vec<u8>, Vec<u8>) {
            let mut stdout = Vec::new();
            let mut stderr = Vec::new();
            let exit = quanta_index_searchctl::run(args.to_vec(), &mut stdout, &mut stderr);
            (exit, stdout, stderr)
        };
        let (exit, stdout, stderr) = run(&[
            "code-search",
            "--socket",
            socket_text,
            "--repo",
            BOUND.repo,
            "--revision",
            BOUND.revision,
            "--output",
            "json",
            "--query-text",
            "rare common",
            "--top-k",
            "10",
        ]);
        assert_eq!(
            exit,
            std::process::ExitCode::SUCCESS,
            "{}",
            String::from_utf8_lossy(&stderr)
        );
        let code_search: quanta_index_contract::TextQueryResponse =
            serde_json::from_slice(&stdout)?;
        assert_eq!(
            code_search
                .results
                .iter()
                .map(|candidate| candidate.repo_relative_path.as_str())
                .collect::<Vec<_>>(),
            [rare_path.as_str()]
        );
        let scope = BOUND.source_scope();
        let workspace_hex = hex([
            scope.tenant.0,
            scope.collection.0,
            scope.worktree.0,
            scope.slot.0,
        ]
        .into_iter()
        .flatten());
        let instance_hex = hex(instance.0);
        let native = |query: &str| {
            run(&[
                "native",
                "search-text",
                "--socket",
                socket_text,
                "--workspace",
                &workspace_hex,
                "--epoch",
                "1",
                "--instance",
                &instance_hex,
                "--query-text",
                query,
                "--top-k",
                "10",
                "--output",
                "json",
            ])
        };
        let (exit, stdout, stderr) = native("file.contains('rare') file.contains('common')");
        assert_eq!(
            exit,
            std::process::ExitCode::SUCCESS,
            "{}",
            String::from_utf8_lossy(&stderr)
        );
        let result: quanta_index_contract::native_protocol::NativeTextResult =
            serde_json::from_slice(&stdout)?;
        assert_eq!(
            result
                .hits
                .iter()
                .map(|hit| hit.path.as_str())
                .collect::<Vec<_>>(),
            [rare_path.as_str()]
        );
        let (exit, _stdout, stderr) = native("file.contains('rare') OR file.contains('common')");
        assert_ne!(exit, std::process::ExitCode::SUCCESS);
        let stderr = String::from_utf8(stderr)?;
        assert!(stderr.contains("ResourceLimited"), "{stderr}");
        Ok(())
    })();
    let stopped = running.stop();
    primary?;
    stopped
}
