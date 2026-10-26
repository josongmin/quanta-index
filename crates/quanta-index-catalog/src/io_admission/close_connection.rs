//! Consuming native close with the original connection restored on failure.

use std::sync::Mutex;

use rusqlite::{Connection, Error};

pub(super) fn close_connection_with_scope<E, S>(
    slot: &mut Option<Mutex<Connection>>,
    enter_scope: impl FnOnce() -> S,
    map_error: impl FnOnce(Error) -> E,
) -> Result<(), E> {
    if slot.is_none() {
        return Ok(());
    }
    // Scope entry may fail or panic. Keep the original connection in `slot`
    // until close funding and its witness are installed.
    let _scope = enter_scope();
    let Some(mutex) = slot.take() else {
        return Ok(());
    };
    let connection = match mutex.into_inner() {
        Ok(connection) => connection,
        Err(poisoned) => poisoned.into_inner(),
    };
    match connection.close() {
        Ok(()) => Ok(()),
        Err((connection, error)) => {
            *slot = Some(Mutex::new(connection));
            Err(map_error(error))
        }
    }
}
