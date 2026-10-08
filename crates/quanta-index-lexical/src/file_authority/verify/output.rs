//! Typed consumers of the same completed source and posting checks.

use super::{
    AuthorityRoot, Partition, PostingBucketDirectory, PostingDirectory, PostingPageDescriptor,
    PostingSurface, SourceFile, corrupt,
};

pub(crate) trait VerificationOutput: Sized {
    fn prepare(root: &AuthorityRoot) -> Result<Self, String>;
    fn source(&mut self, materialize: impl FnOnce() -> SourceFile);
    fn directory(
        &mut self,
        surface: PostingSurface,
        descriptor: &Partition,
        pages: usize,
    ) -> Result<Option<&mut Vec<PostingPageDescriptor>>, String>;
}

#[derive(Debug, Default)]
pub(crate) struct ServingOutput {
    pub(crate) files: Vec<SourceFile>,
    pub(crate) posting_directory: PostingDirectory,
}

impl VerificationOutput for ServingOutput {
    fn prepare(root: &AuthorityRoot) -> Result<Self, String> {
        let mut output = Self::default();
        output
            .posting_directory
            .path
            .try_reserve_exact(root.path_postings.len())
            .map_err(|_allocation_error| corrupt("path directory allocation refused"))?;
        output
            .posting_directory
            .content
            .try_reserve_exact(root.content_postings.len())
            .map_err(|_allocation_error| corrupt("content directory allocation refused"))?;
        output
            .files
            .try_reserve(root.sources.len())
            .map_err(|_allocation_error| corrupt("source vector allocation refused"))?;
        Ok(output)
    }

    fn source(&mut self, materialize: impl FnOnce() -> SourceFile) {
        #[cfg(test)]
        MATERIALIZATIONS.with(|count| count.set((count.get().0.saturating_add(1), count.get().1)));
        self.files.push(materialize());
    }

    fn directory(
        &mut self,
        surface: PostingSurface,
        descriptor: &Partition,
        pages: usize,
    ) -> Result<Option<&mut Vec<PostingPageDescriptor>>, String> {
        let mut rows = Vec::new();
        rows.try_reserve_exact(pages)
            .map_err(|_allocation_error| corrupt("term directory allocation refused"))?;
        let target = match surface {
            PostingSurface::Path => &mut self.posting_directory.path,
            PostingSurface::Content => &mut self.posting_directory.content,
        };
        target.push(PostingBucketDirectory {
            partition: descriptor.clone(),
            pages: rows,
        });
        #[cfg(test)]
        MATERIALIZATIONS.with(|count| count.set((count.get().0, count.get().1.saturating_add(1))));
        target
            .last_mut()
            .map(|bucket| Some(&mut bucket.pages))
            .ok_or_else(|| corrupt("materialized directory is missing"))
    }
}

/// No source bodies, normalized fields, directories, or serving conversion.
#[derive(Debug)]
pub(crate) struct PublicationOutput;

impl VerificationOutput for PublicationOutput {
    fn prepare(_root: &AuthorityRoot) -> Result<Self, String> {
        Ok(Self)
    }

    fn source(&mut self, _materialize: impl FnOnce() -> SourceFile) {}

    fn directory(
        &mut self,
        _surface: PostingSurface,
        _descriptor: &Partition,
        _pages: usize,
    ) -> Result<Option<&mut Vec<PostingPageDescriptor>>, String> {
        Ok(None)
    }
}

#[cfg(test)]
thread_local! {
    static MATERIALIZATIONS: std::cell::Cell<(usize, usize)> = const { std::cell::Cell::new((0, 0)) };
}

#[cfg(test)]
pub(super) fn materialization_counts() -> (usize, usize) {
    MATERIALIZATIONS.with(std::cell::Cell::get)
}
