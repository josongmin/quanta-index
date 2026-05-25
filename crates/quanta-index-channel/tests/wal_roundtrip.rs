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

type TestRes = Result<(), Box<dyn std::error::Error>>;

fn repo() -> RepoId {
    RepoId::new("repo-1")
}

fn revision() -> RevisionId {
    RevisionId::new("rev-1")
}

fn gen_42() -> ManifestGeneration {
    ManifestGeneration::new(42)
}

fn require<T>(opt: Option<T>, what: &str) -> Result<T, Box<dyn std::error::Error>> {
    opt.ok_or_else(|| format!("expected {what}, got None").into())
}

fn require_eq<T: PartialEq + std::fmt::Debug>(actual: &T, expected: &T, what: &str) -> TestRes {
    if actual == expected {
        Ok(())
    } else {
        Err(format!("{what}: expected {expected:?}, got {actual:?}").into())
    }
}

#[test]
fn lexical_publish_then_subscribe_in_order() -> TestRes {
    let dir = tempfile::tempdir()?;
    let publisher = open_lexical_publisher(dir.path())?;

    let seq_bundle = publisher.publish(LexicalChannelOp::FullBundle(LexicalFullBundle {
        repo_id: repo(),
        revision_id: revision(),
        generation: gen_42(),
        payload: b"manifest-bytes".to_vec(),
    }))?;
    require_eq(&seq_bundle.get(), &1, "first publish seq")?;

    let seq_upsert = publisher.publish(LexicalChannelOp::UpsertChunk(UpsertChunk {
        repo_id: repo(),
        revision_id: revision(),
        generation: gen_42(),
        chunk_id: ChunkId::new("chunk-a"),
        payload: b"chunk-text".to_vec(),
    }))?;
    require_eq(&seq_upsert.get(), &2, "upsert seq")?;

    let seq_delete = publisher.publish(LexicalChannelOp::DeleteChunk(DeleteChunk {
        repo_id: repo(),
        revision_id: revision(),
        generation: gen_42(),
        chunk_id: ChunkId::new("chunk-b"),
    }))?;
    require_eq(&seq_delete.get(), &3, "delete seq")?;

    let seq_seal = publisher.seal(repo(), revision(), gen_42())?;
    require_eq(&seq_seal.get(), &4, "seal seq")?;

    drop(publisher);

    let mut subscriber = open_lexical_subscriber(dir.path())?;
    require_eq(
        &subscriber.cursor(),
        &ChannelSeq::ZERO,
        "fresh subscriber cursor",
    )?;

    let evt1 = require(subscriber.next_event()?, "evt1")?;
    require_eq(&evt1.seq.get(), &1, "evt1.seq")?;
    match evt1.op {
        LexicalChannelOp::FullBundle(b) => {
            require_eq(&b.payload, &b"manifest-bytes".to_vec(), "evt1 payload")?;
        }
        LexicalChannelOp::UpsertChunk(_)
        | LexicalChannelOp::DeleteChunk(_)
        | LexicalChannelOp::UpsertSymbol(_)
        | LexicalChannelOp::DeleteSymbol(_)
        | LexicalChannelOp::Seal(_)
        | LexicalChannelOp::UpsertCommit(_)
        | LexicalChannelOp::UpsertRef(_)
        | LexicalChannelOp::UpsertTag(_)
        | LexicalChannelOp::DeleteRef(_)
        | LexicalChannelOp::DeleteTag(_)
        | LexicalChannelOp::UpsertDirty(_)
        | LexicalChannelOp::EvictDirty(_)
        | LexicalChannelOp::UpsertParseTree(_)
        | LexicalChannelOp::DeleteParseTree(_)
        | LexicalChannelOp::UpsertDiffHunk(_) => {
            return Err(format!("expected FullBundle at evt1, got {:?}", evt1.seq).into());
        }
    }

    let evt2 = require(subscriber.next_event()?, "evt2")?;
    require_eq(&evt2.seq.get(), &2, "evt2.seq")?;
    match evt2.op {
        LexicalChannelOp::UpsertChunk(u) => {
            require_eq(
                &u.chunk_id.as_str().to_owned(),
                &"chunk-a".to_owned(),
                "evt2 chunk_id",
            )?;
            require_eq(&u.payload, &b"chunk-text".to_vec(), "evt2 payload")?;
        }
        LexicalChannelOp::FullBundle(_)
        | LexicalChannelOp::DeleteChunk(_)
        | LexicalChannelOp::UpsertSymbol(_)
        | LexicalChannelOp::DeleteSymbol(_)
        | LexicalChannelOp::Seal(_)
        | LexicalChannelOp::UpsertCommit(_)
        | LexicalChannelOp::UpsertRef(_)
        | LexicalChannelOp::UpsertTag(_)
        | LexicalChannelOp::DeleteRef(_)
        | LexicalChannelOp::DeleteTag(_)
        | LexicalChannelOp::UpsertDirty(_)
        | LexicalChannelOp::EvictDirty(_)
        | LexicalChannelOp::UpsertParseTree(_)
        | LexicalChannelOp::DeleteParseTree(_)
        | LexicalChannelOp::UpsertDiffHunk(_) => {
            return Err(format!("expected UpsertChunk at evt2, got {:?}", evt2.seq).into());
        }
    }

    let evt3 = require(subscriber.next_event()?, "evt3")?;
    require_eq(&evt3.seq.get(), &3, "evt3.seq")?;
    match evt3.op {
        LexicalChannelOp::DeleteChunk(d) => {
            require_eq(
                &d.chunk_id.as_str().to_owned(),
                &"chunk-b".to_owned(),
                "evt3 chunk_id",
            )?;
        }
        LexicalChannelOp::FullBundle(_)
        | LexicalChannelOp::UpsertChunk(_)
        | LexicalChannelOp::UpsertSymbol(_)
        | LexicalChannelOp::DeleteSymbol(_)
        | LexicalChannelOp::Seal(_)
        | LexicalChannelOp::UpsertCommit(_)
        | LexicalChannelOp::UpsertRef(_)
        | LexicalChannelOp::UpsertTag(_)
        | LexicalChannelOp::DeleteRef(_)
        | LexicalChannelOp::DeleteTag(_)
        | LexicalChannelOp::UpsertDirty(_)
        | LexicalChannelOp::EvictDirty(_)
        | LexicalChannelOp::UpsertParseTree(_)
        | LexicalChannelOp::DeleteParseTree(_)
        | LexicalChannelOp::UpsertDiffHunk(_) => {
            return Err(format!("expected DeleteChunk at evt3, got {:?}", evt3.seq).into());
        }
    }

    let evt4 = require(subscriber.next_event()?, "evt4")?;
    require_eq(&evt4.seq.get(), &4, "evt4.seq")?;
    if !matches!(evt4.op, LexicalChannelOp::Seal(_)) {
        return Err("expected Seal at evt4".into());
    }

    subscriber.ack(evt4.seq)?;
    require_eq(&subscriber.cursor().get(), &4, "cursor after ack")?;

    let next = subscriber.next_event()?;
    if next.is_some() {
        return Err("expected None after seal".into());
    }
    Ok(())
}

#[test]
fn subscriber_resumes_from_persisted_cursor() -> TestRes {
    let dir = tempfile::tempdir()?;
    let publisher = open_lexical_publisher(dir.path())?;
    for i in 0..5_u64 {
        let _seq = publisher.publish(LexicalChannelOp::UpsertChunk(UpsertChunk {
            repo_id: repo(),
            revision_id: revision(),
            generation: gen_42(),
            chunk_id: ChunkId::new(format!("c-{i}")),
            payload: vec![],
        }))?;
    }
    publisher.flush()?;

    {
        let mut sub = open_lexical_subscriber(dir.path())?;
        let evt_a = require(sub.next_event()?, "evt_a")?;
        let evt_b = require(sub.next_event()?, "evt_b")?;
        sub.ack(evt_b.seq)?;
        require_eq(&evt_a.seq.get(), &1, "evt_a.seq")?;
        require_eq(&evt_b.seq.get(), &2, "evt_b.seq")?;
    }

    let mut sub2 = open_lexical_subscriber(dir.path())?;
    require_eq(&sub2.cursor().get(), &2, "cursor persisted across reopen")?;
    let evt_c = require(sub2.next_event()?, "evt_c")?;
    require_eq(&evt_c.seq.get(), &3, "resumes from cursor+1")?;
    Ok(())
}

#[test]
fn semantic_track_isolated_from_lexical() -> TestRes {
    let dir = tempfile::tempdir()?;
    let lex = open_lexical_publisher(dir.path())?;
    let sem = open_semantic_publisher(dir.path())?;

    let _lex_seq = lex.publish(LexicalChannelOp::FullBundle(LexicalFullBundle {
        repo_id: repo(),
        revision_id: revision(),
        generation: gen_42(),
        payload: b"lex-bundle".to_vec(),
    }))?;
    let _sem_seq1 = sem.publish(SemanticChannelOp::FullBundle(SemanticFullBundle {
        repo_id: repo(),
        revision_id: revision(),
        generation: gen_42(),
        payload: b"sem-bundle".to_vec(),
    }))?;
    let _sem_seq2 = sem.publish(SemanticChannelOp::UpsertEmbedding(UpsertEmbedding {
        repo_id: repo(),
        revision_id: revision(),
        generation: gen_42(),
        embedding_id: EmbeddingId::new("e1"),
        payload: vec![1, 2, 3],
    }))?;

    drop(lex);
    drop(sem);

    let mut lex_sub = open_lexical_subscriber(dir.path())?;
    let lex_evt = require(lex_sub.next_event()?, "lex_evt")?;
    if !matches!(lex_evt.op, LexicalChannelOp::FullBundle(_)) {
        return Err("expected lex FullBundle".into());
    }
    if lex_sub.next_event()?.is_some() {
        return Err("lex channel saw a semantic op".into());
    }

    let mut sem_sub = open_semantic_subscriber(dir.path())?;
    let sem_evt1 = require(sem_sub.next_event()?, "sem_evt1")?;
    if !matches!(sem_evt1.op, SemanticChannelOp::FullBundle(_)) {
        return Err("expected sem FullBundle".into());
    }
    let sem_evt2 = require(sem_sub.next_event()?, "sem_evt2")?;
    if !matches!(sem_evt2.op, SemanticChannelOp::UpsertEmbedding(_)) {
        return Err("expected sem UpsertEmbedding".into());
    }
    Ok(())
}

#[test]
fn publisher_lock_prevents_double_publisher() -> TestRes {
    let dir = tempfile::tempdir()?;
    let _publisher = open_lexical_publisher(dir.path())?;
    let second = open_lexical_publisher(dir.path());
    if second.is_ok() {
        return Err("second publisher should fail to lock".into());
    }
    Ok(())
}
