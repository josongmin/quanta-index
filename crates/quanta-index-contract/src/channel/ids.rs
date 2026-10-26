//! Channel-specific identifiers. Newtypes are intentional: they prevent mixing
//! op-key domains (chunk vs symbol vs embedding) across tracks.
//!
//! The newtype shapes here are produced by the crate-local `string_newtype!`
//! and `u64_newtype!` macros (see `src/macros.rs`). The macros expand to
//! manual `serde::Serialize` / `serde::Deserialize` impls — proc-macro
//! `#[derive]` for serde is banned workspace-wide.

u64_newtype!(ChannelSeq);

impl ChannelSeq {
    /// Saturating successor — never wraps. Used to step a write cursor without
    /// admitting the arithmetic-overflow lint into the call sites.
    #[must_use]
    pub const fn next(self) -> Self {
        Self(self.0.saturating_add(1))
    }
}

string_newtype!(ChunkId);
string_newtype!(SymbolId);
string_newtype!(EmbeddingId);

#[cfg(test)]
mod admitted_scalar_tests {
    use std::cell::Cell;

    use quanta_index_contract_base::{DecodeBackingAdmissionV1, DecodeBackingRefusalV1};
    use serde::de::{
        DeserializeSeed as _, value::BorrowedStrDeserializer, value::StringDeserializer,
    };

    use super::{ChunkId, SymbolId};

    /// Test-only admission probe; it is not an Original memory owner.
    struct TestAdmission {
        refuse: bool,
        borrowed_copies: Cell<usize>,
        attempt: std::cell::RefCell<quanta_index_contract_base::DecodeAttemptStateV1>,
    }

    impl DecodeBackingAdmissionV1 for TestAdmission {
        fn abort_decode(&self) {
            self.attempt.borrow_mut().abort_v1();
        }

        fn enter_decode_field_v1(
            &self,
            kind: quanta_index_contract_base::DecodeFieldKindV1,
            maximum: usize,
        ) -> Result<quanta_index_contract_base::DecodeFieldTokenV1, DecodeBackingRefusalV1>
        {
            let mut attempt = self.attempt.borrow_mut();
            if !attempt.is_reading_v1() {
                attempt.begin_decode_v1()?;
            }
            attempt.enter_field_v1(kind, maximum)
        }

        fn leave_decode_field_v1(
            &self,
            token: quanta_index_contract_base::DecodeFieldTokenV1,
        ) -> Result<(), DecodeBackingRefusalV1> {
            self.attempt.borrow_mut().leave_field_v1(token)
        }

        fn reserve_text(
            &self,
            destination: &mut String,
            additional: usize,
        ) -> Result<(), DecodeBackingRefusalV1> {
            if self.refuse {
                return Err(DecodeBackingRefusalV1);
            }
            destination
                .try_reserve_exact(additional)
                .map_err(|_allocation_refusal| DecodeBackingRefusalV1)?;
            self.borrowed_copies
                .set(self.borrowed_copies.get().saturating_add(1));
            Ok(())
        }

        fn reserve_bytes(
            &self,
            destination: &mut Vec<u8>,
            additional: usize,
        ) -> Result<(), DecodeBackingRefusalV1> {
            if self.refuse {
                return Err(DecodeBackingRefusalV1);
            }
            destination
                .try_reserve_exact(additional)
                .map_err(|_allocation_refusal| DecodeBackingRefusalV1)
        }

        fn reserve_elements<T>(
            &self,
            destination: &mut Vec<T>,
            additional: usize,
        ) -> Result<(), DecodeBackingRefusalV1> {
            if self.refuse {
                return Err(DecodeBackingRefusalV1);
            }
            destination
                .try_reserve_exact(additional)
                .map_err(|_allocation_refusal| DecodeBackingRefusalV1)
        }

        fn copy_boxed_text(&self, _source: &str) -> Result<Box<str>, DecodeBackingRefusalV1> {
            Err(DecodeBackingRefusalV1)
        }

        fn adopt_parser_text<E>(
            &self,
            _source: &mut Option<String>,
            _validate: impl FnOnce(&str) -> Result<(), E>,
        ) -> Result<String, crate::ParserTextInputErrorV1<E>> {
            Err(DecodeBackingRefusalV1.into())
        }

        fn copy_boxed_parser_text<E>(
            &self,
            _source: &mut Option<String>,
            _validate: impl FnOnce(&str) -> Result<(), E>,
        ) -> Result<Box<str>, crate::ParserTextInputErrorV1<E>> {
            Err(DecodeBackingRefusalV1.into())
        }

        fn verify_parser_owned_bytes(
            &self,
            _source: &[u8],
            _capacity: usize,
        ) -> Result<(), DecodeBackingRefusalV1> {
            if self.refuse {
                return Err(DecodeBackingRefusalV1);
            }
            Ok(())
        }
    }

    #[test]
    fn admitted_ids_preserve_existing_scalar_wire_v1() -> Result<(), Box<dyn std::error::Error>> {
        let admission = TestAdmission {
            refuse: false,
            borrowed_copies: Cell::new(0),
            attempt: std::cell::RefCell::new(
                quanta_index_contract_base::DecodeAttemptStateV1::default(),
            ),
        };
        let chunk = ChunkId::deserialize_seed_with_admission_v1(&admission, 32).deserialize(
            BorrowedStrDeserializer::<serde::de::value::Error>::new("chunk:fixed"),
        )?;
        let symbol = SymbolId::deserialize_seed_with_admission_v1(&admission, 32).deserialize(
            BorrowedStrDeserializer::<serde::de::value::Error>::new("symbol:fixed"),
        )?;
        if chunk != serde_json::from_str::<ChunkId>("\"chunk:fixed\"")?
            || symbol != serde_json::from_str::<SymbolId>("\"symbol:fixed\"")?
            || serde_json::to_string(&chunk)? != "\"chunk:fixed\""
            || serde_json::to_string(&symbol)? != "\"symbol:fixed\""
            || admission.borrowed_copies.get() != 2
        {
            return Err("admitted ID differs from the existing scalar wire".into());
        }
        Ok(())
    }

    #[test]
    fn admitted_ids_reject_limit_denial_and_unproven_owned_input_v1() {
        let admission = TestAdmission {
            refuse: false,
            borrowed_copies: Cell::new(0),
            attempt: std::cell::RefCell::new(
                quanta_index_contract_base::DecodeAttemptStateV1::default(),
            ),
        };
        assert!(
            ChunkId::deserialize_seed_with_admission_v1(&admission, 4)
                .deserialize(BorrowedStrDeserializer::<serde::de::value::Error>::new(
                    "chunk:fixed"
                ))
                .is_err()
        );
        assert_eq!(admission.borrowed_copies.get(), 0);
        assert!(
            ChunkId::deserialize_seed_with_admission_v1(&admission, 32)
                .deserialize(StringDeserializer::<serde::de::value::Error>::new(
                    "chunk:fixed".to_owned()
                ))
                .is_err()
        );

        let refusing = TestAdmission {
            refuse: true,
            borrowed_copies: Cell::new(0),
            attempt: std::cell::RefCell::new(
                quanta_index_contract_base::DecodeAttemptStateV1::default(),
            ),
        };
        assert!(
            SymbolId::deserialize_seed_with_admission_v1(&refusing, 32)
                .deserialize(BorrowedStrDeserializer::<serde::de::value::Error>::new(
                    "symbol:fixed"
                ))
                .is_err()
        );
        assert_eq!(refusing.borrowed_copies.get(), 0);
    }
}
