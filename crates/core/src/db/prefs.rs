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
}
