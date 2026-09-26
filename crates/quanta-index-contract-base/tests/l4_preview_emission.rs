//! Wire-local excerpt invariants do not require reopening immutable source files.
#![expect(
    clippy::expect_used,
    reason = "fixed fixture construction and wire field assertions"
)]
use quanta_index_contract_base::{
    HighlightSpan, LexicalCandidate, ManifestGeneration, PreviewByteRange, PreviewKind,
    PreviewMetadata, PreviewUnavailableReason, RepoId, RepoRelativePath, RevisionId, SourceFileKey,
    SourceFileRevision,
};

fn candidate() -> LexicalCandidate {
    let source = SourceFileRevision {
        file: SourceFileKey {
            source_repo_id: RepoId::new("source").expect("repo"),
            repo_relative_path: RepoRelativePath::new("src/a.rs"),
        },
        revision_id: RevisionId::new("source-rev").expect("revision"),
        source_sha256: [3; 32],
    };
    LexicalCandidate {
        source_repo_id: source.file.source_repo_id.clone(),
        source: Some(source.clone()),
        preview: Some(PreviewMetadata {
            kind: PreviewKind::SourceChunk,
            source: Some(source),
            chunk_start_byte: Some(100),
            original_focus: Some(PreviewByteRange { start: 13, end: 19 }),
            original_context: Some(PreviewByteRange { start: 10, end: 19 }),
            normalized_focus: Some(PreviewByteRange { start: 13, end: 19 }),
            normalization_equivalent: false,
            unavailable_reason: None,
        }),
        candidate_id: "c".into(),
        repo_id: RepoId::new("container").expect("repo"),
        revision_id: RevisionId::new("snapshot").expect("revision"),
        manifest_generation: ManifestGeneration::new(1),
        repo_relative_path: RepoRelativePath::new("src/a.rs"),
        start_line: 1,
        end_line: 1,
        score: 1.0,
        snippet: "é needle".into(),
        snippet_hit_offset: Some(3),
        highlights: vec![HighlightSpan { start: 3, len: 6 }],
    }
}

#[test]
fn l4_wire_rejects_inconsistent_emitted_bytes() {
    let row = candidate();
    let valid = serde_json::to_value(&row).expect("valid source excerpt");
    assert_eq!(
        serde_json::from_value::<LexicalCandidate>(valid.clone()).expect("roundtrip"),
        row
    );
    for (pointer, forged) in [
        ("/preview/original_context/end", serde_json::json!(20)),
        ("/preview/original_focus/start", serde_json::json!(11)),
        ("/highlights/0/start", serde_json::json!(1)),
        ("/highlights/0/len", serde_json::json!(7)),
        ("/highlights/0/len", serde_json::json!(0)),
        ("/snippet_hit_offset", serde_json::json!(4)),
        ("/snippet", serde_json::json!("")),
    ] {
        let mut wire = valid.clone();
        *wire.pointer_mut(pointer).expect("fixture field") = forged;
        assert!(
            serde_json::from_value::<LexicalCandidate>(wire).is_err(),
            "accepted inconsistent emission at {pointer}"
        );
    }
}

#[test]
fn l4_wire_cannot_bypass_highlight_validation_with_absent_preview_metadata() {
    let mut row = candidate();
    row.preview = None;
    let valid = serde_json::to_value(&row).expect("legacy preview metadata is optional");
    assert_eq!(
        serde_json::from_value::<LexicalCandidate>(valid.clone()).expect("roundtrip"),
        row
    );
    for (pointer, forged) in [
        ("/highlights/0/start", serde_json::json!(1)),
        ("/highlights/0/len", serde_json::json!(7)),
        ("/snippet_hit_offset", serde_json::json!(4)),
    ] {
        let mut wire = valid.clone();
        *wire.pointer_mut(pointer).expect("fixture field") = forged;
        assert!(
            serde_json::from_value::<LexicalCandidate>(wire).is_err(),
            "accepted unbound highlight {pointer}"
        );
    }
}

#[test]
fn l4_wire_rejects_text_claimed_as_unavailable() {
    let mut row = candidate();
    row.preview = Some(PreviewMetadata::unavailable(
        PreviewKind::SourceChunk,
        PreviewUnavailableReason::WorkBudget,
        None,
    ));
    row.snippet.clear();
    row.snippet_hit_offset = None;
    row.highlights.clear();
    let mut wire = serde_json::to_value(&row).expect("unavailable");
    *wire.pointer_mut("/snippet").expect("snippet") = serde_json::json!("forged excerpt");
    assert!(serde_json::from_value::<LexicalCandidate>(wire).is_err());
    row.snippet = "forged excerpt".into();
    assert!(serde_json::to_value(&row).is_err());
}

#[test]
fn l4_wire_requires_primary_highlight_to_match_declared_focus() {
    let mut row = candidate();
    let valid = serde_json::to_value(&row).expect("source excerpt");
    for highlights in [
        serde_json::json!([{"start": 0, "len": 2}]),
        serde_json::json!([{"start": 0, "len": 2}, {"start": 3, "len": 6}]),
    ] {
        let mut wire = valid.clone();
        *wire.pointer_mut("/highlights").expect("highlights") = highlights;
        *wire.pointer_mut("/snippet_hit_offset").expect("offset") = serde_json::json!(0);
        assert!(serde_json::from_value::<LexicalCandidate>(wire).is_err());
    }
    row.highlights = vec![HighlightSpan { start: 0, len: 2 }];
    row.snippet_hit_offset = Some(0);
    assert!(serde_json::to_value(&row).is_err());
}

#[test]
fn l4_wire_preserves_overlaps_but_rejects_duplicate_or_reordered_highlights() {
    let mut row = candidate();
    row.highlights.insert(0, HighlightSpan { start: 0, len: 9 });
    row.snippet_hit_offset = Some(0);
    let preview = row.preview.as_mut().expect("preview");
    preview.original_focus = Some(PreviewByteRange { start: 10, end: 19 });
    preview.normalized_focus = Some(PreviewByteRange { start: 10, end: 19 });
    let valid = serde_json::to_value(&row).expect("overlapping highlights are distinct witnesses");
    assert_eq!(
        serde_json::from_value::<LexicalCandidate>(valid).expect("roundtrip"),
        row
    );
    row.highlights.reverse();
    assert!(serde_json::to_value(&row).is_err());
    row.highlights = vec![HighlightSpan { start: 3, len: 6 }; 2];
    row.snippet_hit_offset = Some(3);
    assert!(serde_json::to_value(&row).is_err());
}

#[test]
fn l4_wire_path_labels_cannot_claim_other_text_or_chunk_coordinates() {
    let mut row = candidate();
    let preview = row.preview.as_mut().expect("preview");
    preview.kind = PreviewKind::Path;
    preview.chunk_start_byte = None;
    preview.original_context = None;
    preview.original_focus = None;
    preview.normalized_focus = None;
    row.snippet = "src/a.rs".into();
    row.snippet_hit_offset = None;
    row.highlights.clear();
    let valid = serde_json::to_value(&row).expect("path preview");
    assert_eq!(
        serde_json::from_value::<LexicalCandidate>(valid.clone()).expect("roundtrip"),
        row
    );
    for (pointer, forged) in [
        ("/snippet", serde_json::json!("different.rs")),
        ("/preview/chunk_start_byte", serde_json::json!(100)),
        ("/highlights", serde_json::json!([{"start": 0, "len": 3}])),
    ] {
        let mut wire = valid.clone();
        *wire.pointer_mut(pointer).expect("fixture field") = forged;
        assert!(
            serde_json::from_value::<LexicalCandidate>(wire).is_err(),
            "accepted path mutation {pointer}"
        );
    }
}
