//! In-memory generation ledger. Authoritative for query-time activation;
//! recovery on restart is by channel replay from the persistent cursor.

mod ledger;

pub use ledger::Ledger;
