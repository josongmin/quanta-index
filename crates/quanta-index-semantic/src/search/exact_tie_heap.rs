//! Bounded Top-K ordering for the public-API exact scan.

use std::cmp::Ordering;
use std::collections::BinaryHeap;

use quanta_index_core::CoreError;

#[derive(Clone, Debug)]
pub(super) struct ExactTieKey {
    pub(super) score: f32,
    pub(super) source_repo_id: String,
    pub(super) repo_relative_path: String,
    pub(super) start_line: u32,
    pub(super) end_line: u32,
    pub(super) embedding_id: String,
    pub(super) row_id: u64,
}

impl Ord for ExactTieKey {
    fn cmp(&self, other: &Self) -> Ordering {
        other
            .score
            .total_cmp(&self.score)
            .then_with(|| self.source_repo_id.cmp(&other.source_repo_id))
            .then_with(|| self.repo_relative_path.cmp(&other.repo_relative_path))
            .then_with(|| self.start_line.cmp(&other.start_line))
            .then_with(|| self.end_line.cmp(&other.end_line))
            .then_with(|| self.embedding_id.cmp(&other.embedding_id))
            .then_with(|| self.row_id.cmp(&other.row_id))
    }
}

impl PartialOrd for ExactTieKey {
    fn partial_cmp(&self, other: &Self) -> Option<Ordering> {
        Some(self.cmp(other))
    }
}

impl PartialEq for ExactTieKey {
    fn eq(&self, other: &Self) -> bool {
        self.cmp(other) == Ordering::Equal
    }
}

impl Eq for ExactTieKey {}

struct Entry<T> {
    key: ExactTieKey,
    value: T,
}

impl<T> Ord for Entry<T> {
    fn cmp(&self, other: &Self) -> Ordering {
        self.key.cmp(&other.key)
    }
}

impl<T> PartialOrd for Entry<T> {
    fn partial_cmp(&self, other: &Self) -> Option<Ordering> {
        Some(self.cmp(other))
    }
}

impl<T> PartialEq for Entry<T> {
    fn eq(&self, other: &Self) -> bool {
        self.key == other.key
    }
}

impl<T> Eq for Entry<T> {}

/// The heap head is the worst retained candidate; no row outside K is kept.
pub(super) struct ExactTieHeap<T> {
    capacity: usize,
    rows: BinaryHeap<Entry<T>>,
}

impl<T> ExactTieHeap<T> {
    pub(super) fn new(capacity: usize) -> Result<Self, CoreError> {
        if capacity == 0 {
            return Err(CoreError::InvalidContract(
                "exact Top-K capacity is zero".into(),
            ));
        }
        Ok(Self {
            capacity,
            rows: BinaryHeap::with_capacity(capacity),
        })
    }

    #[cfg(test)]
    pub(super) fn len(&self) -> usize {
        self.rows.len()
    }

    #[cfg(test)]
    pub(super) fn consider_with(
        &mut self,
        key: ExactTieKey,
        materialize: impl FnOnce() -> T,
    ) -> Result<bool, CoreError> {
        self.consider_try_with(key, || Ok(materialize()))
    }

    pub(super) fn consider_try_with(
        &mut self,
        key: ExactTieKey,
        materialize: impl FnOnce() -> Result<T, CoreError>,
    ) -> Result<bool, CoreError> {
        if !key.score.is_finite() {
            return Err(CoreError::Storage(format!(
                "semantic: non-finite exact score for {}",
                key.embedding_id
            )));
        }
        if self.rows.len() == self.capacity {
            let Some(worst) = self.rows.peek() else {
                return Err(CoreError::InvalidContract(
                    "exact Top-K heap lost its head".into(),
                ));
            };
            if key >= worst.key {
                return Ok(false);
            }
            let value = materialize()?;
            drop(self.rows.pop());
            self.rows.push(Entry { key, value });
            return Ok(true);
        }
        self.rows.push(Entry {
            key,
            value: materialize()?,
        });
        Ok(true)
    }

    pub(super) fn into_best(self) -> Vec<(ExactTieKey, T)> {
        self.rows
            .into_sorted_vec()
            .into_iter()
            .map(|entry| (entry.key, entry.value))
            .collect()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn key(score: f32, path: &str, row_id: u64) -> ExactTieKey {
        ExactTieKey {
            score,
            source_repo_id: "repo".into(),
            repo_relative_path: path.into(),
            start_line: 1,
            end_line: 2,
            embedding_id: path.into(),
            row_id,
        }
    }

    #[test]
    fn picks_canonical_first_before_top_k_cut() -> Result<(), CoreError> {
        let mut heap = ExactTieHeap::new(25)?;
        for index in (0..26_u32).rev() {
            let path = format!("src/file_{index:02}.go");
            let _selected =
                heap.consider_with(key(1.0, &path, u64::from(25_u32 - index)), || path)?;
            assert!(heap.len() <= 25);
        }
        let got = heap
            .into_best()
            .into_iter()
            .map(|(_, path)| path)
            .collect::<Vec<_>>();
        let expected = (0..25)
            .map(|index| format!("src/file_{index:02}.go"))
            .collect::<Vec<_>>();
        assert_eq!(got, expected);
        Ok(())
    }

    #[test]
    fn score_precedes_repo_path_and_row_id() -> Result<(), CoreError> {
        let lower = f32::from_bits(1.0_f32.to_bits() - 1);
        let mut rows = vec![
            key(lower, "a", 0),
            key(1.0, "z", 9),
            key(1.0, "a", 9),
            key(1.0, "a", 2),
        ];
        rows[2].source_repo_id = "z".into();
        let mut heap = ExactTieHeap::new(3)?;
        for row in rows {
            let row_id = row.row_id;
            let _selected = heap.consider_with(row, || row_id)?;
        }
        let got = heap
            .into_best()
            .into_iter()
            .map(|(key, _)| {
                (
                    key.score.to_bits(),
                    key.source_repo_id,
                    key.repo_relative_path,
                    key.row_id,
                )
            })
            .collect::<Vec<_>>();
        assert_eq!(
            got,
            vec![
                (1.0_f32.to_bits(), "repo".into(), "a".into(), 2),
                (1.0_f32.to_bits(), "repo".into(), "z".into(), 9),
                (1.0_f32.to_bits(), "z".into(), "a".into(), 9),
            ]
        );
        Ok(())
    }

    #[test]
    fn rounded_reported_score_ties_use_canonical_key_before_raw_distance() -> Result<(), CoreError>
    {
        let closer = 0.25_f32;
        let farther = f32::from_bits(closer.to_bits() + 1);
        assert!(closer < farther);
        let closer_score = 1.0_f32 - closer;
        let farther_score = 1.0_f32 - farther;
        assert_eq!(closer_score.to_bits(), farther_score.to_bits());

        let mut heap = ExactTieHeap::new(1)?;
        let _selected = heap.consider_with(key(closer_score, "z", 1), || "closer")?;
        let _selected = heap.consider_with(key(farther_score, "a", 2), || "canonical")?;
        assert_eq!(heap.into_best()[0].1, "canonical");
        Ok(())
    }

    #[test]
    fn line_embedding_and_row_id_break_remaining_ties() -> Result<(), CoreError> {
        let mut start_later = key(1.0, "same", 10);
        start_later.start_line = 2;
        let mut end_later = key(1.0, "same", 11);
        end_later.end_line = 3;
        let mut id_later = key(1.0, "same", 12);
        id_later.embedding_id = "z".into();
        let first = key(1.0, "same", 1);
        let second = key(1.0, "same", 2);
        let mut heap = ExactTieHeap::new(5)?;
        for item in [start_later, end_later, id_later, second, first] {
            let row_id = item.row_id;
            let _selected = heap.consider_with(item, || row_id)?;
        }
        let row_ids = heap
            .into_best()
            .into_iter()
            .map(|(_, row_id)| row_id)
            .collect::<Vec<_>>();
        assert_eq!(row_ids, vec![1, 2, 12, 11, 10]);
        Ok(())
    }

    #[test]
    fn rejects_nonfinite_without_materializing_or_displacing() -> Result<(), CoreError> {
        let mut heap = ExactTieHeap::new(1)?;
        let _selected = heap.consider_with(key(0.5, "valid", 1), || "valid")?;
        for bad in [f32::NAN, f32::INFINITY, f32::NEG_INFINITY] {
            let result = heap.consider_with(key(bad, "bad", 0), || "bad");
            assert!(matches!(result, Err(CoreError::Storage(_))));
            assert_eq!(heap.len(), 1);
        }
        assert_eq!(heap.into_best()[0].1, "valid");
        Ok(())
    }

    #[test]
    fn failed_selected_materialization_keeps_previous_heap_head() -> Result<(), CoreError> {
        let mut heap = ExactTieHeap::new(1)?;
        let _selected = heap.consider_with(key(0.5, "old", 1), || "old")?;
        let refusal = heap.consider_try_with(key(1.0, "new", 2), || {
            Err(CoreError::Storage("selected row is corrupt".into()))
        });
        assert!(matches!(refusal, Err(CoreError::Storage(_))));
        assert_eq!(heap.into_best()[0].1, "old");
        Ok(())
    }

    #[test]
    fn bounded_heap_matches_independent_full_sort_for_many_ties() -> Result<(), CoreError> {
        let mut state = 0x9e37_79b9_u32;
        for (round, capacity) in (0..128_u64).zip((1..=15_usize).cycle()) {
            let mut heap = ExactTieHeap::new(capacity)?;
            let mut all = Vec::new();
            for row in 0..64_u64 {
                state = state.wrapping_mul(1_664_525).wrapping_add(1_013_904_223);
                let score = match state & 3 {
                    0 => 0.0,
                    1 => 1.0,
                    2 => 2.0,
                    _ => 3.0,
                };
                let mut item = key(score, &format!("path-{}", state.rotate_right(3) & 3), row);
                item.source_repo_id = format!("repo-{}", state.rotate_right(7) & 3);
                item.start_line = state.rotate_right(10) & 3;
                item.end_line = item.start_line.saturating_add(state.rotate_right(12) & 3);
                item.embedding_id = format!("id-{}", state.rotate_right(14) & 3);
                all.push(item.clone());
                let _selected = heap.consider_with(item, || row)?;
                assert!(heap.len() <= capacity);
            }
            all.sort_by(|a, b| {
                b.score
                    .partial_cmp(&a.score)
                    .unwrap_or(Ordering::Equal)
                    .then_with(|| a.source_repo_id.cmp(&b.source_repo_id))
                    .then_with(|| a.repo_relative_path.cmp(&b.repo_relative_path))
                    .then_with(|| a.start_line.cmp(&b.start_line))
                    .then_with(|| a.end_line.cmp(&b.end_line))
                    .then_with(|| a.embedding_id.cmp(&b.embedding_id))
                    .then_with(|| a.row_id.cmp(&b.row_id))
            });
            let expected: Vec<_> = all.iter().take(capacity).map(|item| item.row_id).collect();
            let actual: Vec<_> = heap.into_best().into_iter().map(|(_, row)| row).collect();
            assert_eq!(actual, expected, "round {round}, capacity {capacity}");
        }
        Ok(())
    }
}
