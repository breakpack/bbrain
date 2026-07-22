pub mod highlight_repo;
pub mod migrations;
pub mod page_repo;
pub mod paper_repo;
pub mod settings_repo;

use std::path::Path;
use std::sync::{Mutex, MutexGuard};

use rusqlite::Connection;

use crate::error::Result;

/// Serialized access to one SQLite connection. v1 has a single writer and short
/// reads, so a mutex is simpler than a pool and rules out `SQLITE_BUSY`.
pub struct Database {
    conn: Mutex<Connection>,
}

/// sqlite-vec is a statically linked extension. Registering it as an auto
/// extension must happen before any connection is opened, and applies to every
/// connection opened afterwards.
static REGISTER_VEC: std::sync::Once = std::sync::Once::new();

fn register_sqlite_vec() {
    REGISTER_VEC.call_once(|| {
        unsafe {
            rusqlite::ffi::sqlite3_auto_extension(Some(std::mem::transmute::<
                *const (),
                unsafe extern "C" fn(
                    *mut rusqlite::ffi::sqlite3,
                    *mut *mut i8,
                    *const rusqlite::ffi::sqlite3_api_routines,
                ) -> i32,
            >(sqlite_vec::sqlite3_vec_init as *const ())));
        }
    });
}

impl Database {
    pub fn open(path: &Path) -> Result<Self> {
        register_sqlite_vec();
        let mut conn = Connection::open(path)?;
        apply_pragmas(&conn)?;
        migrations::migrate(&mut conn)?;
        Ok(Self {
            conn: Mutex::new(conn),
        })
    }

    #[cfg(test)]
    pub fn open_in_memory() -> Result<Self> {
        register_sqlite_vec();
        let mut conn = Connection::open_in_memory()?;
        apply_pragmas(&conn)?;
        migrations::migrate(&mut conn)?;
        Ok(Self {
            conn: Mutex::new(conn),
        })
    }

    pub fn conn(&self) -> MutexGuard<'_, Connection> {
        // A poisoned lock means a previous holder panicked mid-statement. The
        // connection itself is still usable, and SQLite rolls back the aborted
        // transaction, so recover rather than cascading the panic.
        self.conn.lock().unwrap_or_else(|e| e.into_inner())
    }
}

pub fn apply_pragmas(conn: &Connection) -> Result<()> {
    conn.pragma_update(None, "journal_mode", "WAL")?;
    conn.pragma_update(None, "synchronous", "NORMAL")?;
    conn.pragma_update(None, "foreign_keys", true)?;
    conn.pragma_update(None, "busy_timeout", 5_000)?;
    Ok(())
}
