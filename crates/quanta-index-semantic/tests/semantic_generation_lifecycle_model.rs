//! Deterministic lifecycle-model proof for the persisted semantic adapter.
//!
//! The adapter owns durable generation build/seal/open/scan. It deliberately
//! does not own the process-wide active-generation CAS; that belongs to the
//! search-plane readiness catalog. This rail models activation as selecting a
//! sealed adapter generation and rollback as reselecting an older sealed
//! generation. Every selection is checked against a small reference model and
//! the real on-disk LanceDB state.

#![forbid(unsafe_code)]

use std::collections::BTreeMap;
use std::path::Path;

use quanta_index_contract::{
    BatchIngestMode, ManifestGeneration, OwnerDocKind, RepoId, RevisionId, SemanticCorpusKindV1,
    SemanticReplaceScope,
};
use quanta_index_core::{CoreError, SemanticBatchBuildPort, SemanticIndexOpenPort};
use quanta_index_semantic::{
    SemanticAdapter, embedding_record_v1, ingest_batch_v1, model_contract_v1,
    scan_persisted_generations, search_scope_v1, tombstone_scope_with_semantic_owner_v1,
};

type TestResult = Result<(), Box<dyn std::error::Error>>;

const DIMENSION: u32 = 3;

fn repo_id() -> RepoId {
    RepoId::new("repo-semantic-lifecycle-model")
}

fn revision_id() -> RevisionId {
    RevisionId::new("rev-semantic-lifecycle-model")
}

#[derive(Clone, Debug)]
struct RecordSpec {
    id: &'static str,
    owner: &'static str,
    path: &'static str,
    vector: [f32; 3],
}

#[derive(Clone, Debug)]
struct TombstoneSpec {
    owner: &'static str,
    path: &'static str,
}

impl RecordSpec {
    fn embedding(&self) -> Result<quanta_index_contract::EmbeddingRecord, String> {
        embedding_record_v1(
            self.id,
            self.path,
            OwnerDocKind::Symbol,
            self.owner,
            SemanticCorpusKindV1::SymbolCard,
            self.vector.to_vec(),
        )
    }
}

#[derive(Debug)]
enum LifecycleCommand {
    Build {
        generation: u64,
        base_generation: Option<u64>,
        batch_digest: &'static str,
        replacements: Vec<RecordSpec>,
        tombstones: Vec<TombstoneSpec>,
        seal: bool,
    },
    AssertUnsealedCannotActivate {
        generation: u64,
    },
    Activate {
        generation: u64,
    },
    Rollback {
        target: u64,
    },
    RestartAndRecover,
}

#[derive(Clone, Debug, Default)]
struct ModelGeneration {
    records_by_owner: BTreeMap<String, String>,
    sealed: bool,
}

#[derive(Debug, Default)]
struct LifecycleModel {
    generations: BTreeMap<u64, ModelGeneration>,
    active_generation: Option<u64>,
}

impl LifecycleModel {
    fn generation(generation: u64) -> ManifestGeneration {
        ManifestGeneration::new(generation)
    }

    fn expected_ids(&self, generation: u64) -> Result<Vec<String>, String> {
        let state = self
            .generations
            .get(&generation)
            .ok_or_else(|| format!("model has no generation {generation}"))?;
        if !state.sealed {
            return Err(format!("model generation {generation} is not sealed"));
        }
        Ok(state.records_by_owner.values().cloned().collect())
    }

    fn apply_build(
        &mut self,
        generation: u64,
        base_generation: Option<u64>,
        replacements: &[RecordSpec],
        tombstones: &[TombstoneSpec],
        seal: bool,
    ) -> Result<(), String> {
        if !self.generations.contains_key(&generation) {
            let inherited = match base_generation {
                Some(base) => self
                    .generations
                    .get(&base)
                    .filter(|state| state.sealed)
                    .cloned()
                    .ok_or_else(|| {
                        format!("delta generation {generation} has no sealed model base {base}")
                    })?,
                None => ModelGeneration::default(),
            };
            let _prior = self.generations.insert(
                generation,
                ModelGeneration {
                    records_by_owner: inherited.records_by_owner,
                    sealed: false,
                },
            );
        }

        let state = self
            .generations
            .get_mut(&generation)
            .ok_or_else(|| format!("model failed to create generation {generation}"))?;
        if state.sealed {
            return Err(format!(
                "model refuses mutation of sealed generation {generation}"
            ));
        }
        for tombstone in tombstones {
            let _removed = state.records_by_owner.remove(tombstone.owner);
        }
        for record in replacements {
            let _prior = state
                .records_by_owner
                .insert(record.owner.to_string(), record.id.to_string());
        }
        if seal {
            state.sealed = true;
        }
        Ok(())
    }

    fn assert_open_matches(&self, adapter: &SemanticAdapter, generation: u64) -> TestResult {
        let searcher = adapter.open(&repo_id(), &revision_id(), Self::generation(generation))?;
        let mut actual = searcher
            .search(&[1.0, 1.0, 1.0], 32)?
            .into_iter()
            .map(|hit| hit.candidate_id)
            .collect::<Vec<_>>();
        actual.sort();
        let expected = self.expected_ids(generation)?;
        assert_eq!(
            actual, expected,
            "generation {generation} diverged from the lifecycle reference model"
        );
        Ok(())
    }

    fn execute(
        &mut self,
        root: &Path,
        adapter: &mut SemanticAdapter,
        command: LifecycleCommand,
    ) -> TestResult {
        match command {
            LifecycleCommand::Build {
                generation,
                base_generation,
                batch_digest,
                replacements,
                tombstones,
                seal,
            } => {
                let replace_scopes = replacements
                    .iter()
                    .map(|record| {
                        Ok(SemanticReplaceScope {
                            scope: search_scope_v1(record.path),
                            scope_digest: format!("scope:{}:{}", record.path, record.owner),
                            embeddings: vec![record.embedding()?],
                        })
                    })
                    .collect::<Result<Vec<_>, String>>()?;
                let tombstone_scopes = tombstones
                    .iter()
                    .map(|tombstone| {
                        tombstone_scope_with_semantic_owner_v1(
                            tombstone.path,
                            SemanticCorpusKindV1::SymbolCard,
                            OwnerDocKind::Symbol,
                            tombstone.owner,
                        )
                    })
                    .collect();
                let batch = ingest_batch_v1(
                    repo_id(),
                    revision_id(),
                    Self::generation(generation),
                    base_generation.map(Self::generation),
                    format!("manifest:lifecycle:{generation}"),
                    batch_digest.to_string(),
                    if base_generation.is_some() {
                        BatchIngestMode::Delta
                    } else {
                        BatchIngestMode::ReplaceGeneration
                    },
                    model_contract_v1(DIMENSION),
                    replace_scopes,
                    tombstone_scopes,
                    seal,
                );
                adapter.build_batch(&batch)?;
                self.apply_build(
                    generation,
                    base_generation,
                    &replacements,
                    &tombstones,
                    seal,
                )?;
            }
            LifecycleCommand::AssertUnsealedCannotActivate { generation } => {
                let state = self
                    .generations
                    .get(&generation)
                    .ok_or_else(|| format!("model has no generation {generation}"))?;
                assert!(
                    !state.sealed,
                    "unsealed activation assertion requires an unsealed model generation"
                );
                let error =
                    match adapter.open(&repo_id(), &revision_id(), Self::generation(generation)) {
                        Ok(_) => {
                            return Err("unsealed generation must not become selectable".into());
                        }
                        Err(error) => error,
                    };
                assert!(
                    matches!(error, CoreError::NotReady(ref message) if message.contains("not sealed")),
                    "unsealed activation must fail closed with NotReady, got {error:?}"
                );
            }
            LifecycleCommand::Activate { generation } => {
                self.assert_open_matches(adapter, generation)?;
                self.active_generation = Some(generation);
            }
            LifecycleCommand::Rollback { target } => {
                let previous = self
                    .active_generation
                    .ok_or("rollback requires an active model generation")?;
                assert_ne!(
                    previous, target,
                    "rollback target must differ from active generation"
                );
                self.assert_open_matches(adapter, target)?;
                self.active_generation = Some(target);
            }
            LifecycleCommand::RestartAndRecover => {
                *adapter = SemanticAdapter::with_state_root(root.to_path_buf())?;
                let mut persisted = scan_persisted_generations(root)?
                    .into_iter()
                    .map(|record| record.generation.get())
                    .collect::<Vec<_>>();
                persisted.sort();
                let expected = self
                    .generations
                    .iter()
                    .filter_map(|(generation, state)| state.sealed.then_some(*generation))
                    .collect::<Vec<_>>();
                assert_eq!(
                    persisted, expected,
                    "restart recovery must expose exactly the sealed generation set"
                );
                if let Some(active) = self.active_generation {
                    self.assert_open_matches(adapter, active)?;
                }
            }
        }
        Ok(())
    }
}

#[test]
#[expect(
    clippy::panic_in_result_fn,
    reason = "the lifecycle model has explicit invariant assertions"
)]
fn deterministic_generation_lifecycle_matches_reference_model() -> TestResult {
    let temp = tempfile::tempdir()?;
    let root = temp.path().to_path_buf();
    let mut adapter = SemanticAdapter::with_state_root(root.clone())?;
    let mut model = LifecycleModel::default();

    let trace = vec![
        LifecycleCommand::Build {
            generation: 701,
            base_generation: None,
            batch_digest: "lifecycle:g701:build",
            replacements: vec![
                RecordSpec {
                    id: "alpha-v1",
                    owner: "symbol:Session::alpha",
                    path: "src/session.rs",
                    vector: [1.0, 0.0, 0.0],
                },
                RecordSpec {
                    id: "beta-v1",
                    owner: "symbol:Session::beta",
                    path: "src/session.rs",
                    vector: [0.0, 1.0, 0.0],
                },
            ],
            tombstones: Vec::new(),
            seal: false,
        },
        LifecycleCommand::AssertUnsealedCannotActivate { generation: 701 },
        LifecycleCommand::Build {
            generation: 701,
            base_generation: None,
            batch_digest: "lifecycle:g701:append-replace",
            replacements: vec![
                RecordSpec {
                    id: "alpha-v2",
                    owner: "symbol:Session::alpha",
                    path: "src/session.rs",
                    vector: [0.9, 0.1, 0.0],
                },
                RecordSpec {
                    id: "gamma-v1",
                    owner: "symbol:Session::gamma",
                    path: "src/other.rs",
                    vector: [0.0, 0.0, 1.0],
                },
            ],
            tombstones: Vec::new(),
            seal: false,
        },
        LifecycleCommand::Build {
            generation: 701,
            base_generation: None,
            batch_digest: "lifecycle:g701:seal",
            replacements: Vec::new(),
            tombstones: Vec::new(),
            seal: true,
        },
        LifecycleCommand::Activate { generation: 701 },
        LifecycleCommand::Build {
            generation: 702,
            base_generation: Some(701),
            batch_digest: "lifecycle:g702:delta-replace-tombstone-seal",
            replacements: vec![RecordSpec {
                id: "beta-v2",
                owner: "symbol:Session::beta",
                path: "src/session.rs",
                vector: [0.1, 0.9, 0.0],
            }],
            tombstones: vec![TombstoneSpec {
                owner: "symbol:Session::gamma",
                path: "src/other.rs",
            }],
            seal: true,
        },
        LifecycleCommand::Activate { generation: 702 },
        LifecycleCommand::Rollback { target: 701 },
        LifecycleCommand::Build {
            generation: 703,
            base_generation: Some(701),
            batch_digest: "lifecycle:g703:unsealed",
            replacements: vec![RecordSpec {
                id: "delta-v1",
                owner: "symbol:Session::delta",
                path: "src/unsealed.rs",
                vector: [0.0, 0.5, 0.5],
            }],
            tombstones: Vec::new(),
            seal: false,
        },
        LifecycleCommand::RestartAndRecover,
    ];

    for command in trace {
        model.execute(&root, &mut adapter, command)?;
    }
    assert_eq!(model.active_generation, Some(701));
    Ok(())
}
