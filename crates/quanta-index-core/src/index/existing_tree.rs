//! One tree enumeration/comparison algorithm for ordinary and admitted reads.
//! Admitted callers supply the actual existing-root metadata I/O owner.
use super::error::ExistingFileReadFailureV1;
use super::ports::{ExistingTreeEntryKindV1, ExistingTreeMetadataIoPortV1};
use crate::index::{IndexError, MemoryLease, MemoryLifetime, WorkBudget};
use crate::tree_commitment::{SealedArtifactCommitmentV1, TreeCommitmentMismatchV1};
use std::{
    alloc::Layout,
    ffi::OsStr,
    path::{Path, PathBuf},
};

/// Observed layout DATA and the original grants for its allocations. Holding
/// this value is not an activation or a sealed-content proof. The caller must
/// still hash all committed bytes and retain its actual root/read owners.
pub struct ReadOnlyTreeLayoutV1 {
    observed: Observed<MemoryLease>,
}
impl ReadOnlyTreeLayoutV1 {
    #[must_use]
    pub fn outcome_v1(&self) -> Result<u64, &TreeCommitmentMismatchV1> {
        self.observed.outcome.as_ref().copied()
    }
}

/// One regular file found under a tree: its committed name and its length,
/// from directory metadata alone.
pub(crate) struct TreeEntryV1 {
    pub(crate) name: String,
    pub(crate) bytes: u64,
    pub(crate) path: PathBuf,
}

trait Control {
    type Error: From<std::io::Error>;
    type Grant;
    fn checkpoint(&mut self) -> Result<(), Self::Error>;
    fn reserve(&mut self, layout: Layout) -> Result<Self::Grant, Self::Error>;
}
trait DirectoryControl: Control {
    fn visit(
        &mut self,
        path: &Path,
        visit: &mut dyn FnMut(
            &OsStr,
            ExistingTreeEntryKindV1,
            u64,
            &mut dyn Control<Error = Self::Error, Grant = Self::Grant>,
        ) -> Result<(), Self::Error>,
    ) -> Result<(), Self::Error>;
}
struct Grants<G> {
    grants: Vec<G>,
}
impl<G> Grants<G> {
    fn new() -> Self {
        Self { grants: Vec::new() }
    }
    fn reserve<C: Control<Grant = G> + ?Sized>(
        &mut self,
        control: &mut C,
        allocation: Layout,
    ) -> Result<(), C::Error> {
        if allocation.size() == 0 {
            return Ok(());
        }
        // Retain grants for both collection backings and payloads until every
        // observed allocation is gone. A popped pending row cannot refund its
        // still-live Vec backing. Growth admits the full overlapping backing.
        let grow = self.grants.len() == self.grants.capacity();
        let next = if grow {
            self.grants
                .capacity()
                .checked_mul(2)
                .ok_or_else(|| invalid("layout grant count overflow"))?
                .max(1)
        } else {
            0
        };
        let backing =
            Layout::array::<G>(next).map_err(|_| invalid("layout grant capacity overflow"))?;
        let layout = allocation
            .extend(backing)
            .map_err(|_| invalid("layout grant allocation overflow"))?
            .0
            .pad_to_align();
        let grant = control.reserve(layout)?;
        if grow {
            self.grants
                .try_reserve_exact(next.saturating_sub(self.grants.len()))
                .map_err(|_| invalid("layout grant allocation failed"))?;
        }
        self.grants.push(grant);
        Ok(())
    }
}
struct Observed<G> {
    outcome: Result<u64, TreeCommitmentMismatchV1>,
    _entries: Vec<TreeEntryV1>,
    // Entries and outcome strings drop before their original allocation grants.
    _memory: Grants<G>,
}
fn invalid(message: &'static str) -> std::io::Error {
    std::io::Error::other(message)
}
fn bytes_layout(bytes: usize) -> std::io::Result<Layout> {
    Layout::array::<u8>(bytes).map_err(|_| invalid("layout byte width overflow"))
}
fn reserve_vec<T, C: Control + ?Sized>(
    values: &mut Vec<T>,
    memory: &mut Grants<C::Grant>,
    control: &mut C,
) -> Result<(), C::Error> {
    if values.len() == values.capacity() {
        let next = values
            .capacity()
            .checked_mul(2)
            .ok_or_else(|| invalid("layout vector capacity overflow"))?
            .max(1);
        memory.reserve(
            control,
            Layout::array::<T>(next).map_err(|_| invalid("layout vector width overflow"))?,
        )?;
        values
            .try_reserve_exact(next.saturating_sub(values.len()))
            .map_err(|_| invalid("layout vector allocation failed"))?;
    }
    Ok(())
}
fn list<C: DirectoryControl>(
    root: &Path,
    prefix: &str,
    control: &mut C,
) -> Result<(Vec<TreeEntryV1>, Grants<C::Grant>), C::Error> {
    let mut memory = Grants::new();
    let mut entries = Vec::new();
    let mut pending = Vec::<(PathBuf, String)>::new();
    control.checkpoint()?;
    let root_bytes = root
        .as_os_str()
        .len()
        .checked_add(prefix.len())
        .ok_or_else(|| invalid("layout root width overflow"))?;
    memory.reserve(control, bytes_layout(root_bytes)?)?;
    reserve_vec(&mut pending, &mut memory, control)?;
    pending.push((root.to_path_buf(), prefix.to_owned()));
    while let Some((directory, name)) = pending.pop() {
        control.checkpoint()?;
        control.visit(&directory, &mut |file_name, kind, bytes, control| {
            control.checkpoint()?;
            let file_name = file_name
                .to_str()
                .ok_or_else(|| invalid("non-UTF-8 generation file name"))?;
            if kind == ExistingTreeEntryKindV1::Symlink {
                return Err(invalid("refusing to measure generation symlink").into());
            }
            // Preserve the canonical regular-file tree semantics. The actual
            // physical port may additionally refuse unsupported inode kinds.
            if matches!(
                kind,
                ExistingTreeEntryKindV1::Other | ExistingTreeEntryKindV1::Socket
            ) {
                return Ok(());
            }
            let name_bytes = name
                .len()
                .checked_add(1)
                .and_then(|n| n.checked_add(file_name.len()))
                .ok_or_else(|| invalid("layout entry name overflow"))?;
            let path_bytes = directory
                .as_os_str()
                .len()
                .checked_add(1)
                .and_then(|n| n.checked_add(file_name.len()))
                .ok_or_else(|| invalid("layout entry path overflow"))?;
            memory.reserve(
                control,
                bytes_layout(
                    name_bytes
                        .checked_add(path_bytes)
                        .ok_or_else(|| invalid("layout entry allocation overflow"))?,
                )?,
            )?;
            let mut entry_name = String::new();
            entry_name
                .try_reserve_exact(name_bytes)
                .map_err(|_| invalid("layout name allocation failed"))?;
            entry_name.push_str(&name);
            entry_name.push('/');
            entry_name.push_str(file_name);
            let mut path = PathBuf::new();
            path.try_reserve_exact(path_bytes)
                .map_err(|_| invalid("layout path allocation failed"))?;
            path.push(&directory);
            path.push(file_name);
            if kind == ExistingTreeEntryKindV1::Directory {
                reserve_vec(&mut pending, &mut memory, control)?;
                pending.push((path, entry_name));
            } else {
                reserve_vec(&mut entries, &mut memory, control)?;
                entries.push(TreeEntryV1 {
                    name: entry_name,
                    bytes,
                    path,
                });
            }
            Ok(())
        })?;
    }
    let mut refused = None;
    // Unstable sorting needs no scratch allocation. Check the same request in
    // each comparator; after a refusal comparisons become constant work until
    // sort unwinds, then preserve the exact original failure.
    entries.sort_unstable_by(|left, right| {
        if refused.is_some() {
            return std::cmp::Ordering::Equal;
        }
        if let Err(error) = control.checkpoint() {
            refused = Some(error);
            return std::cmp::Ordering::Equal;
        }
        left.name.cmp(&right.name)
    });
    if let Some(error) = refused {
        return Err(error);
    }
    Ok((entries, memory))
}
fn verify<C: DirectoryControl>(
    root: &Path,
    prefix: &str,
    committed: &[SealedArtifactCommitmentV1],
    control: &mut C,
) -> Result<Observed<C::Grant>, C::Error> {
    let (entries, mut memory) = list(root, prefix, control)?;
    let mut total = 0_u64;
    for artifact in committed {
        control.checkpoint()?;
        let found = entries
            .binary_search_by(|entry| entry.name.as_str().cmp(artifact.name.as_str()))
            .ok()
            .and_then(|index| entries.get(index));
        let mismatch = match found {
            None => Some(false),
            Some(found) if found.bytes != artifact.bytes => Some(true),
            Some(_) => None,
        };
        if let Some(length) = mismatch {
            memory.reserve(control, bytes_layout(artifact.name.len())?)?;
            let outcome = Err(if length {
                TreeCommitmentMismatchV1::Length {
                    name: artifact.name.clone(),
                    on_disk: found.map_or(0, |entry| entry.bytes),
                    committed: artifact.bytes,
                }
            } else {
                TreeCommitmentMismatchV1::Missing {
                    name: artifact.name.clone(),
                }
            });
            return Ok(Observed {
                outcome,
                _entries: entries,
                _memory: memory,
            });
        }
        total = total.saturating_add(artifact.bytes);
    }
    // Borrowed sorted names replace the old temporary BTreeSet, retaining one
    // comparison algorithm and making its allocation layout explicit.
    memory.reserve(
        control,
        Layout::array::<&str>(committed.len())
            .map_err(|_| invalid("committed-name width overflow"))?,
    )?;
    let mut names = Vec::new();
    names
        .try_reserve_exact(committed.len())
        .map_err(|_| invalid("committed-name allocation failed"))?;
    for artifact in committed {
        control.checkpoint()?;
        names.push(artifact.name.as_str());
    }
    let mut refused = None;
    names.sort_unstable_by(|left, right| {
        if refused.is_some() {
            return std::cmp::Ordering::Equal;
        }
        if let Err(error) = control.checkpoint() {
            refused = Some(error);
            return std::cmp::Ordering::Equal;
        }
        left.cmp(right)
    });
    if let Some(error) = refused {
        return Err(error);
    }
    for entry in &entries {
        control.checkpoint()?;
        if names.binary_search(&entry.name.as_str()).is_err() {
            memory.reserve(control, bytes_layout(entry.name.len())?)?;
            let outcome = Err(TreeCommitmentMismatchV1::Extra {
                name: entry.name.clone(),
            });
            return Ok(Observed {
                outcome,
                _entries: entries,
                _memory: memory,
            });
        }
    }
    let outcome = Ok(total);
    Ok(Observed {
        outcome,
        _entries: entries,
        _memory: memory,
    })
}

struct Admitted<'a> {
    io: &'a dyn ExistingTreeMetadataIoPortV1,
    budget: &'a mut WorkBudget,
}
impl Control for Admitted<'_> {
    type Error = ExistingFileReadFailureV1;
    type Grant = MemoryLease;
    fn checkpoint(&mut self) -> Result<(), Self::Error> {
        self.budget.checkpoint().map_err(Into::into)
    }
    fn reserve(&mut self, layout: Layout) -> Result<MemoryLease, Self::Error> {
        let bytes = u64::try_from(layout.size()).map_err(|_| IndexError::ResourceLimited)?;
        self.budget
            .reserve_memory(bytes, MemoryLifetime::Temporary)?
            .ok_or(IndexError::ResourceLimited.into())
    }
}
impl DirectoryControl for Admitted<'_> {
    fn visit(
        &mut self,
        path: &Path,
        visit: &mut dyn FnMut(
            &OsStr,
            ExistingTreeEntryKindV1,
            u64,
            &mut dyn Control<Error = Self::Error, Grant = Self::Grant>,
        ) -> Result<(), Self::Error>,
    ) -> Result<(), Self::Error> {
        let io = self.io;
        io.visit_directory_v1(path, self.budget, &mut |name, kind, bytes, budget| {
            visit(name, kind, bytes, &mut Admitted { io, budget })
        })
    }
}
/// Compare the exact regular-file set and lengths under genuine existing-root
/// metadata custody and the same admitted WorkBudget. This never hashes bytes.
pub fn verify_tree_layout_admitted_v1(
    root: &Path,
    prefix: &str,
    committed: &[SealedArtifactCommitmentV1],
    io: &dyn ExistingTreeMetadataIoPortV1,
    budget: &mut WorkBudget,
) -> Result<ReadOnlyTreeLayoutV1, ExistingFileReadFailureV1> {
    Ok(ReadOnlyTreeLayoutV1 {
        observed: verify(root, prefix, committed, &mut Admitted { io, budget })?,
    })
}

struct Ordinary;
impl Control for Ordinary {
    type Error = std::io::Error;
    type Grant = ();
    fn checkpoint(&mut self) -> std::io::Result<()> {
        Ok(())
    }
    fn reserve(&mut self, _: Layout) -> std::io::Result<()> {
        Ok(())
    }
}
impl DirectoryControl for Ordinary {
    fn visit(
        &mut self,
        path: &Path,
        visit: &mut dyn FnMut(
            &OsStr,
            ExistingTreeEntryKindV1,
            u64,
            &mut dyn Control<Error = Self::Error, Grant = Self::Grant>,
        ) -> std::io::Result<()>,
    ) -> std::io::Result<()> {
        for entry in std::fs::read_dir(path)? {
            let entry = entry?;
            let name = entry.file_name();
            let kind = entry.file_type()?;
            let (kind, bytes) = if kind.is_symlink() {
                (ExistingTreeEntryKindV1::Symlink, 0)
            } else if kind.is_dir() {
                (ExistingTreeEntryKindV1::Directory, 0)
            } else if kind.is_file() {
                (ExistingTreeEntryKindV1::File, entry.metadata()?.len())
            } else {
                (ExistingTreeEntryKindV1::Other, 0)
            };
            visit(&name, kind, bytes, self)?;
        }
        Ok(())
    }
}
pub(crate) fn list_ordinary(root: &Path, prefix: &str) -> std::io::Result<Vec<TreeEntryV1>> {
    Ok(list(root, prefix, &mut Ordinary)?.0)
}
pub(crate) fn verify_ordinary(
    root: &Path,
    prefix: &str,
    committed: &[SealedArtifactCommitmentV1],
) -> std::io::Result<Result<u64, TreeCommitmentMismatchV1>> {
    Ok(verify(root, prefix, committed, &mut Ordinary)?.outcome)
}
