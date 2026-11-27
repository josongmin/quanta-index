//! The merge-join literal anchor against the per-owner probing oracle.
//!
//! Every corpus here is a fixed set of bodies with a packed Literal member
//! over their raw bytes. The merge-join anchor must return exactly the
//! oracle's candidate set, or `None` exactly when the oracle does.

use super::super::super::literal::LiteralGrams;
use super::super::super::literal_blocks::{LiteralBlockBuilder, PushResult, literal_key};
use super::super::super::literal_packed::PackedRoot;
use super::super::super::text_literal_anchor::{
    CharVariants, ExactScalars, SimpleCaseFold, anchor_chars, anchor_chars_probing, anchor_needle,
    case_inverse,
};
use super::*;
use quanta_index_core::index::ports::{ImmutableObjectPort, ObjectWriteSink};
use quanta_index_core::index::{
    IndexInstanceId, InputBinding, MemoryLease, MemoryLeaseOwner, MemoryScope, MemoryScopePort,
    ObjectBatchInput, RegisteredObject, RegisteredObjectBatch, SchemaId, StorageFormatId,
    StoredObject,
};
use std::collections::BTreeSet;
use std::sync::atomic::AtomicU64;

struct Corpus {
    fixture: VisitorFixture,
    grams: LiteralGrams,
}

fn large_budget() -> Result<WorkBudget, IndexError> {
    Ok(WorkBudget::new(
        WorkLimits {
            reads: u64::MAX,
            writes: u64::MAX,
            bytes: u64::MAX,
            mutations: u64::MAX,
        },
        Instant::now()
            .checked_add(Duration::from_secs(600))
            .ok_or(IndexError::ResourceLimited)?,
    ))
}

fn tagged_ref(tag: u8, ordinal: usize) -> Result<ObjectRef, IndexError> {
    let mut digest = [tag; 32];
    for (slot, byte) in digest.iter_mut().skip(1).zip(
        u32::try_from(ordinal)
            .map_err(|_width| IndexError::ResourceLimited)?
            .to_be_bytes(),
    ) {
        *slot = byte;
    }
    Ok(ObjectRef(Digest(digest)))
}

fn digest_ref(payload: &[u8]) -> ObjectRef {
    ObjectRef(Digest(Sha256::digest(payload).into()))
}

/// A packed Literal member over every raw byte trigram of `bodies`; owner `i`
/// holds `bodies[i]`.
fn corpus(bodies: &[String]) -> Result<Corpus, IndexError> {
    let mut fixture = visitor_fixture()?;
    let mut keys = BTreeSet::new();
    for (ordinal, body) in bodies.iter().enumerate() {
        let mut owner = [0_u8; 32];
        for (slot, byte) in owner.iter_mut().zip(fixture.owner.producer.0) {
            *slot = byte;
        }
        for (slot, byte) in owner.iter_mut().skip(16).zip(
            u32::try_from(ordinal)
                .map_err(|_width| IndexError::ResourceLimited)?
                .to_be_bytes(),
        ) {
            *slot = byte;
        }
        for window in body.as_bytes().windows(3) {
            let gram = <[u8; 3]>::try_from(window).map_err(|_width| IndexError::ResourceLimited)?;
            let _inserted = keys.insert((gram, owner));
        }
    }
    let mut work = large_budget()?;
    let sources = fixture.ticket.view.source.owners;
    let blocks = MapRoot(Some(tagged_ref(0xd2, 0)?));
    let mut builder = LiteralBlockBuilder::new(&work, None)?;
    let mut pending = Vec::new();
    for (gram, owner) in &keys {
        let key = literal_key(*gram, *owner);
        if builder.try_push(key, &mut work)? == PushResult::Full {
            pending.push(builder.finish()?);
            builder = LiteralBlockBuilder::new(&work, None)?;
            check!(builder.try_push(key, &mut work)? == PushResult::Added);
        }
    }
    if !builder.is_empty() {
        pending.push(builder.finish()?);
    }
    for block in pending {
        let last = block.last_key;
        let (input, _memory) = block.into_batch_input();
        let reference = digest_ref(&input.payload);
        fixture.objects.add(reference, input.payload, Vec::new())?;
        fixture.maps.add(blocks, last.to_vec(), reference)?;
    }
    let root = PackedRoot { sources, blocks };
    let payload = root.fixture_payload()?;
    let artifact = digest_ref(&payload);
    fixture.objects.add(
        artifact,
        payload.clone(),
        root.children().as_slice().to_vec(),
    )?;
    let member = MemberBinding {
        instance: IndexInstanceId([0x4d; 16]),
        format: StorageFormatId([0x4c; 16]),
        artifact,
        input: InputBinding {
            root: sources,
            schema: SchemaId([0x4e; 16]),
            policy: Digest([0x4f; 32]),
            context: Digest([0x50; 32]),
        },
        logical_commitment: Digest(Sha256::digest(&payload).into()),
    };
    let grams = LiteralGrams::open(&member, sources, &fixture.objects, &mut large_budget()?)?;
    Ok(Corpus { fixture, grams })
}

/// Both anchors over one needle, as candidate sets and charged reads.
#[expect(
    clippy::type_complexity,
    reason = "a test helper pairing two candidate sets with their read charges"
)]
fn both(
    corpus: &Corpus,
    needle: &[char],
    variants: &dyn CharVariants,
) -> Result<((Option<Vec<OwnerKey>>, u64), (Option<Vec<OwnerKey>>, u64)), IndexError> {
    let fixture = &corpus.fixture;
    let mut merge_budget = large_budget()?;
    let merged = anchor_chars(
        corpus.grams,
        needle,
        variants,
        &mut TextReadContext {
            ticket: &fixture.ticket,
            objects: &fixture.objects,
            maps: &fixture.maps,
            budget: &mut merge_budget,
        },
    )?
    .map(|anchor| anchor.owners().to_vec());
    let mut probe_budget = large_budget()?;
    let probed = anchor_chars_probing(
        corpus.grams,
        needle,
        variants,
        &mut TextReadContext {
            ticket: &fixture.ticket,
            objects: &fixture.objects,
            maps: &fixture.maps,
            budget: &mut probe_budget,
        },
    )?
    .map(|anchor| anchor.owners().to_vec());
    Ok((
        (merged, merge_budget.usage().reads),
        (probed, probe_budget.usage().reads),
    ))
}

/// A mid-selective needle: 600 of 2,000 bodies hold it. Probing reads a
/// directory entry and a block per candidate per other window; the merge
/// reads each posting block once.
#[test]
fn merge_join_reads_each_posting_block_once() -> Result<(), IndexError> {
    let bodies: Vec<String> = (0..2_000_usize)
        .map(|ordinal| {
            if ordinal % 10 < 3 {
                format!("let value{ordinal} = needle_alpha_beta();")
            } else {
                format!("let other{ordinal} = filler_{}();", ordinal % 97)
            }
        })
        .collect();
    let corpus = corpus(&bodies)?;
    let needle: Vec<char> = "needle_alpha_beta".chars().collect();
    let ((merged, merged_reads), (probed, probed_reads)) = both(&corpus, &needle, &ExactScalars)?;
    check_eq!(merged, probed);
    check_eq!(merged.map(|owners| owners.len()), Some(600));
    check!(
        merged_reads <= 300,
        "merge-join anchor read {merged_reads} times (probing read {probed_reads})"
    );
    Ok(())
}

/// A fixed xorshift stream.
pub(super) struct Rng(pub(super) u64);

impl Rng {
    fn next(&mut self) -> u64 {
        self.0 ^= self.0 << 13;
        self.0 ^= self.0 >> 7;
        self.0 ^= self.0 << 17;
        self.0
    }

    pub(super) fn below(&mut self, bound: usize) -> Result<usize, IndexError> {
        let bound = u64::try_from(bound).map_err(|_width| IndexError::ResourceLimited)?;
        usize::try_from(self.next().checked_rem(bound.max(1)).unwrap_or_default())
            .map_err(|_width| IndexError::ResourceLimited)
    }
}

/// Short bodies over a small alphabet with case and normalization hazards.
///
/// The alphabet holds the Kelvin sign, long s, sharp s, a precomposed and a
/// decomposed é, Greek sigma forms, and dotted and dotless i. Fixed seed, so
/// every failing needle is hand-checkable.
fn random_bodies(rng: &mut Rng, count: usize) -> Result<Vec<String>, IndexError> {
    const ALPHABET: [&str; 20] = [
        "a", "b", "k", "K", "\u{212A}", "s", "\u{17F}", "S", "ß", "é", "e\u{301}", "σ", "ς", "Σ",
        "_", "/", "i", "I", "\u{130}", "\u{131}",
    ];
    let mut bodies = Vec::new();
    for _ in 0..count {
        let len = rng.below(18)?.saturating_add(4);
        let mut body = String::new();
        for _ in 0..len {
            body.push_str(
                ALPHABET
                    .get(rng.below(ALPHABET.len())?)
                    .copied()
                    .unwrap_or("a"),
            );
        }
        bodies.push(body);
    }
    Ok(bodies)
}

/// Needles cut from the bodies (so most match somewhere) and random ones.
fn random_needles(
    rng: &mut Rng,
    bodies: &[String],
    count: usize,
) -> Result<Vec<String>, IndexError> {
    let mut needles = Vec::new();
    for round in 0..count {
        if round % 4 == 3 {
            needles.extend(random_bodies(rng, 1)?);
            continue;
        }
        let body: Vec<char> = bodies
            .get(rng.below(bodies.len())?)
            .map(|body| body.chars().collect())
            .unwrap_or_default();
        let start = rng.below(body.len())?;
        let len = rng.below(6)?.saturating_add(3);
        needles.push(body.iter().skip(start).take(len).collect());
    }
    Ok(needles)
}

/// Every body a matcher accepts must be in an anchor that is not `None`.
fn check_superset(
    corpus: &Corpus,
    bodies: &[String],
    anchor: Option<&Vec<OwnerKey>>,
    accepts: impl Fn(&str) -> bool,
    label: &str,
) -> Result<(), IndexError> {
    let Some(owners) = anchor else {
        return Ok(());
    };
    for (ordinal, body) in bodies.iter().enumerate() {
        if !accepts(body) {
            continue;
        }
        let mut stable = [0_u8; 16];
        for (slot, byte) in stable.iter_mut().zip(
            u32::try_from(ordinal)
                .map_err(|_width| IndexError::ResourceLimited)?
                .to_be_bytes(),
        ) {
            *slot = byte;
        }
        let owner = OwnerKey {
            producer: corpus.fixture.owner.producer,
            stable_id: StableOwnerId(stable),
        };
        check!(
            owners.binary_search(&owner).is_ok(),
            "{label}: matching body {body:?} is outside the anchor"
        );
    }
    Ok(())
}

/// The merge-join anchor equals the probing oracle on random needles.
///
/// Both are also supersets of the true matches: byte-exact containment, the
/// regex engine's case-insensitive literal, and the `file.contains` policy
/// image in both case modes.
#[test]
fn merge_join_anchor_equals_probing_on_random_needles() -> Result<(), IndexError> {
    let mut rng = Rng(0x5eed_c0de_1234_5678);
    let bodies = random_bodies(&mut rng, 80)?;
    let corpus = corpus(&bodies)?;
    let needles = random_needles(&mut rng, &bodies, 160)?;
    let mut anchored = 0_usize;
    for needle in &needles {
        let chars: Vec<char> = needle.chars().collect();
        let sources: [(&str, &dyn CharVariants); 2] =
            [("exact", &ExactScalars), ("fold", &SimpleCaseFold)];
        for (label, variants) in sources {
            let ((merged, _), (probed, _)) = both(&corpus, &chars, variants)?;
            check!(
                merged == probed,
                "{label} {needle:?}: {merged:?} != {probed:?}"
            );
            let folded = regex::RegexBuilder::new(&regex::escape(needle))
                .case_insensitive(true)
                .build()
                .map_err(|error| IndexError::InvalidInput(error.to_string()))?;
            check_superset(
                &corpus,
                &bodies,
                merged.as_ref(),
                |body| {
                    if label == "exact" {
                        body.contains(needle.as_str())
                    } else {
                        folded.is_match(body)
                    }
                },
                &format!("{label} {needle:?}"),
            )?;
            anchored =
                anchored.saturating_add(usize::from(merged.is_some_and(|set| !set.is_empty())));
        }
        // The `file.contains` policy image and its inverse table.
        for case in [CaseMode::Sensitive, CaseMode::Folded] {
            let fixture = &corpus.fixture;
            let mut merge_budget = large_budget()?;
            let merged = anchor_needle(
                corpus.grams,
                needle,
                case,
                &mut TextReadContext {
                    ticket: &fixture.ticket,
                    objects: &fixture.objects,
                    maps: &fixture.maps,
                    budget: &mut merge_budget,
                },
            )?
            .map(|anchor| anchor.owners().to_vec());
            let image: Vec<char> =
                crate::normalize::apply_case(crate::normalize::nfc(needle).as_ref(), case)
                    .chars()
                    .collect();
            let mut probe_budget = large_budget()?;
            let probed = anchor_chars_probing(
                corpus.grams,
                &image,
                case_inverse(case),
                &mut TextReadContext {
                    ticket: &fixture.ticket,
                    objects: &fixture.objects,
                    maps: &fixture.maps,
                    budget: &mut probe_budget,
                },
            )?
            .map(|anchor| anchor.owners().to_vec());
            check!(
                merged == probed,
                "{case:?} {needle:?}: {merged:?} != {probed:?}"
            );
            let image: String = image.iter().collect();
            check_superset(
                &corpus,
                &bodies,
                merged.as_ref(),
                |body| {
                    crate::normalize::apply_case(crate::normalize::nfc(body).as_ref(), case)
                        .contains(image.as_str())
                },
                &format!("{case:?} {needle:?}"),
            )?;
        }
    }
    // The corpus must exercise real anchors, not only empty or None sets.
    check!(anchored > 100, "only {anchored} non-empty anchors");
    Ok(())
}

/// Every object read through it holds a memory grant for its payload while
/// it lives, as a production object port's stored objects do.
struct ChargedObjects<'a>(&'a dyn ImmutableObjectPort);

impl ImmutableObjectPort for ChargedObjects<'_> {
    fn identify(
        &self,
        payload: &[u8],
        children: &[ObjectRef],
        budget: &mut WorkBudget,
    ) -> Result<RegisteredObject, IndexError> {
        self.0.identify(payload, children, budget)
    }

    fn put(
        &self,
        payload: &[u8],
        children: &[ObjectRef],
        budget: &mut WorkBudget,
    ) -> Result<RegisteredObject, IndexError> {
        self.0.put(payload, children, budget)
    }

    fn install(
        &self,
        payload: &[u8],
        children: &[ObjectRef],
        sink: &dyn ObjectWriteSink,
        budget: &mut WorkBudget,
    ) -> Result<RegisteredObject, IndexError> {
        self.0.install(payload, children, sink, budget)
    }

    fn install_batch(
        &self,
        inputs: &[ObjectBatchInput],
        sink: &dyn ObjectWriteSink,
        budget: &mut WorkBudget,
    ) -> Result<RegisteredObjectBatch, IndexError> {
        self.0.install_batch(inputs, sink, budget)
    }

    fn get(
        &self,
        reference: ObjectRef,
        budget: &mut WorkBudget,
    ) -> Result<StoredObject, IndexError> {
        let stored = self.0.get(reference, budget)?;
        let bytes = StoredObject::payload_allocation_bytes(stored.payload.len())?;
        let memory = budget.reserve_memory(bytes, MemoryLifetime::Temporary)?;
        Ok(StoredObject::new(
            stored.registration.clone(),
            stored.payload.clone(),
            memory,
        ))
    }
}

/// A memory scope that refuses any reservation past `cap` live bytes.
struct Capped {
    live: AtomicU64,
    cap: u64,
}

impl MemoryLeaseOwner for Capped {
    fn release(&self, bytes: u64) {
        let _prior = self.live.fetch_sub(bytes, Ordering::SeqCst);
    }
}

struct CappedScope(Arc<Capped>);

impl MemoryScopePort for CappedScope {
    fn reserve(&self, bytes: u64, _lifetime: MemoryLifetime) -> Result<MemoryLease, IndexError> {
        let prior = self.0.live.fetch_add(bytes, Ordering::SeqCst);
        if prior
            .checked_add(bytes)
            .is_none_or(|live| live > self.0.cap)
        {
            let _undo = self.0.live.fetch_sub(bytes, Ordering::SeqCst);
            return Err(IndexError::ResourceLimited);
        }
        Ok(MemoryLease::new(bytes, self.0.clone()))
    }
}

/// One anchor under a `cap`-byte memory grant, every posting block charged.
fn capped_anchor(
    corpus: &Corpus,
    needle: &[char],
    cap: u64,
    merge: bool,
) -> Result<(Option<Vec<OwnerKey>>, u64), IndexError> {
    let fixture = &corpus.fixture;
    let mut budget = large_budget()?;
    budget.bind_memory_scope(Some(MemoryScope::new(Arc::new(CappedScope(Arc::new(
        Capped {
            live: AtomicU64::new(0),
            cap,
        },
    ))))));
    let objects = ChargedObjects(&fixture.objects);
    let mut context = TextReadContext {
        ticket: &fixture.ticket,
        objects: &objects,
        maps: &fixture.maps,
        budget: &mut budget,
    };
    let anchor = if merge {
        anchor_chars(corpus.grams, needle, &ExactScalars, &mut context)?
    } else {
        anchor_chars_probing(corpus.grams, needle, &ExactScalars, &mut context)?
    };
    Ok((
        anchor.map(|anchor| anchor.owners().to_vec()),
        budget.usage().reads,
    ))
}

/// A long needle holds a bounded number of posting blocks.
///
/// A 4,096-character needle has about 4,094 windows. Under a memory grant
/// that the per-owner probing oracle answers within, the merge-join must
/// answer too: the blocks it holds at once are bounded by one combination's
/// grams, not by the needle length.
#[test]
fn long_needle_holds_a_bounded_number_of_posting_blocks() -> Result<(), IndexError> {
    let mut rng = Rng(0x10e9_4096_b10c_0001);
    let mut letters = |len: usize| -> Result<String, IndexError> {
        (0..len)
            .map(|_| {
                let offset =
                    u8::try_from(rng.below(26)?).map_err(|_width| IndexError::ResourceLimited)?;
                Ok(char::from(b'a'.saturating_add(offset)))
            })
            .collect()
    };
    let needle = letters(4_096)?;
    let mut bodies = Vec::new();
    for ordinal in 0..48_usize {
        let filler = letters(2_048)?;
        bodies.push(if ordinal % 8 == 0 {
            format!("{filler} {needle} {filler}")
        } else {
            filler
        });
    }
    let corpus = corpus(&bodies)?;
    let chars: Vec<char> = needle.chars().collect();
    let cap = 4 << 20;
    let (probed, probed_reads) = capped_anchor(&corpus, &chars, cap, false)
        .map_err(|error| IndexError::BackendViolation(format!("probing: {error:?}")))?;
    check_eq!(probed.as_ref().map(Vec::len), Some(6));
    let (merged, merged_reads) = capped_anchor(&corpus, &chars, cap, true)
        .map_err(|error| IndexError::BackendViolation(format!("merge-join: {error:?}")))?;
    check_eq!(merged, probed);
    check!(
        merged_reads <= probed_reads,
        "merge read {merged_reads} times, probing {probed_reads}"
    );
    Ok(())
}
