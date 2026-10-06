// Copyright 2012-2015 The Rust Project Developers. See the COPYRIGHT
// file at the top-level directory of this distribution and at
// http://rust-lang.org/COPYRIGHT.
//
// Licensed under the Apache License, Version 2.0 <LICENSE-APACHE or
// http://www.apache.org/licenses/LICENSE-2.0> or the MIT license
// <LICENSE-MIT or http://opensource.org/licenses/MIT>, at your
// option. This file may not be copied, modified, or distributed
// except according to those terms.
use crate::native_scratch_v1::{NormalizationPolicyV1, OrdinaryNormalizationPolicyV1};
use core::fmt::{self, Write};
use core::iter::{Fuse, FusedIterator};
use core::ops::Range;
use tinyvec::TinyVec;

#[derive(Clone)]
enum DecompositionType {
    Canonical,
    Compatible,
}

/// External iterator for a string decomposition's characters.
#[derive(Clone)]
pub struct Decompositions<I> {
    kind: DecompositionType,
    iter: Fuse<I>,

    // This buffer stores pairs of (canonical combining class, character),
    // pushed onto the end in text order.
    //
    // It's divided into up to three sections:
    // 1) A prefix that is free space;
    // 2) "Ready" characters which are sorted and ready to emit on demand;
    // 3) A "pending" block which stills needs more characters for us to be able
    //    to sort in canonical order and is not safe to emit.
    buffer: TinyVec<[(u8, char); 4]>,
    ready: Range<usize>,
}

impl<I: Iterator<Item = char>> Decompositions<I> {
    /// Create a new decomposition iterator for canonical decompositions (NFD)
    ///
    /// Note that this iterator can also be obtained by directly calling [`.nfd()`](crate::UnicodeNormalization::nfd)
    /// on the iterator.
    #[inline]
    pub fn new_canonical(iter: I) -> Decompositions<I> {
        Decompositions {
            kind: self::DecompositionType::Canonical,
            iter: iter.fuse(),
            buffer: TinyVec::new(),
            ready: 0..0,
        }
    }

    /// Create a new decomposition iterator for compatability decompositions (NFkD)
    ///
    /// Note that this iterator can also be obtained by directly calling [`.nfkd()`](crate::UnicodeNormalization::nfkd)
    /// on the iterator.
    #[inline]
    pub fn new_compatible(iter: I) -> Decompositions<I> {
        Decompositions {
            kind: self::DecompositionType::Compatible,
            iter: iter.fuse(),
            buffer: TinyVec::new(),
            ready: 0..0,
        }
    }
}

impl<I> Decompositions<I> {
    #[inline]
    fn push_back<P: NormalizationPolicyV1>(
        &mut self,
        ch: char,
        policy: &mut P,
    ) -> Result<(), P::Error> {
        policy.work_v1(1)?;
        let class = super::char::canonical_combining_class(ch);

        if class == 0 {
            self.sort_pending(policy)?;
            policy.push_decomposition_v1(&mut self.buffer, (class, ch))?;
            self.ready.end = self.buffer.len();
        } else {
            policy.push_decomposition_v1(&mut self.buffer, (class, ch))?;
        }
        Ok(())
    }

    #[inline]
    fn sort_pending<P: NormalizationPolicyV1>(&mut self, policy: &mut P) -> Result<(), P::Error> {
        // NB: `sort_by_key` is stable, so it will preserve the original text's
        // order within a combining class.
        policy.work_v1(self.buffer.len() - self.ready.end)?;
        policy.sort_v1(&mut self.buffer[self.ready.end..])
    }

    #[inline]
    fn reset_buffer<P: NormalizationPolicyV1>(&mut self, policy: &mut P) -> Result<(), P::Error> {
        // Equivalent to `self.buffer.drain(0..self.ready.end)`
        // but faster than drain() if the buffer is a SmallVec or TinyVec
        let pending = self.buffer.len() - self.ready.end;
        policy.work_v1(pending)?;
        for i in 0..pending {
            self.buffer[i] = self.buffer[i + self.ready.end];
        }
        self.buffer.truncate(pending);
        self.ready = 0..0;
        Ok(())
    }

    #[inline]
    fn increment_next_ready<P: NormalizationPolicyV1>(
        &mut self,
        policy: &mut P,
    ) -> Result<(), P::Error> {
        let next = self.ready.start + 1;
        if next == self.ready.end {
            self.reset_buffer(policy)?;
        } else {
            self.ready.start = next;
        }
        Ok(())
    }
}

impl<I: Iterator<Item = char>> Decompositions<I> {
    #[inline]
    pub(crate) fn try_next_with_policy_v1<P: NormalizationPolicyV1>(
        &mut self,
        policy: &mut P,
    ) -> Result<Option<char>, P::Error> {
        while self.ready.end == 0 {
            policy.work_v1(1)?;
            match (self.iter.next(), &self.kind) {
                (Some(ch), &DecompositionType::Canonical) => {
                    policy.work_v1(1)?;
                    let mut failure = None;
                    super::char::decompose_canonical(ch, |d| {
                        if failure.is_none() {
                            failure = self.push_back(d, policy).err();
                        }
                    });
                    if let Some(cause) = failure {
                        return Err(cause);
                    }
                }
                (Some(ch), &DecompositionType::Compatible) => {
                    policy.work_v1(1)?;
                    let mut failure = None;
                    super::char::decompose_compatible(ch, |d| {
                        if failure.is_none() {
                            failure = self.push_back(d, policy).err();
                        }
                    });
                    if let Some(cause) = failure {
                        return Err(cause);
                    }
                }
                (None, _) => {
                    if self.buffer.is_empty() {
                        return Ok(None);
                    } else {
                        self.sort_pending(policy)?;
                        self.ready.end = self.buffer.len();

                        // This implementation means that we can call `next`
                        // on an exhausted iterator; the last outer `next` call
                        // will result in an inner `next` call. To make this
                        // safe, we use `fuse`.
                        break;
                    }
                }
            }
        }

        // We can assume here that, if `self.ready.end` is greater than zero,
        // it's also greater than `self.ready.start`. That's because we only
        // increment `self.ready.start` inside `increment_next_ready`, and
        // whenever it reaches equality with `self.ready.end`, we reset both
        // to zero, maintaining the invariant that:
        //      self.ready.start < self.ready.end || self.ready.end == self.ready.start == 0
        //
        // This less-than-obviously-safe implementation is chosen for performance,
        // minimizing the number & complexity of branches in `next` in the common
        // case of buffering then unbuffering a single character with each call.
        let (_, ch) = self.buffer[self.ready.start];
        self.increment_next_ready(policy)?;
        Ok(Some(ch))
    }
}

impl<I: Iterator<Item = char>> Iterator for Decompositions<I> {
    type Item = char;
    #[inline]
    fn next(&mut self) -> Option<char> {
        match self.try_next_with_policy_v1(&mut OrdinaryNormalizationPolicyV1) {
            Ok(value) => value,
            Err(never) => match never {},
        }
    }

    fn size_hint(&self) -> (usize, Option<usize>) {
        let (lower, _) = self.iter.size_hint();
        (lower, None)
    }
}

impl<I: Iterator<Item = char> + FusedIterator> FusedIterator for Decompositions<I> {}

impl<I: Iterator<Item = char> + Clone> fmt::Display for Decompositions<I> {
    fn fmt(&self, f: &mut fmt::Formatter) -> fmt::Result {
        for c in self.clone() {
            f.write_char(c)?;
        }
        Ok(())
    }
}
