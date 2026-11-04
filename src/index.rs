//! The resolver's own index, a SQLite file next to the store (`index.sqlite` in the data
//! directory): what this machine holds and why, which the store's trait does not enumerate.
//! The store stays the record of every receipt, descriptor, binding and version; the index
//! only names them.
//!
//! | Table | Rows |
//! |---|---|
//! | `names` | function names held (fetched or defined here), by host |
//! | `cells` | stored cells held, by cell id: the host, the cell URI's rest without `asof`, and the slice that brought it, if any |
//! | `slices` | slices prefetched: the slice URI's rest, the host, the members last seen |
//! | `forwarded` | answers of the forward mesh, by the request's cell URI rest: the envelope as received |
//! | `answered` | receipts this machine signed and answered with (for export) |
//! | `quarantine` | imported receipts signed by another machine's local key, not trusted |
//! | `trusted` | other machines' local keys trusted with `import --trust-key`: their key entries |

use std::path::Path;
use std::sync::Mutex;

use rusqlite::{params, Connection, OptionalExtension};

use crate::Error;

const SCHEMA: &str = "
PRAGMA journal_mode = WAL;
CREATE TABLE IF NOT EXISTS names (name TEXT PRIMARY KEY, host TEXT NOT NULL, at TEXT NOT NULL);
CREATE TABLE IF NOT EXISTS cells (cell_id TEXT PRIMARY KEY, host TEXT NOT NULL, rest TEXT NOT NULL, slice TEXT, at TEXT NOT NULL);
CREATE TABLE IF NOT EXISTS slices (rest TEXT PRIMARY KEY, host TEXT NOT NULL, members INTEGER NOT NULL, at TEXT NOT NULL);
CREATE TABLE IF NOT EXISTS forwarded (rest TEXT PRIMARY KEY, mesh TEXT NOT NULL, status INTEGER NOT NULL, body BLOB NOT NULL, cell_id TEXT NOT NULL, at TEXT NOT NULL);
CREATE TABLE IF NOT EXISTS answered (receipt TEXT PRIMARY KEY, cell_id TEXT NOT NULL, at TEXT NOT NULL);
CREATE TABLE IF NOT EXISTS quarantine (receipt TEXT PRIMARY KEY, key_id TEXT NOT NULL, bytes BLOB NOT NULL, at TEXT NOT NULL);
CREATE TABLE IF NOT EXISTS trusted (key_id TEXT PRIMARY KEY, entry TEXT NOT NULL, at TEXT NOT NULL);
";

/// A forwarded answer as cached.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Forwarded {
    /// The request's cell URI rest.
    pub rest: String,
    /// The mesh that answered.
    pub mesh: String,
    /// Its HTTP status.
    pub status: u16,
    /// The envelope, as received.
    pub body: Vec<u8>,
    /// The cell id.
    pub cell_id: String,
}

/// A held stored cell.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct HeldCell {
    /// The cell id.
    pub cell_id: String,
    /// Its host.
    pub host: String,
    /// The cell URI's rest, without `asof`.
    pub rest: String,
    /// The slice (its URI rest) that brought it, if any.
    pub slice: Option<String>,
}

/// The index (see the module documentation).
pub struct Index {
    conn: Mutex<Connection>,
}

impl std::fmt::Debug for Index {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("Index").finish_non_exhaustive()
    }
}

fn e(err: rusqlite::Error) -> Error {
    Error::Store(format!("index: {err}"))
}

impl Index {
    /// The index in `path`, created when absent.
    pub fn open(path: &Path) -> Result<Index, Error> {
        let conn = Connection::open(path).map_err(e)?;
        conn.busy_timeout(std::time::Duration::from_secs(30)).map_err(e)?;
        conn.execute_batch(SCHEMA).map_err(e)?;
        Ok(Index { conn: Mutex::new(conn) })
    }

    fn with<T>(&self, f: impl FnOnce(&Connection) -> rusqlite::Result<T>) -> Result<T, Error> {
        let c = self.conn.lock().unwrap_or_else(|p| p.into_inner());
        f(&c).map_err(e)
    }

    /// Records a held name.
    pub fn hold_name(&self, name: &str, host: &str, at: &str) -> Result<(), Error> {
        self.with(|c| {
            c.execute("INSERT OR IGNORE INTO names (name, host, at) VALUES (?1, ?2, ?3)", params![name, host, at])
                .map(|_| ())
        })
    }

    /// The held names and their hosts.
    pub fn names(&self) -> Result<Vec<(String, String)>, Error> {
        self.with(|c| {
            let mut s = c.prepare("SELECT name, host FROM names ORDER BY name")?;
            let rows = s.query_map([], |r| Ok((r.get(0)?, r.get(1)?)))?;
            rows.collect()
        })
    }

    /// Records a held stored cell (the slice kept when one brought it before).
    pub fn hold_cell(&self, cell: &HeldCell, at: &str) -> Result<(), Error> {
        self.with(|c| {
            c.execute(
                "INSERT INTO cells (cell_id, host, rest, slice, at) VALUES (?1, ?2, ?3, ?4, ?5)
                 ON CONFLICT (cell_id) DO UPDATE SET slice = COALESCE(cells.slice, excluded.slice)",
                params![cell.cell_id, cell.host, cell.rest, cell.slice, at],
            )
            .map(|_| ())
        })
    }

    /// Whether a cell is held.
    pub fn holds_cell(&self, cell_id: &str) -> Result<bool, Error> {
        self.with(|c| {
            c.query_row("SELECT 1 FROM cells WHERE cell_id = ?1", params![cell_id], |_| Ok(()))
                .optional()
                .map(|o| o.is_some())
        })
    }

    /// The held cells.
    pub fn cells(&self) -> Result<Vec<HeldCell>, Error> {
        self.with(|c| {
            let mut s = c.prepare("SELECT cell_id, host, rest, slice FROM cells ORDER BY cell_id")?;
            let rows = s.query_map([], |r| {
                Ok(HeldCell { cell_id: r.get(0)?, host: r.get(1)?, rest: r.get(2)?, slice: r.get(3)? })
            })?;
            rows.collect()
        })
    }

    /// Records a prefetched slice.
    pub fn hold_slice(&self, rest: &str, host: &str, members: usize, at: &str) -> Result<(), Error> {
        self.with(|c| {
            c.execute(
                "INSERT INTO slices (rest, host, members, at) VALUES (?1, ?2, ?3, ?4)
                 ON CONFLICT (rest) DO UPDATE SET members = excluded.members, at = excluded.at",
                params![rest, host, members as i64, at],
            )
            .map(|_| ())
        })
    }

    /// The prefetched slices: rest and host.
    pub fn slices(&self) -> Result<Vec<(String, String)>, Error> {
        self.with(|c| {
            let mut s = c.prepare("SELECT rest, host FROM slices ORDER BY rest")?;
            let rows = s.query_map([], |r| Ok((r.get(0)?, r.get(1)?)))?;
            rows.collect()
        })
    }

    /// Caches a forwarded answer.
    pub fn put_forwarded(&self, f: &Forwarded, at: &str) -> Result<(), Error> {
        self.with(|c| {
            c.execute(
                "INSERT INTO forwarded (rest, mesh, status, body, cell_id, at) VALUES (?1, ?2, ?3, ?4, ?5, ?6)
                 ON CONFLICT (rest) DO UPDATE SET mesh = excluded.mesh, status = excluded.status,
                   body = excluded.body, cell_id = excluded.cell_id, at = excluded.at",
                params![f.rest, f.mesh, f.status as i64, f.body, f.cell_id, at],
            )
            .map(|_| ())
        })
    }

    /// A cached forwarded answer for the request `rest`.
    pub fn forwarded(&self, rest: &str) -> Result<Option<Forwarded>, Error> {
        self.with(|c| {
            c.query_row("SELECT rest, mesh, status, body, cell_id FROM forwarded WHERE rest = ?1", params![rest], |r| {
                Ok(Forwarded {
                    rest: r.get(0)?,
                    mesh: r.get(1)?,
                    status: r.get::<_, i64>(2)? as u16,
                    body: r.get(3)?,
                    cell_id: r.get(4)?,
                })
            })
            .optional()
        })
    }

    /// Every cached forwarded answer.
    pub fn all_forwarded(&self) -> Result<Vec<Forwarded>, Error> {
        self.with(|c| {
            let mut s = c.prepare("SELECT rest, mesh, status, body, cell_id FROM forwarded ORDER BY rest")?;
            let rows = s.query_map([], |r| {
                Ok(Forwarded {
                    rest: r.get(0)?,
                    mesh: r.get(1)?,
                    status: r.get::<_, i64>(2)? as u16,
                    body: r.get(3)?,
                    cell_id: r.get(4)?,
                })
            })?;
            rows.collect()
        })
    }

    /// Records a receipt this machine answered with.
    pub fn answered(&self, receipt: &str, cell_id: &str, at: &str) -> Result<(), Error> {
        self.with(|c| {
            c.execute(
                "INSERT OR IGNORE INTO answered (receipt, cell_id, at) VALUES (?1, ?2, ?3)",
                params![receipt, cell_id, at],
            )
            .map(|_| ())
        })
    }

    /// The receipts answered with: hash and cell id.
    pub fn all_answered(&self) -> Result<Vec<(String, String)>, Error> {
        self.with(|c| {
            let mut s = c.prepare("SELECT receipt, cell_id FROM answered ORDER BY receipt")?;
            let rows = s.query_map([], |r| Ok((r.get(0)?, r.get(1)?)))?;
            rows.collect()
        })
    }

    /// Quarantines an imported receipt signed by an untrusted local key.
    pub fn quarantine(&self, receipt: &str, key_id: &str, bytes: &[u8], at: &str) -> Result<(), Error> {
        self.with(|c| {
            c.execute(
                "INSERT OR IGNORE INTO quarantine (receipt, key_id, bytes, at) VALUES (?1, ?2, ?3, ?4)",
                params![receipt, key_id, bytes, at],
            )
            .map(|_| ())
        })
    }

    /// The quarantined receipts: hash, key id and bytes.
    pub fn quarantined(&self) -> Result<Vec<(String, String, Vec<u8>)>, Error> {
        self.with(|c| {
            let mut s = c.prepare("SELECT receipt, key_id, bytes FROM quarantine ORDER BY receipt")?;
            let rows = s.query_map([], |r| Ok((r.get(0)?, r.get(1)?, r.get(2)?)))?;
            rows.collect()
        })
    }

    /// Removes a receipt from the quarantine (it was trusted and stored).
    pub fn release(&self, receipt: &str) -> Result<(), Error> {
        self.with(|c| c.execute("DELETE FROM quarantine WHERE receipt = ?1", params![receipt]).map(|_| ()))
    }

    /// Records a trusted machine key (its key document entry, JCS).
    pub fn trust(&self, key_id: &str, entry: &str, at: &str) -> Result<(), Error> {
        self.with(|c| {
            c.execute(
                "INSERT OR IGNORE INTO trusted (key_id, entry, at) VALUES (?1, ?2, ?3)",
                params![key_id, entry, at],
            )
            .map(|_| ())
        })
    }

    /// The trusted machine keys: key id and entry.
    pub fn trusted(&self) -> Result<Vec<(String, String)>, Error> {
        self.with(|c| {
            let mut s = c.prepare("SELECT key_id, entry FROM trusted ORDER BY key_id")?;
            let rows = s.query_map([], |r| Ok((r.get(0)?, r.get(1)?)))?;
            rows.collect()
        })
    }
}
