use super::*;
use quanta_index_contract::{
    index::{Digest, ObjectRef, OwnerKey, StableOwnerId, ViewRef},
    native_protocol::{
        NativeInteractionPublisherId, NativeOverlayDocument, NativeOverlaySession,
        NativeStubBinding, NativeStubByteEdit, NativeStubCoordinateUnit, NativeStubEdit,
        NativeStubItem, NativeStubPosition, NativeStubRange, NativeStubReply, NativeStubRequest,
    },
};

const SOURCE: &str = "abcde\nfghi";

fn binding() -> NativeStubBinding {
    let operation = key(1);
    NativeStubBinding {
        document: NativeOverlayDocument {
            session: NativeOverlaySession {
                workspace: operation.workspace,
                publisher: NativeInteractionPublisherId([21; 32]),
                epoch: operation.epoch,
            },
            owner: OwnerKey {
                producer: operation.producer,
                stable_id: StableOwnerId([22; 16]),
            },
        },
        revision: 7,
        selected: ViewRef {
            workspace: operation.workspace,
            epoch: operation.epoch,
            sequence: 8,
            digest: Digest([23; 32]),
        },
        source: ObjectRef(Digest([24; 32])),
        language: "rust".into(),
    }
}

fn item(id: u8, start: u32, end: u32) -> NativeStubItem {
    NativeStubItem {
        stable_id: Digest([id; 32]),
        ast_digest: Digest([129; 32]),
        start,
        end,
        label: format!("item-{id}"),
    }
}

fn items() -> Vec<NativeStubItem> {
    vec![item(1, 0, 5), item(2, 6, 10)]
}

fn certify(reply: NativeStubReply) -> Result<NativeStubReply, NativeUdsError> {
    scripted_reply(NativeIndexResponse::Stub(reply), |client, budget| {
        client.stub(
            NativeStubRequest::Certify {
                binding: binding(),
                items: items(),
            },
            SOURCE,
            budget,
        )
    })
}

fn position(column: u32) -> NativeStubPosition {
    NativeStubPosition { line: 0, column }
}

fn edit(start: u32, end: u32, replacement: &str) -> NativeStubEdit {
    NativeStubEdit {
        target_id: Digest([1; 32]),
        expected_ast: Digest([129; 32]),
        range: NativeStubRange {
            start: position(start),
            end: position(end),
        },
        replacement: replacement.into(),
    }
}

fn prepared_edits() -> Vec<NativeStubByteEdit> {
    vec![
        NativeStubByteEdit {
            target_id: Digest([1; 32]),
            start: 0,
            end: 2,
            replacement: b"XY".to_vec(),
        },
        NativeStubByteEdit {
            target_id: Digest([1; 32]),
            start: 2,
            end: 5,
            replacement: b"ZZZ".to_vec(),
        },
    ]
}

fn preflight(reply: NativeStubReply) -> Result<NativeStubReply, NativeUdsError> {
    scripted_reply(NativeIndexResponse::Stub(reply), |client, budget| {
        client.stub(
            NativeStubRequest::Preflight {
                binding: binding(),
                items: items(),
                selected_id: Digest([1; 32]),
                edits: vec![edit(0, 2, "XY"), edit(2, 5, "ZZZ")],
                unit: NativeStubCoordinateUnit::Utf8Bytes,
            },
            SOURCE,
            budget,
        )
    })
}

fn is_protocol(result: &Result<NativeStubReply, NativeUdsError>) -> bool {
    matches!(result, Err(NativeUdsError::Protocol(_)))
}

#[test]
fn stub_sdk_preserves_exact_certification_and_prepared_bytes()
-> Result<(), Box<dyn std::error::Error>> {
    let certified = NativeStubReply::Certified {
        binding: binding(),
        items: items(),
        action_supported: true,
    };
    if certify(certified.clone())? != certified {
        return Err("Stub certification changed on the SDK wire".into());
    }

    let prepared = NativeStubReply::Prepared {
        binding: binding(),
        selected_id: Digest([1; 32]),
        edits: prepared_edits(),
    };
    if preflight(prepared.clone())? != prepared {
        return Err("Stub prepared bytes changed on the SDK wire".into());
    }

    // The cap is inclusive: all 128 distinct, strictly ordered IDs round-trip.
    let maximal: Vec<_> = (1..=128).map(|id| item(id, 0, 1)).collect();
    let maximal_reply = NativeStubReply::Certified {
        binding: binding(),
        items: maximal.clone(),
        action_supported: false,
    };
    let actual = scripted_reply(
        NativeIndexResponse::Stub(maximal_reply.clone()),
        |client, budget| {
            client.stub(
                NativeStubRequest::Certify {
                    binding: binding(),
                    items: maximal,
                },
                SOURCE,
                budget,
            )
        },
    )?;
    if actual != maximal_reply {
        return Err("maximal Stub certification changed on the SDK wire".into());
    }
    Ok(())
}

#[test]
fn admitted_stub_owner_retains_prepared_bytes_after_scope_handle_drop()
-> Result<(), Box<dyn std::error::Error>> {
    let request = NativeStubRequest::Preflight {
        binding: binding(),
        items: items(),
        selected_id: Digest([1; 32]),
        edits: vec![edit(0, 2, "XY"), edit(2, 5, "ZZZ")],
        unit: NativeStubCoordinateUnit::Utf8Bytes,
    };
    let expected = NativeStubReply::Prepared {
        binding: binding(),
        selected_id: Digest([1; 32]),
        edits: prepared_edits(),
    };
    let (memory, active, admissions) = publication_scope(false);
    let owner = scripted_reply(
        NativeIndexResponse::Stub(expected.clone()),
        |client, budget| client.stub_admitted(request.clone(), SOURCE, budget, &memory),
    )?;
    if owner.request() != &request || owner.reply() != &expected {
        return Err("admitted Stub request or fixed prepared bytes changed".into());
    }
    let charged = active.load(AtomicOrdering::SeqCst);
    if admissions.load(AtomicOrdering::SeqCst) == 0 || charged == 0 {
        return Err("admitted Stub owner lacks a live response charge".into());
    }
    drop(memory);
    if owner.reply() != &expected || active.load(AtomicOrdering::SeqCst) != charged {
        return Err("admitted Stub owner lost prepared bytes or charge with caller scope".into());
    }
    drop(owner);
    if active.load(AtomicOrdering::SeqCst) != 0 {
        return Err("admitted Stub response charge survived owner drop".into());
    }
    Ok(())
}

#[test]
fn stub_sdk_rejects_foreign_binding_changed_items_and_unordered_ids()
-> Result<(), Box<dyn std::error::Error>> {
    let original = binding();
    let mut foreign = vec![NativeStubBinding {
        revision: 8,
        ..original.clone()
    }];
    foreign.push(NativeStubBinding {
        source: ObjectRef(Digest([25; 32])),
        ..original.clone()
    });
    foreign.push(NativeStubBinding {
        selected: ViewRef {
            digest: Digest([26; 32]),
            ..original.selected
        },
        ..original.clone()
    });
    foreign.push(NativeStubBinding {
        document: NativeOverlayDocument {
            owner: OwnerKey {
                stable_id: StableOwnerId([27; 16]),
                ..original.document.owner
            },
            ..original.document
        },
        ..original.clone()
    });
    foreign.push(NativeStubBinding {
        document: NativeOverlayDocument {
            session: NativeOverlaySession {
                publisher: NativeInteractionPublisherId([28; 32]),
                ..original.document.session
            },
            ..original.document
        },
        ..original.clone()
    });
    for binding in foreign {
        if !is_protocol(&certify(NativeStubReply::Certified {
            binding,
            items: items(),
            action_supported: true,
        })) {
            return Err("Stub SDK accepted a foreign source binding".into());
        }
    }

    let mut changed = items();
    if let Some(first) = changed.first_mut() {
        first.label.push_str("-forged");
    }
    let changed = NativeStubReply::Certified {
        binding: original.clone(),
        items: changed,
        action_supported: true,
    };
    let reversed = NativeStubReply::Certified {
        binding: original.clone(),
        items: items().into_iter().rev().collect(),
        action_supported: true,
    };
    let duplicated = NativeStubReply::Certified {
        binding: original,
        items: vec![item(1, 0, 5), item(1, 0, 5)],
        action_supported: true,
    };
    for reply in [changed, reversed, duplicated] {
        if !is_protocol(&certify(reply)) {
            return Err("Stub SDK accepted changed or unordered certified items".into());
        }
    }
    Ok(())
}

#[test]
fn stub_sdk_rejects_forged_prepared_edit_binding_target_and_bytes()
-> Result<(), Box<dyn std::error::Error>> {
    let original = binding();
    let mut cases = vec![NativeStubReply::Prepared {
        binding: NativeStubBinding {
            revision: 8,
            ..original.clone()
        },
        selected_id: Digest([1; 32]),
        edits: prepared_edits(),
    }];
    cases.push(NativeStubReply::Prepared {
        binding: original.clone(),
        selected_id: Digest([2; 32]),
        edits: prepared_edits(),
    });

    let wrong_target = prepared_edits()
        .into_iter()
        .map(|mut edit| {
            if edit.start == 0 {
                edit.target_id = Digest([2; 32]);
            }
            edit
        })
        .collect();
    cases.push(NativeStubReply::Prepared {
        binding: original.clone(),
        selected_id: Digest([1; 32]),
        edits: wrong_target,
    });
    let wrong_range = prepared_edits()
        .into_iter()
        .map(|mut edit| {
            if edit.start == 2 {
                edit.start = 6;
                edit.end = 7;
            }
            edit
        })
        .collect();
    cases.push(NativeStubReply::Prepared {
        binding: original.clone(),
        selected_id: Digest([1; 32]),
        edits: wrong_range,
    });
    let shifted_inside_item = prepared_edits()
        .into_iter()
        .map(|mut edit| {
            if edit.start == 0 {
                edit.start = 1;
            }
            edit
        })
        .collect();
    cases.push(NativeStubReply::Prepared {
        binding: original.clone(),
        selected_id: Digest([1; 32]),
        edits: shifted_inside_item,
    });
    let wrong_replacement = prepared_edits()
        .into_iter()
        .map(|mut edit| {
            if edit.start == 0 {
                edit.replacement = b"forged".to_vec();
            }
            edit
        })
        .collect();
    cases.push(NativeStubReply::Prepared {
        binding: original.clone(),
        selected_id: Digest([1; 32]),
        edits: wrong_replacement,
    });
    let overlap = prepared_edits()
        .into_iter()
        .map(|mut edit| {
            if edit.start == 2 {
                edit.start = 1;
            }
            edit
        })
        .collect();
    cases.push(NativeStubReply::Prepared {
        binding: original.clone(),
        selected_id: Digest([1; 32]),
        edits: overlap,
    });
    let duplicate = vec![prepared_edits().remove(0), prepared_edits().remove(0)];
    cases.push(NativeStubReply::Prepared {
        binding: original,
        selected_id: Digest([1; 32]),
        edits: duplicate,
    });
    for reply in cases {
        if !is_protocol(&preflight(reply)) {
            return Err("Stub SDK accepted a forged prepared edit".into());
        }
    }
    Ok(())
}

#[test]
fn stub_sdk_checks_utf16_scalar_and_crlf_byte_ranges_against_source()
-> Result<(), Box<dyn std::error::Error>> {
    const UNICODE_SOURCE: &str = "a🦀b\r\nz";
    let source_item = item(1, 0, u32::try_from(UNICODE_SOURCE.len())?);
    let request = NativeStubRequest::Preflight {
        binding: binding(),
        items: vec![source_item],
        selected_id: Digest([1; 32]),
        edits: vec![NativeStubEdit {
            target_id: Digest([1; 32]),
            expected_ast: Digest([129; 32]),
            range: NativeStubRange {
                start: NativeStubPosition { line: 0, column: 1 },
                end: NativeStubPosition { line: 0, column: 3 },
            },
            replacement: "x".into(),
        }],
        unit: NativeStubCoordinateUnit::Utf16Units,
    };
    let reply = |start, end| NativeStubReply::Prepared {
        binding: binding(),
        selected_id: Digest([1; 32]),
        edits: vec![NativeStubByteEdit {
            target_id: Digest([1; 32]),
            start,
            end,
            replacement: b"x".to_vec(),
        }],
    };
    let valid = scripted_reply(NativeIndexResponse::Stub(reply(1, 5)), |client, budget| {
        client.stub(request.clone(), UNICODE_SOURCE, budget)
    })?;
    if valid != reply(1, 5) {
        return Err("Stub SDK changed valid UTF-16 byte coordinates".into());
    }
    for forged in [reply(2, 5), reply(1, 7)] {
        if !is_protocol(&scripted_reply(
            NativeIndexResponse::Stub(forged),
            |client, budget| client.stub(request.clone(), UNICODE_SOURCE, budget),
        )) {
            return Err("Stub SDK accepted a scalar or CRLF byte-range forgery".into());
        }
    }
    Ok(())
}

#[test]
fn stub_sdk_refuses_more_than_128_items_before_connecting() {
    let client = NativeUdsClient::new(std::path::PathBuf::from("/unreachable-stub-socket"));
    let budget = RequestBudgetV1::for_duration(Duration::from_secs(2));
    let mut items: Vec<_> = (1..=128).map(|id| item(id, 0, 1)).collect();
    items.push(item(129, 0, 1));
    let result = client.stub(
        NativeStubRequest::Certify {
            binding: binding(),
            items,
        },
        SOURCE,
        &budget,
    );
    assert!(is_protocol(&result));
}
