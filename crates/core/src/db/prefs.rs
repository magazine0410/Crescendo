use super::Db;
use anyhow::Result;
use std::collections::HashMap;

impl Db {
    pub fn prefs_get_all(&self) -> Result<HashMap<String, String>> {
        self.with(|conn| {
            let mut stmt = conn.prepare("SELECT key, value FROM prefs")?;
            let rows = stmt.query_map([], |r| Ok((r.get::<_, String>(0)?, r.get::<_, String>(1)?)))?;
            let mut out = HashMap::new();
            for row in rows {
                let (k, v) = row?;
                out.insert(k, v);
            }
            Ok(out)
        })
    }

    pub fn prefs_get(&self, key: &str) -> Result<Option<String>> {
        self.with(|conn| {
            Ok(conn
                .query_row("SELECT value FROM prefs WHERE key = ?1", [key], |r| r.get(0))
                .ok())
        })
    }

    /// The frontend's "Prefer romanized names" tick box (default on). Ordering
    /// reads it so ORDER BY follows what the rows actually show — see
    /// `queries::order_sql` and `browse_values`. Never called from inside a
    /// `with` closure: the connection mutex is not reentrant.
    pub fn prefer_romanized(&self) -> bool {
        !matches!(
            self.prefs_get("prefer_romanized").ok().flatten().as_deref(),
            Some("false")
        )
    }

    /// The frontend's "Look up lyrics online" tick box (default on). When off,
    /// the Now Playing panel shows lyrics from local sources only (sidecar
    /// files and embedded tags); nothing is sent to VocaDB or LRCLIB. Read it
    /// like `prefer_romanized` — never from inside a `with` closure.
    pub fn lyrics_online(&self) -> bool {
        !matches!(
            self.prefs_get("lyrics_online").ok().flatten().as_deref(),
            Some("false")
        )
    }

    pub fn prefs_set(&self, key: &str, value: &str) -> Result<()> {
        self.with(|conn| {
            conn.execute(
                "INSERT INTO prefs(key, value) VALUES (?1, ?2)
                 ON CONFLICT(key) DO UPDATE SET value = excluded.value",
                [key, value],
            )?;
            Ok(())
        })
    }

    pub fn prefs_delete(&self, key: &str) -> Result<()> {
        self.with(|conn| {
            conn.execute("DELETE FROM prefs WHERE key = ?1", [key])?;
            Ok(())
        })
    }

    pub fn meta_get(&self, key: &str) -> Result<Option<String>> {
        self.with(|conn| {
            Ok(conn
                .query_row("SELECT value FROM meta WHERE key = ?1", [key], |row| row.get(0))
                .ok())
        })
    }

    pub fn meta_set(&self, key: &str, value: &str) -> Result<()> {
        self.with(|conn| {
            conn.execute(
                "INSERT INTO meta(key, value) VALUES (?1, ?2)
                 ON CONFLICT(key) DO UPDATE SET value = excluded.value",
                [key, value],
            )?;
            Ok(())
        })
    }
}
