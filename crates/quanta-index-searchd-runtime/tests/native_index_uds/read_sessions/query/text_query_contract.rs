//! Public text query contracts retained across the Native SDK and UDS boundary.

use super::*;

#[test]
fn exact_path_constraint_filters_before_top_k_and_treats_metacharacters_literally() -> TestResult {
    let root = private_root()?;
    let socket = root.path().join("native.sock");
    let running = NativeServiceGuard::start(read_session_service(
        root.path(),
        &socket,
        Duration::from_secs(30),
    )?);
    let primary = (|| -> TestResult {
        let client = NativeUdsClient::new(socket);
        let configured = create_text_workspace(&client)?;
        let (first, _) = publish_text_owner(
            &client,
            configured,
            2,
            1,
            "src/high.rs",
            "needle needle needle needle needle needle",
        )?;
        let literal_path = "src/a[1](test).rs";
        let (view, target_source) =
            publish_text_owner(&client, first, 3, 2, literal_path, "needle")?;
        let instance = native_text_spec().instance;
        let selection = ViewSelection::Exact(view);
        let ordinary = client.search_text(
            selection,
            instance,
            native_text_request("needle", 2),
            &budget(),
        )?;
        assert_eq!(ordinary.matched, 2);
        assert_eq!(ordinary.hits.len(), 2);
        assert_eq!(ordinary.hits[0].path, "src/high.rs");

        let mut scoped = native_text_request("needle", 1);
        scoped.constraints =
            quanta_index_contract::QueryConstraintSetV1::from_exact_repo_relative_path(
                quanta_index_contract::ExactRepoRelativePathV1::new(literal_path)?,
            );
        let result = client.search_text(selection, instance, scoped, &budget())?;
        assert!(result.exhaustive);
        assert_eq!(result.matched, 1);
        let [hit] = result.hits.as_slice() else {
            return Err("exact path must retain the lower-ranked owner before top-k".into());
        };
        assert_eq!(hit.path, literal_path);
        assert_eq!(hit.source, target_source);
        assert_eq!(hit.owner.stable_id, StableOwnerId([2; 16]));
        Ok(())
    })();
    let stopped = running.stop();
    primary?;
    stopped
}

#[test]
fn phrase_query_matches_only_adjacent_terms_through_native_uds() -> TestResult {
    let root = private_root()?;
    let socket = root.path().join("native.sock");
    let running = NativeServiceGuard::start(read_session_service(
        root.path(),
        &socket,
        Duration::from_secs(30),
    )?);
    let primary = (|| -> TestResult {
        let client = NativeUdsClient::new(socket);
        let mut view = create_text_workspace(&client)?;
        let mut sources = Vec::new();
        for (sequence, id, path, body) in [
            (2, 1, "src/adjacent.rs", "Lemon Yellow Banana"),
            (3, 2, "src/gapped.rs", "lemon bright yellow banana"),
            (4, 3, "src/reversed.rs", "banana yellow lemon"),
        ] {
            let (published, source) = publish_text_owner(&client, view, sequence, id, path, body)?;
            view = published;
            sources.push(source);
        }
        let instance = native_text_spec().instance;
        let selection = ViewSelection::Exact(view);
        for (query, expected) in [
            ("\"lemon yellow banana\"", Some(1_u8)),
            ("\"Lemon Yellow Banana\" case:yes", Some(1_u8)),
            ("\"lemon yellow banana\" case:yes", None),
            ("\"banana yellow lemon\"", Some(3_u8)),
            ("\"lemon banana\"", None),
        ] {
            let result = client.search_text(
                selection,
                instance,
                native_text_request(query, 3),
                &budget(),
            )?;
            assert_eq!(result.view, view, "{query}");
            assert!(result.exhaustive, "{query}");
            assert_eq!(result.matched, u64::from(expected.is_some()), "{query}");
            assert_eq!(
                result.hits.len(),
                usize::from(expected.is_some()),
                "{query}"
            );
            let actual = result
                .hits
                .first()
                .map(|hit| (hit.owner.stable_id, hit.source));
            assert_eq!(
                actual,
                expected.map(|id| (StableOwnerId([id; 16]), sources[usize::from(id - 1)])),
                "{query}"
            );
        }
        let query = native_text_request("\"Lemon Yellow Banana\" case:yes", 3);
        let ordinary = client.search_text(selection, instance, query.clone(), &budget())?;
        let preview = client.search_text_preview(selection, instance, query, &budget())?;
        assert_eq!(preview.ranking, ordinary);
        let [row] = preview.previews.as_slice() else {
            return Err("exact phrase must return one Source preview".into());
        };
        assert_eq!(row.owner.stable_id, StableOwnerId([1; 16]));
        assert_eq!(row.source, sources[0]);
        let quanta_index_contract::native_protocol::NativeTextPreviewEvidence::Excerpt(excerpt) =
            &row.evidence
        else {
            return Err("exact phrase must return a Source excerpt".into());
        };
        assert_eq!(excerpt.text, "Lemon Yellow Banana");
        Ok(())
    })();
    let stopped = running.stop();
    primary?;
    stopped
}

#[test]
fn file_contains_substrings_intersect_on_one_published_source_before_top_k_over_uds() -> TestResult
{
    let root = private_root()?;
    let socket = root.path().join("native.sock");
    let running = NativeServiceGuard::start(read_session_service(
        root.path(),
        &socket,
        Duration::from_secs(30),
    )?);
    let primary = (|| -> TestResult {
        let client = NativeUdsClient::new(socket);
        let mut view = create_text_workspace(&client)?;
        for (sequence, id, path, body) in [
            (2, 1, "src/both.rs", "foo_bar and café"),
            (3, 2, "src/first.rs", "foo_bar without the other word"),
            (4, 3, "src/second.rs", "café without the first word"),
        ] {
            (view, _) = publish_text_owner(&client, view, sequence, id, path, body)?;
        }
        let selection = ViewSelection::Exact(view);
        let instance = native_text_spec().instance;
        let only_substring = client.search_text(
            selection,
            instance,
            native_text_request("file.contains('oo_ba')", 1),
            &budget(),
        )?;
        assert!(only_substring.exhaustive);
        assert_eq!(only_substring.matched, 2);
        assert_eq!(only_substring.hits.len(), 1);

        let query = "file.contains('oo_ba') AND file.contains('café')";
        let joined = client.search_text(
            selection,
            instance,
            native_text_request(query, 1),
            &budget(),
        )?;
        assert!(joined.exhaustive);
        assert_eq!(joined.matched, 1);
        let [hit] = joined.hits.as_slice() else {
            return Err("file predicates must intersect on one source".into());
        };
        assert_eq!(hit.path, "src/both.rs");
        assert_eq!(hit.owner.stable_id, StableOwnerId([1; 16]));

        let mut wrong_path = native_text_request(query, 1);
        wrong_path.constraints =
            quanta_index_contract::QueryConstraintSetV1::from_exact_repo_relative_path(
                quanta_index_contract::ExactRepoRelativePathV1::new("src/first.rs")?,
            );
        let excluded = client.search_text(selection, instance, wrong_path, &budget())?;
        assert!(excluded.exhaustive);
        assert_eq!(excluded.matched, 0);
        assert!(excluded.hits.is_empty());
        Ok(())
    })();
    let stopped = running.stop();
    primary?;
    stopped
}
