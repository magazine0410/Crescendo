mod schema;
pub mod collections;
pub mod playlists;
pub mod prefs;
pub mod queries;

use anyhow::Result;
use rusqlite::Connection;
use std::path::{Path, PathBuf};
use std::sync::Mutex;

/// Shared handle to the library database. rusqlite connections are not Sync,
/// so a Mutex guards the main connection; the scanner opens its own
/// connection (WAL allows one writer + many readers across connections).
pub struct Db {
    pub(crate) conn: Mutex<Connection>,
    path: PathBuf,
}

impl Db {
    pub fn open(path: &Path) -> Result<Self> {
        if let Some(parent) = path.parent() {
            std::fs::create_dir_all(parent)?;
        }
        let conn = Self::open_conn(path)?;
        let db = Self { conn: Mutex::new(conn), path: path.to_path_buf() };
        // Once per process, not in `open_conn` — the scanner's side connection
        // must not repeat it.
        db.lyrics_cache_prune(crate::lyrics::CACHE_TTL_SECS)?;
        Ok(db)
    }

    /// A second connection to the same database (for the scanner writer).
    pub fn side_connection(&self) -> Result<Connection> {
        Self::open_conn(&self.path)
    }

    fn open_conn(path: &Path) -> Result<Connection> {
        let conn = Connection::open(path)?;
        conn.execute_batch(
            "PRAGMA journal_mode = WAL;
             PRAGMA synchronous = NORMAL;
             PRAGMA busy_timeout = 10000;
             PRAGMA cache_size = -64000;",
        )?;
        schema::migrate(&conn)?;
        register_udfs(&conn)?;
        Ok(conn)
    }

    pub fn with<T>(&self, f: impl FnOnce(&mut Connection) -> Result<T>) -> Result<T> {
        let mut guard = self.conn.lock().expect("db mutex poisoned");
        f(&mut guard)
    }
}

/// Unicode-aware helpers SQLite lacks: `ucontains(haystack, needle)` does a
/// case-folded substring test (SQLite's LIKE/lower() are ASCII-only, which
/// breaks matching in a heavily Japanese library), and `regexp(pattern, text)`
/// backs the `tag=/re/` query syntax.
fn register_udfs(conn: &Connection) -> Result<()> {
    use rusqlite::functions::FunctionFlags;
    use std::sync::Arc;

    conn.create_scalar_function(
        "ucontains",
        2,
        FunctionFlags::SQLITE_UTF8 | FunctionFlags::SQLITE_DETERMINISTIC,
        |ctx| {
            let hay = ctx.get_raw(0).as_str_or_null().map_err(|e| {
                rusqlite::Error::UserFunctionError(Box::new(e))
            })?;
            let needle = ctx.get_raw(1).as_str_or_null().map_err(|e| {
                rusqlite::Error::UserFunctionError(Box::new(e))
            })?;
            Ok(match (hay, needle) {
                (Some(h), Some(n)) => {
                    let h = h.to_lowercase();
                    let n = n.to_lowercase();
                    h.contains(&n)
                }
                _ => false,
            })
        },
    )?;

    // Cache compiled regexes per connection: queries run the same pattern
    // against every row.
    type RegexCache = std::sync::Mutex<std::collections::HashMap<String, Arc<regex::Regex>>>;
    let cache: Arc<RegexCache> = Arc::new(Default::default());
    conn.create_scalar_function(
        "regexp",
        2,
        FunctionFlags::SQLITE_UTF8 | FunctionFlags::SQLITE_DETERMINISTIC,
        move |ctx| {
            let pattern = ctx.get_raw(0).as_str_or_null().map_err(|e| {
                rusqlite::Error::UserFunctionError(Box::new(e))
            })?;
            let text = ctx.get_raw(1).as_str_or_null().map_err(|e| {
                rusqlite::Error::UserFunctionError(Box::new(e))
            })?;
            let (Some(pattern), Some(text)) = (pattern, text) else {
                return Ok(false);
            };
            let mut cache = cache.lock().expect("regex cache poisoned");
            let re = match cache.get(pattern) {
                Some(re) => re.clone(),
                None => {
                    let re = Arc::new(
                        regex::Regex::new(pattern)
                            .map_err(|e| rusqlite::Error::UserFunctionError(Box::new(e)))?,
                    );
                    cache.insert(pattern.to_string(), re.clone());
                    re
                }
            };
            Ok(re.is_match(text))
        },
    )?;
    Ok(())
}
