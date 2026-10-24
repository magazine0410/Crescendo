pub mod collections;
pub mod history;
pub mod playlists;
pub mod plugins;
pub mod prefs;
pub mod queries;
mod schema;

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
        let db = Self {
            conn: Mutex::new(conn),
            path: path.to_path_buf(),
        };
        // Once per process, not in `open_conn` — the scanner's side connection
        // must not repeat it.
        db.lastfm_cache_prune(crate::lastfm::CACHE_TTL_SECS)?;
        db.lyrics_cache_prune(crate::lyrics::CACHE_TTL_SECS)?;
        Ok(db)
    }

    /// A second connection to the same database (for the scanner writer).
    pub fn side_connection(&self) -> Result<Connection> {
        Self::open_conn(&self.path)
    }

    fn open_conn(path: &Path) -> Result<Connection> {
        let mut conn = Connection::open(path)?;
        conn.execute_batch(
            "PRAGMA journal_mode = WAL;
             PRAGMA synchronous = NORMAL;
             PRAGMA busy_timeout = 10000;
             PRAGMA cache_size = -64000;",
        )?;
        // Every transaction in this crate writes, and most read first. A
        // deferred one that reads and then finds the scanner's connection
        // holding the write lock fails at once with "database is locked":
        // SQLite does not run the busy handler when upgrading a read
        // transaction. IMMEDIATE takes the write lock at BEGIN, where
        // busy_timeout applies.
        conn.set_transaction_behavior(rusqlite::TransactionBehavior::Immediate);
        schema::migrate(&mut conn)?;
        register_udfs(&conn)?;
        Ok(conn)
    }

    pub fn with<T>(&self, f: impl FnOnce(&mut Connection) -> Result<T>) -> Result<T> {
        let mut guard = self.conn.lock().expect("db mutex poisoned");
        f(&mut guard)
    }
}

/// Kana, CJK ideographs (+ ext. A and compatibility), halfwidth katakana and
/// Hangul: the frontend's CJK_RE, which decides when a sort tag stands in for
/// a name.
pub(crate) fn has_cjk(s: &str) -> bool {
    s.chars().any(|c| {
        matches!(
            c,
            '\u{3040}'..='\u{30ff}'
                | '\u{3400}'..='\u{4dbf}'
                | '\u{4e00}'..='\u{9fff}'
                | '\u{f900}'..='\u{faff}'
                | '\u{ff66}'..='\u{ff9f}'
                | '\u{ac00}'..='\u{d7af}'
        )
    })
}

/// Unicode-aware helpers SQLite lacks: `ulower(text)` supplies deterministic
/// Unicode lowercasing for persisted identity keys, `ucontains(haystack,
/// needle)` does a lowercased substring test, `has_cjk(text)` mirrors the
/// frontend's romanized-display rule, and `regexp(pattern, text)` backs the
/// `tag=/re/` query syntax.
fn register_udfs(conn: &Connection) -> Result<()> {
    use rusqlite::functions::FunctionFlags;
    use std::sync::Arc;

    conn.create_scalar_function(
        "ulower",
        1,
        FunctionFlags::SQLITE_UTF8 | FunctionFlags::SQLITE_DETERMINISTIC,
        |ctx| {
            let value = ctx
                .get_raw(0)
                .as_str_or_null()
                .map_err(|e| rusqlite::Error::UserFunctionError(Box::new(e)))?;
            Ok(value.map(str::to_lowercase))
        },
    )?;

    conn.create_scalar_function(
        "ucontains",
        2,
        FunctionFlags::SQLITE_UTF8 | FunctionFlags::SQLITE_DETERMINISTIC,
        |ctx| {
            let hay = ctx
                .get_raw(0)
                .as_str_or_null()
                .map_err(|e| rusqlite::Error::UserFunctionError(Box::new(e)))?;
            let needle = ctx
                .get_raw(1)
                .as_str_or_null()
                .map_err(|e| rusqlite::Error::UserFunctionError(Box::new(e)))?;
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

    conn.create_scalar_function(
        "has_cjk",
        1,
        FunctionFlags::SQLITE_UTF8 | FunctionFlags::SQLITE_DETERMINISTIC,
        |ctx| {
            let value = ctx
                .get_raw(0)
                .as_str_or_null()
                .map_err(|e| rusqlite::Error::UserFunctionError(Box::new(e)))?;
            Ok(value.is_some_and(has_cjk))
        },
    )?;

    // Cache compiled regexes per connection: queries run the same pattern
    // against every row. Search-as-you-type compiles a new pattern per edit
    // for the life of the process, so the cache is emptied once it holds a
    // few dozen; one query only ever uses a handful.
    type RegexCache = std::sync::Mutex<std::collections::HashMap<String, Arc<regex::Regex>>>;
    let cache: Arc<RegexCache> = Arc::new(Default::default());
    conn.create_scalar_function(
        "regexp",
        2,
        FunctionFlags::SQLITE_UTF8 | FunctionFlags::SQLITE_DETERMINISTIC,
        move |ctx| {
            let pattern = ctx
                .get_raw(0)
                .as_str_or_null()
                .map_err(|e| rusqlite::Error::UserFunctionError(Box::new(e)))?;
            let text = ctx
                .get_raw(1)
                .as_str_or_null()
                .map_err(|e| rusqlite::Error::UserFunctionError(Box::new(e)))?;
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
                    if cache.len() >= 32 {
                        cache.clear();
                    }
                    cache.insert(pattern.to_string(), re.clone());
                    re
                }
            };
            Ok(re.is_match(text))
        },
    )?;
    Ok(())
}
