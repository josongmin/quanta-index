//! End-to-end roundtrip: publisher writes a generation's worth of lexical ops,
//! subscriber reads them back in order, cursor persists across reopen.
//!
//! No external infrastructure; uses a `tempfile::TempDir` as `state_root`.

#![forbid(unsafe_code)]

use quanta_index_channel::{
    BundleChannelPublisher, BundleChannelSubscriber, open_lexical_publisher,
    open_lexical_subscriber, open_semantic_publisher, open_semantic_subscriber,
};
use quanta_index_contract::{
    ChannelSeq, ChunkId, DeleteChunk, EmbeddingId, LexicalChannelOp, LexicalFullBundle,
    ManifestGeneration, RepoId, RevisionId, SemanticChannelOp, SemanticFullBundle, UpsertChunk,
    UpsertEmbedding,
};

fn repo() -> RepoId {
    RepoId::new("repo-1")
}

fn revision() -> RevisionId {
    RevisionId::new("rev-1")
}

fn gen_42() -> ManifestGeneration {
    ManifestGeneration::new(42)
}

#[test]
fn lexical_publish_then_subscribe_in_order() {
    let dir = tempfile::tempdir().expect("tempdir");
    let publisher = open_lexical_publisher(dir.path()).expect("publisher");

    let seq_bundle = publisher
        .publish(LexicalChannelOp::FullBundle(LexicalFullBundle {
            repo_id: repo(),
            revision_id: revision(),
            generation: gen_42(),
            payload: b"manifest-bytes".to_vec(),
        }))
        .expect("publish bundle");
    assert_eq!(seq_bundle.get(), 1, "first publish should be seq 1");

    let seq_upsert = publisher
        .publish(LexicalChannelOp::UpsertChunk(UpsertChunk {
            repo_id: repo(),
            revision_id: revision(),
            generation: gen_42(),
            chunk_id: ChunkId::new("chunk-a"),
            payload: b"chunk-text".to_vec(),
        }))
        .expect("publish upsert");
    assert_eq!(seq_upsert.get(), 2);

    let seq_delete = publisher
        .publish(LexicalChannelOp::DeleteChunk(DeleteChunk {
            repo_id: repo(),
            revision_id: revision(),
            generation: gen_42(),
            chunk_id: ChunkId::new("chunk-b"),
        }))
        .expect("publish delete");
    assert_eq!(seq_delete.get(), 3);

    let seq_seal = publisher
        .seal(repo(), revision(), gen_42())
        .expect("publish seal");
    assert_eq!(seq_seal.get(), 4);

    drop(publisher);

    let mut subscriber = open_lexical_subscriber(dir.path()).expect("subscriber");
    assert_eq!(
        subscriber.cursor(),
        ChannelSeq::ZERO,
        "fresh subscriber starts at zero"
    );

    let evt1 = subscriber.next_event().expect("evt1").expect("evt1 some");
    assert_eq!(evt1.seq.get(), 1);
    match evt1.op {
        LexicalChannelOp::FullBundle(b) => assert_eq!(b.payload, b"manifest-bytes"),
        other => panic!("expected FullBundle, got {other:?}"),
    }

    let evt2 = subscriber.next_event().expect("evt2").expect("evt2 some");
    assert_eq!(evt2.seq.get(), 2);
    match evt2.op {
        LexicalChannelOp::UpsertChunk(u) => {
            assert_eq!(u.chunk_id.as_str(), "chunk-a");
            assert_eq!(u.payload, b"chunk-text");
        }
        other => panic!("expected UpsertChunk, got {other:?}"),
    }

    let evt3 = subscriber.next_event().expect("evt3").expect("evt3 some");
    assert_eq!(evt3.seq.get(), 3);
    match evt3.op {
        LexicalChannelOp::DeleteChunk(d) => assert_eq!(d.chunk_id.as_str(), "chunk-b"),
        other => panic!("expected DeleteChunk, got {other:?}"),
    }

    let evt4 = subscriber.next_event().expect("evt4").expect("evt4 some");
    assert_eq!(evt4.seq.get(), 4);
    assert!(matches!(evt4.op, LexicalChannelOp::Seal(_)));

    subscriber.ack(evt4.seq).expect("ack");
    assert_eq!(subscriber.cursor().get(), 4);

    let next = subscriber.next_event().expect("post-tail next");
    assert!(next.is_none(), "expected None after seal");
}

#[test]
fn subscriber_resumes_from_persisted_cursor() {
    let dir = tempfile::tempdir().expect("tempdir");
    let publisher = open_lexical_publisher(dir.path()).expect("publisher");
    for i in 0..5_u64 {
        let _ = publisher
            .publish(LexicalChannelOp::UpsertChunk(UpsertChunk {
                repo_id: repo(),
                revision_id: revision(),
                generation: gen_42(),
                chunk_id: ChunkId::new(format!("c-{i}")),
                payload: vec![],
            }))
            .expect("publish");
    }
    publisher.flush().expect("flush");

    {
        let mut sub = open_lexical_subscriber(dir.path()).expect("sub");
        let evt_a = sub.next_event().expect("evt_a").expect("some");
        let evt_b = sub.next_event().expect("evt_b").expect("some");
        sub.ack(evt_b.seq).expect("ack");
        assert_eq!(evt_a.seq.get(), 1);
        assert_eq!(evt_b.seq.get(), 2);
    }

    let mut sub2 = open_lexical_subscriber(dir.path()).expect("sub2");
    assert_eq!(sub2.cursor().get(), 2, "cursor persisted across reopen");
    let evt_c = sub2.next_event().expect("evt_c").expect("some");
    assert_eq!(evt_c.seq.get(), 3, "resumes from cursor+1");
}

#[test]
fn semantic_track_isolated_from_lexical() {
    let dir = tempfile::tempdir().expect("tempdir");
    let lex = open_lexical_publisher(dir.path()).expect("lex");
    let sem = open_semantic_publisher(dir.path()).expect("sem");

    let _ = lex
        .publish(LexicalChannelOp::FullBundle(LexicalFullBundle {
            repo_id: repo(),
            revision_id: revision(),
            generation: gen_42(),
            payload: b"lex-bundle".to_vec(),
        }))
        .expect("publish lex");
    let _ = sem
        .publish(SemanticChannelOp::FullBundle(SemanticFullBundle {
            repo_id: repo(),
            revision_id: revision(),
            generation: gen_42(),
            payload: b"sem-bundle".to_vec(),
        }))
        .expect("publish sem");
    let _ = sem
        .publish(SemanticChannelOp::UpsertEmbedding(UpsertEmbedding {
            repo_id: repo(),
            revision_id: revision(),
            generation: gen_42(),
            embedding_id: EmbeddingId::new("e1"),
            payload: vec![1, 2, 3],
        }))
        .expect("publish embed");

    drop(lex);
    drop(sem);

    let mut lex_sub = open_lexical_subscriber(dir.path()).expect("lex sub");
    let lex_evt = lex_sub.next_event().expect("lex evt").expect("some");
    assert!(matches!(lex_evt.op, LexicalChannelOp::FullBundle(_)));
    assert!(
        lex_sub.next_event().expect("after lex").is_none(),
        "lex channel must not see semantic ops"
    );

    let mut sem_sub = open_semantic_subscriber(dir.path()).expect("sem sub");
    let sem_evt1 = sem_sub.next_event().expect("sem1").expect("some");
    assert!(matches!(sem_evt1.op, SemanticChannelOp::FullBundle(_)));
    let sem_evt2 = sem_sub.next_event().expect("sem2").expect("some");
    assert!(matches!(sem_evt2.op, SemanticChannelOp::UpsertEmbedding(_)));
}

#[test]
fn publisher_lock_prevents_double_publisher() {
    let dir = tempfile::tempdir().expect("tempdir");
    let _publisher = open_lexical_publisher(dir.path()).expect("first");
    let second = open_lexical_publisher(dir.path());
    assert!(second.is_err(), "second publisher should fail to lock");
}
