#![forbid(unsafe_code)]
#![allow(
    dead_code,
    reason = "shared scenario authority is consumed selectively by test binaries"
)]

use quanta_index_contract::TextQuerySyntax;

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub(super) struct TypedErrorExpectation {
    pub(super) code: &'static str,
    pub(super) message_contains: &'static str,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub(super) enum SdkFrontdoorSurface {
    Lexical,
    History,
    RuntimeMetadata,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub(super) enum SdkFrontdoorExpectation {
    CandidateIds(&'static [&'static str]),
    CommitShas(&'static [&'static str]),
    TypedError(TypedErrorExpectation),
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub(super) struct SdkFrontdoorScenario {
    pub(super) name: &'static str,
    pub(super) surface: SdkFrontdoorSurface,
    pub(super) syntax: TextQuerySyntax,
    pub(super) query_text: &'static str,
    pub(super) expected: SdkFrontdoorExpectation,
}

pub(super) const SDK_FRONTDOOR_SCENARIOS: &[SdkFrontdoorScenario] = &[
    SdkFrontdoorScenario {
        name: "native_file_contains_hit",
        surface: SdkFrontdoorSurface::Lexical,
        syntax: TextQuerySyntax::Native,
        query_text: "file.contains('oo_ba')",
        expected: SdkFrontdoorExpectation::CandidateIds(&["chunk-file-contains"]),
    },
    SdkFrontdoorScenario {
        name: "native_file_contains_phrase_miss",
        surface: SdkFrontdoorSurface::Lexical,
        syntax: TextQuerySyntax::Native,
        query_text: "file.contains(\"banana lemon\")",
        expected: SdkFrontdoorExpectation::CandidateIds(&[]),
    },
    SdkFrontdoorScenario {
        name: "native_since_time_success",
        surface: SdkFrontdoorSurface::History,
        syntax: TextQuerySyntax::Native,
        query_text: "type:commit since.time:1970-01-01T00:00:00.012Z fix",
        expected: SdkFrontdoorExpectation::CommitShas(&[
            "0123456789abcdef0123456789abcdef01234567",
        ]),
    },
    SdkFrontdoorScenario {
        name: "native_since_commit_success",
        surface: SdkFrontdoorSurface::History,
        syntax: TextQuerySyntax::Native,
        query_text: "type:commit since.commit:refs/heads/main fix",
        expected: SdkFrontdoorExpectation::CommitShas(&[
            "0123456789abcdef0123456789abcdef01234567",
        ]),
    },
    SdkFrontdoorScenario {
        name: "native_since_commit_unknown_ref_typed_fail",
        surface: SdkFrontdoorSurface::History,
        syntax: TextQuerySyntax::Native,
        query_text: "type:commit since.commit:refs/heads/missing fix",
        expected: SdkFrontdoorExpectation::TypedError(TypedErrorExpectation {
            code: "HISTORY_INVALID_TIMEREF",
            message_contains: "since.commit",
        }),
    },
    SdkFrontdoorScenario {
        name: "native_history_missing_type_invalid_request",
        surface: SdkFrontdoorSurface::History,
        syntax: TextQuerySyntax::Native,
        query_text: "fix",
        expected: SdkFrontdoorExpectation::TypedError(TypedErrorExpectation {
            code: "INVALID_REQUEST",
            message_contains: "explicit `type:commit` or `type:diff` is required",
        }),
    },
    SdkFrontdoorScenario {
        name: "native_history_commit_file_invalid_request",
        surface: SdkFrontdoorSurface::History,
        syntax: TextQuerySyntax::Native,
        query_text: "type:commit file:src/history.rs fix",
        expected: SdkFrontdoorExpectation::TypedError(TypedErrorExpectation {
            code: "INVALID_REQUEST",
            message_contains: "`file:` and `diff.*` filters require `type:diff`",
        }),
    },
    SdkFrontdoorScenario {
        name: "native_history_predicate_leaf_not_implemented",
        surface: SdkFrontdoorSurface::History,
        syntax: TextQuerySyntax::Native,
        query_text: "type:commit file.contains('fix')",
        expected: SdkFrontdoorExpectation::TypedError(TypedErrorExpectation {
            code: "NOT_IMPLEMENTED",
            message_contains: "history: predicate leaves are not executable",
        }),
    },
    SdkFrontdoorScenario {
        name: "runtime_dirty_no_clean_complement",
        surface: SdkFrontdoorSurface::RuntimeMetadata,
        syntax: TextQuerySyntax::Sourcegraph,
        query_text: "dirty:no quartz",
        expected: SdkFrontdoorExpectation::CandidateIds(&["alpha"]),
    },
    SdkFrontdoorScenario {
        name: "runtime_affected_positive",
        surface: SdkFrontdoorSurface::RuntimeMetadata,
        syntax: TextQuerySyntax::Sourcegraph,
        query_text: "affected:rebuild=lexical quartz",
        expected: SdkFrontdoorExpectation::CandidateIds(&["alpha"]),
    },
    SdkFrontdoorScenario {
        name: "runtime_invalidated_by_positive",
        surface: SdkFrontdoorSurface::RuntimeMetadata,
        syntax: TextQuerySyntax::Sourcegraph,
        query_text: "invalidated_by:rebuild=lexical quartz",
        expected: SdkFrontdoorExpectation::CandidateIds(&["alpha"]),
    },
];

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub(super) enum IpcFrontdoorSurface {
    History,
    RuntimeMetadata,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub(super) enum IpcFrontdoorExpectation {
    CandidateIds(&'static [&'static str]),
    CommitShas(&'static [&'static str]),
    DiffPaths(&'static [&'static str]),
    TypedError(TypedErrorExpectation),
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub(super) struct IpcFrontdoorScenario {
    pub(super) name: &'static str,
    pub(super) surface: IpcFrontdoorSurface,
    pub(super) syntax: TextQuerySyntax,
    pub(super) query_text: &'static str,
    pub(super) expected: IpcFrontdoorExpectation,
}

pub(super) const IPC_FRONTDOOR_SCENARIOS: &[IpcFrontdoorScenario] = &[
    IpcFrontdoorScenario {
        name: "history_after_positive",
        surface: IpcFrontdoorSurface::History,
        syntax: TextQuerySyntax::Sourcegraph,
        query_text: "type:commit after:1970-01-01T00:00:00.011Z alpha_content_needle",
        expected: IpcFrontdoorExpectation::CommitShas(&[
            "0123456789abcdef0123456789abcdef01234567",
        ]),
    },
    IpcFrontdoorScenario {
        name: "history_until_positive",
        surface: IpcFrontdoorSurface::History,
        syntax: TextQuerySyntax::Sourcegraph,
        query_text: "type:commit until:1970-01-01T00:00:00.012Z alpha_content_needle",
        expected: IpcFrontdoorExpectation::CommitShas(&[
            "0123456789abcdef0123456789abcdef01234567",
        ]),
    },
    IpcFrontdoorScenario {
        name: "history_diff_added_positive",
        surface: IpcFrontdoorSurface::History,
        syntax: TextQuerySyntax::Sourcegraph,
        query_text: "type:diff diff.added:history",
        expected: IpcFrontdoorExpectation::DiffPaths(&["src/history.rs"]),
    },
    IpcFrontdoorScenario {
        name: "history_diff_removed_positive",
        surface: IpcFrontdoorSurface::History,
        syntax: TextQuerySyntax::Sourcegraph,
        query_text: "type:diff diff.removed:history",
        expected: IpcFrontdoorExpectation::DiffPaths(&["src/history.rs"]),
    },
    IpcFrontdoorScenario {
        name: "history_diff_touched_positive",
        surface: IpcFrontdoorSurface::History,
        syntax: TextQuerySyntax::Sourcegraph,
        query_text: "type:diff diff.touched:history",
        expected: IpcFrontdoorExpectation::DiffPaths(&["src/history.rs"]),
    },
    IpcFrontdoorScenario {
        name: "history_missing_type_invalid_request",
        surface: IpcFrontdoorSurface::History,
        syntax: TextQuerySyntax::Native,
        query_text: "fix",
        expected: IpcFrontdoorExpectation::TypedError(TypedErrorExpectation {
            code: "INVALID_REQUEST",
            message_contains: "explicit `type:commit` or `type:diff` is required",
        }),
    },
    IpcFrontdoorScenario {
        name: "history_commit_file_invalid_request",
        surface: IpcFrontdoorSurface::History,
        syntax: TextQuerySyntax::Native,
        query_text: "type:commit file:src/history.rs fix",
        expected: IpcFrontdoorExpectation::TypedError(TypedErrorExpectation {
            code: "INVALID_REQUEST",
            message_contains: "`file:` and `diff.*` filters require `type:diff`",
        }),
    },
    IpcFrontdoorScenario {
        name: "runtime_snapshot_active_positive",
        surface: IpcFrontdoorSurface::RuntimeMetadata,
        syntax: TextQuerySyntax::Sourcegraph,
        query_text: "snapshot:active catalog_snapshot_needle",
        expected: IpcFrontdoorExpectation::CandidateIds(&["snap"]),
    },
    IpcFrontdoorScenario {
        name: "runtime_snapshot_missing_typed_fail",
        surface: IpcFrontdoorSurface::RuntimeMetadata,
        syntax: TextQuerySyntax::Sourcegraph,
        query_text: "snapshot:missing catalog_snapshot_needle",
        expected: IpcFrontdoorExpectation::TypedError(TypedErrorExpectation {
            code: "SNAPSHOT_UNKNOWN",
            message_contains: "snapshot `missing`",
        }),
    },
    IpcFrontdoorScenario {
        name: "runtime_predicate_leaf_not_implemented",
        surface: IpcFrontdoorSurface::RuntimeMetadata,
        syntax: TextQuerySyntax::Native,
        query_text: "changed:since=1970-01-01T00:00:00.010Z file.contains('catalog_changed_needle')",
        expected: IpcFrontdoorExpectation::TypedError(TypedErrorExpectation {
            code: "NOT_IMPLEMENTED",
            message_contains: "runtime metadata: predicate leaves are not executable",
        }),
    },
    IpcFrontdoorScenario {
        name: "runtime_meta_owner_positive",
        surface: IpcFrontdoorSurface::RuntimeMetadata,
        syntax: TextQuerySyntax::Sourcegraph,
        query_text: "meta.owner:team-a catalog_owner_needle",
        expected: IpcFrontdoorExpectation::CandidateIds(&["owner"]),
    },
    IpcFrontdoorScenario {
        name: "runtime_meta_service_positive",
        surface: IpcFrontdoorSurface::RuntimeMetadata,
        syntax: TextQuerySyntax::Sourcegraph,
        query_text: "meta.service:search catalog_service_needle",
        expected: IpcFrontdoorExpectation::CandidateIds(&["service"]),
    },
    IpcFrontdoorScenario {
        name: "runtime_meta_layer_positive",
        surface: IpcFrontdoorSurface::RuntimeMetadata,
        syntax: TextQuerySyntax::Sourcegraph,
        query_text: "meta.layer:index catalog_layer_needle",
        expected: IpcFrontdoorExpectation::CandidateIds(&["layer"]),
    },
    IpcFrontdoorScenario {
        name: "runtime_meta_surface_positive",
        surface: IpcFrontdoorSurface::RuntimeMetadata,
        syntax: TextQuerySyntax::Sourcegraph,
        query_text: "meta.surface:lexical catalog_surface_needle",
        expected: IpcFrontdoorExpectation::CandidateIds(&["surface"]),
    },
];

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub(super) struct DslFrontdoorScenario {
    pub(super) name: &'static str,
    pub(super) syntax: TextQuerySyntax,
    pub(super) query_text: &'static str,
    pub(super) expected_candidate_ids: &'static [&'static str],
}

pub(super) const DSL_FRONTDOOR_SCENARIOS: &[DslFrontdoorScenario] = &[
    DslFrontdoorScenario {
        name: "sourcegraph_repo_has_file_true_gate",
        syntax: TextQuerySyntax::Sourcegraph,
        query_text: "repo:has.file(path:src/lib.rs) needle",
        expected_candidate_ids: &["alpha", "beta"],
    },
    DslFrontdoorScenario {
        name: "sourcegraph_repo_has_file_miss",
        syntax: TextQuerySyntax::Sourcegraph,
        query_text: "repo:has.file(path:missing.rs) needle",
        expected_candidate_ids: &[],
    },
];
