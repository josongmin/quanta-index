"""Independent golden operation counts for the registered T16 resident recipe."""

import pytest

from tools.benchmark.retrieval.conditional_proof import default_window_operation_oracle


def row(identity, corpus="SymbolCard", components=1):
    return {
        "owner_id": identity,
        "owner_kind": "Symbol",
        "corpus_kind": corpus,
        "record_id": f"record-{identity}",
        "vector": [0.0] * components,
    }


def batch(scopes=(), tombstones=(), clears=()):
    return {
        "replace_scopes": list(scopes),
        "tombstone_scopes": list(tombstones),
        "clear_surfaces": list(clears),
    }


def scope(rows, memberships=()):
    return {"embeddings": rows, "cluster_memberships": list(memberships)}


def tombstone(identity, corpus="SymbolCard"):
    return {"semantic_scope": {"owner_id": identity, "owner_kind": "Symbol", "corpus_kind": corpus}}


def test_empty_plan_has_no_native_operations():
    assert default_window_operation_oracle(batch([scope([])])) == {
        "windows": 0,
        "replace_scopes": 0,
        "semantic_delete_commits": 0,
        "membership_delete_commits": 0,
        "membership_append_calls": 0,
    }


def test_first_seen_groups_are_indivisible_at_exact_owner_limit():
    rows = [row(str(index)) for index in range(1024)]
    # A repeated first owner is not the 1025th owner and cannot split its rows.
    rows.append(row("0"))
    assert default_window_operation_oracle(batch([scope(rows)])) == {
        "windows": 1,
        "replace_scopes": 1,
        "semantic_delete_commits": 1,
        "membership_delete_commits": 0,
        "membership_append_calls": 0,
    }
    rows.append(row("next", "ClusterCard"))
    assert default_window_operation_oracle(
        batch([scope(rows, [{"cluster_record_id": "record-next", "members": ["member"]}])])
    ) == {
        "windows": 2,
        "replace_scopes": 2,
        "semantic_delete_commits": 2,
        "membership_delete_commits": 1,
        "membership_append_calls": 1,
    }


def test_scope_fragments_and_membership_appends_follow_window_not_owner_count():
    item = batch(
        [
            scope(
                [row("a", "ClusterCard"), row("b", "ClusterCard")],
                [
                    {"cluster_record_id": "record-a", "members": ["member-a"]},
                    {"cluster_record_id": "record-b", "members": ["member-b"]},
                ],
            ),
            scope([row("c")]),
        ]
    )
    assert default_window_operation_oracle(item) == {
        "windows": 1,
        "replace_scopes": 2,
        "semantic_delete_commits": 1,
        "membership_delete_commits": 1,
        "membership_append_calls": 1,
    }


def test_tombstone_chunks_preserve_cluster_selection_and_clear_operations():
    tombstones = [tombstone(str(index)) for index in range(1024)]
    tombstones.extend([tombstone("cluster", "ClusterCard"), tombstone("plain")])
    assert default_window_operation_oracle(
        batch(tombstones=tombstones, clears=["File", "Chunk"])
    ) == {
        "windows": 0,
        "replace_scopes": 0,
        "semantic_delete_commits": 4,
        "membership_delete_commits": 3,
        "membership_append_calls": 0,
    }


def test_vector_byte_limit_uses_whole_owner_groups():
    # 1024 groups * 8192 f32 components reach exactly 32 MiB.
    rows = [row(str(index), components=8192) for index in range(1024)]
    result = default_window_operation_oracle(batch([scope(rows)]))
    assert (result["windows"], result["semantic_delete_commits"]) == (1, 1)
    # Same owner added after others still belongs to its first-seen group;
    # the first window now fits only 1023 groups, despite only 1024 owners.
    rows.append(row("0", components=1))
    result = default_window_operation_oracle(batch([scope(rows)]))
    assert (result["windows"], result["replace_scopes"], result["semantic_delete_commits"]) == (
        2,
        2,
        2,
    )


def test_unknown_clear_surface_is_not_assumed_to_execute():
    with pytest.raises(ValueError, match="clear surface"):
        default_window_operation_oracle(batch(clears=["FutureSurface"]))


def test_delete_byte_budget_is_not_implied_by_vector_or_owner_bounds():
    overhead = len("(corpus_kind = 'SymbolCard' AND owner_kind = 'Symbol' AND owner_id IN (''))")
    identity = "a" * (32 * 1024 * 1024 - overhead)
    item = {
        "owner_id": identity,
        "owner_kind": "Symbol",
        "corpus_kind": "SymbolCard",
        "record_id": "fixed",
        "vector": [1.0],
    }
    assert default_window_operation_oracle(batch([scope([item])]))["semantic_delete_commits"] == 1
    item["owner_id"] += "a"
    with pytest.raises(ValueError, match="delete predicate"):
        default_window_operation_oracle(batch([scope([item])]))
    # One tiny vector and one owner can still overflow after SQL escaping.
    item["owner_id"] = "'" * (16 * 1024 * 1024)
    with pytest.raises(ValueError, match="delete predicate"):
        default_window_operation_oracle(batch([scope([item])]))
    with pytest.raises(ValueError, match="delete predicate"):
        default_window_operation_oracle(
            batch(tombstones=[tombstone(item["owner_id"], "ClusterCard")])
        )
