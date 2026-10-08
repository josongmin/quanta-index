// Copyright 2012-2015 The Rust Project Developers. See the COPYRIGHT
// file at the top-level directory of this distribution and at
// http://rust-lang.org/COPYRIGHT.
//
// Licensed under the Apache License, Version 2.0 <LICENSE-APACHE or
// http://www.apache.org/licenses/LICENSE-2.0> or the MIT license
// <LICENSE-MIT or http://opensource.org/licenses/MIT>, at your
// option. This file may not be copied, modified, or distributed
// except according to those terms.

use crate::decompose::Decompositions;
use crate::native_scratch_v1::{NormalizationPolicyV1, OrdinaryNormalizationPolicyV1};
use core::{
    fmt::{self, Write},
    iter::FusedIterator,
};
use tinyvec::TinyVec;

#[derive(Clone)]
enum RecompositionState {
    Composing,
    Purging(usize),
    Finished(usize),
}

/// External iterator for a string recomposition's characters.
#[derive(Clone)]
pub struct Recompositions<I> {
    iter: Decompositions<I>,
    state: RecompositionState,
    buffer: TinyVec<[char; 4]>,
    composee: Option<char>,
    last_ccc: Option<u8>,
}

impl<I: Iterator<Item = char>> Recompositions<I> {
    #[cfg(feature = "quanta-native-scratch-v1")]
    pub(crate) fn swap_scratch_v1(
        &mut self,
        decomposition_v1: &mut TinyVec<[(u8, char); 4]>,
        recomposition_v1: &mut TinyVec<[char; 4]>,
    ) {
        self.iter.swap_scratch_v1(decomposition_v1);
        core::mem::swap(&mut self.buffer, recomposition_v1);
    }

    /// Create a new recomposition iterator for canonical compositions (NFC)
    ///
    /// Note that this iterator can also be obtained by directly calling [`.nfc()`](crate::UnicodeNormalization::nfc)
    /// on the iterator.
    #[inline]
    pub fn new_canonical(iter: I) -> Self {
        Recompositions {
            iter: Decompositions::new_canonical(iter),
            state: self::RecompositionState::Composing,
            buffer: TinyVec::new(),
            composee: None,
            last_ccc: None,
        }
    }

    /// Create a new recomposition iterator for compatability compositions (NFkC)
    ///
    /// Note that this iterator can also be obtained by directly calling [`.nfkc()`](crate::UnicodeNormalization::nfkc)
    /// on the iterator.
    #[inline]
    pub fn new_compatible(iter: I) -> Self {
        Recompositions {
            iter: Decompositions::new_compatible(iter),
            state: self::RecompositionState::Composing,
            buffer: TinyVec::new(),
            composee: None,
            last_ccc: None,
        }
    }
}

impl<I: Iterator<Item = char>> Recompositions<I> {
    #[inline]
    pub(crate) fn try_next_with_policy_v1<P: NormalizationPolicyV1>(
        &mut self,
        policy: &mut P,
    ) -> Result<Option<char>, P::Error> {
        use self::RecompositionState::*;

        loop {
            policy.work_v1(1)?;
            match self.state {
                Composing => {
                    while let Some(ch) = self.iter.try_next_with_policy_v1(policy)? {
                        policy.work_v1(1)?;
                        let ch_class = super::char::canonical_combining_class(ch);
                        let k = match self.composee {
                            None => {
                                if ch_class != 0 {
                                    return Ok(Some(ch));
                                }
                                self.composee = Some(ch);
                                continue;
                            }
                            Some(k) => k,
                        };
                        match self.last_ccc {
                            None => match super::char::compose(k, ch) {
                                Some(r) => {
                                    self.composee = Some(r);
                                    continue;
                                }
                                None => {
                                    if ch_class == 0 {
                                        self.composee = Some(ch);
                                        return Ok(Some(k));
                                    }
                                    policy.push_recomposition_v1(&mut self.buffer, ch)?;
                                    self.last_ccc = Some(ch_class);
                                }
                            },
                            Some(l_class) => {
                                if l_class >= ch_class {
                                    // `ch` is blocked from `composee`
                                    if ch_class == 0 {
                                        self.composee = Some(ch);
                                        self.last_ccc = None;
                                        self.state = Purging(0);
                                        return Ok(Some(k));
                                    }
                                    policy.push_recomposition_v1(&mut self.buffer, ch)?;
                                    self.last_ccc = Some(ch_class);
                                    continue;
                                }
                                match super::char::compose(k, ch) {
                                    Some(r) => {
                                        self.composee = Some(r);
                                        continue;
                                    }
                                    None => {
                                        policy.push_recomposition_v1(&mut self.buffer, ch)?;
                                        self.last_ccc = Some(ch_class);
                                    }
                                }
                            }
                        }
                    }
                    self.state = Finished(0);
                    if self.composee.is_some() {
                        return Ok(self.composee.take());
                    }
                }
                Purging(next) => match self.buffer.get(next).cloned() {
                    None => {
                        self.buffer.clear();
                        self.state = Composing;
                    }
                    s => {
                        self.state = Purging(next + 1);
                        return Ok(s);
                    }
                },
                Finished(next) => match self.buffer.get(next).cloned() {
                    None => {
                        self.buffer.clear();
                        return Ok(self.composee.take());
                    }
                    s => {
                        self.state = Finished(next + 1);
                        return Ok(s);
                    }
                },
            }
        }
    }
}

impl<I: Iterator<Item = char>> Iterator for Recompositions<I> {
    type Item = char;
    #[inline]
    fn next(&mut self) -> Option<char> {
        match self.try_next_with_policy_v1(&mut OrdinaryNormalizationPolicyV1) {
            Ok(value) => value,
            Err(never) => match never {},
        }
    }
}

impl<I: Iterator<Item = char> + FusedIterator> FusedIterator for Recompositions<I> {}

impl<I: Iterator<Item = char> + Clone> fmt::Display for Recompositions<I> {
    fn fmt(&self, f: &mut fmt::Formatter) -> fmt::Result {
        for c in self.clone() {
            f.write_char(c)?;
        }
        Ok(())
    }
}
