use super::Db;
use anyhow::{bail, Result};
use std::collections::HashMap;

impl Db {
    /// Set a track's rating in stars, or clear it with `None`.
    pub fn set_rating(&self, track_id: i64, stars: Option<f64>) -> Result<()> {
        let stars = stars.map(crate::rating::validate).transpose()?;
        self.with(|conn| {
            let changed = conn.execute(
                "UPDATE tracks SET rating = ?1 WHERE id = ?2",
                rusqlite::params![stars, track_id],
            )?;
            if changed == 0 {
                bail!("track {track_id} is not in the library");
            }
            Ok(())
        })
    }

    /// Every rated track's rating in stars.
    pub fn ratings(&self) -> Result<HashMap<i64, f64>> {
        self.with(|conn| {
            let mut stmt =
                conn.prepare("SELECT id, rating FROM tracks WHERE rating IS NOT NULL")?;
            let rows = stmt.query_map([], |r| Ok((r.get(0)?, r.get(1)?)))?;
            Ok(rows.collect::<rusqlite::Result<_>>()?)
        })
    }

    /// The `shuffle_mode` preference; missing or unknown values mean random.
    pub fn shuffle_mode(&self) -> crate::rating::ShuffleMode {
        self.prefs_get(crate::rating::ShuffleMode::PREF_KEY)
            .ok()
            .flatten()
            .and_then(|value| crate::rating::ShuffleMode::parse(&value))
            .unwrap_or_default()
    }
}
