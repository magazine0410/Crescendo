//! Saved internet radio stations.

use super::Db;
use crate::radio::{NewStation, Station};
use anyhow::{bail, Result};
use rusqlite::{params, OptionalExtension, Row};

const COLS: &str = "id, name, stream_url, homepage, logo_url, tags, country, codec, bitrate, uuid";

fn station_from_row(r: &Row) -> rusqlite::Result<Station> {
    Ok(Station {
        id: r.get(0)?,
        name: r.get(1)?,
        stream_url: r.get(2)?,
        homepage: r.get(3)?,
        logo_url: r.get(4)?,
        logo: None,
        tags: r.get(5)?,
        country: r.get(6)?,
        codec: r.get(7)?,
        bitrate: r.get(8)?,
        uuid: r.get(9)?,
    })
}

impl Db {
    /// Saved stations in list order.
    pub fn stations(&self) -> Result<Vec<Station>> {
        self.with(|conn| {
            let mut stmt =
                conn.prepare(&format!("SELECT {COLS} FROM stations ORDER BY position, id"))?;
            let rows = stmt.query_map([], station_from_row)?;
            Ok(rows.collect::<rusqlite::Result<_>>()?)
        })
    }

    pub fn station(&self, id: i64) -> Result<Option<Station>> {
        self.with(|conn| {
            Ok(conn
                .query_row(
                    &format!("SELECT {COLS} FROM stations WHERE id = ?1"),
                    [id],
                    station_from_row,
                )
                .optional()?)
        })
    }

    /// Save a station at the end of the list. A directory station saved
    /// before is not added twice: its existing id is returned.
    pub fn station_add(&self, new: &NewStation) -> Result<i64> {
        crate::radio::check_stream_url(&new.stream_url)?;
        let name = new.name.trim();
        if name.is_empty() {
            bail!("a station needs a name");
        }
        self.with(|conn| {
            if let Some(uuid) = &new.uuid {
                let existing: Option<i64> = conn
                    .query_row("SELECT id FROM stations WHERE uuid = ?1", [uuid], |r| {
                        r.get(0)
                    })
                    .optional()?;
                if let Some(id) = existing {
                    return Ok(id);
                }
            }
            conn.execute(
                "INSERT INTO stations(name, stream_url, homepage, logo_url, tags, country,
                                      codec, bitrate, uuid, position)
                 VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9,
                         (SELECT COALESCE(MAX(position), 0) + 1 FROM stations))",
                params![
                    name,
                    new.stream_url.trim(),
                    new.homepage,
                    new.logo_url,
                    new.tags,
                    new.country,
                    new.codec,
                    new.bitrate,
                    new.uuid
                ],
            )?;
            Ok(conn.last_insert_rowid())
        })
    }

    pub fn station_rename(&self, id: i64, name: &str) -> Result<()> {
        let name = name.trim();
        if name.is_empty() {
            bail!("a station needs a name");
        }
        self.with(|conn| {
            if conn.execute("UPDATE stations SET name = ?2 WHERE id = ?1", params![id, name])? == 0
            {
                bail!("station {id} is not saved");
            }
            Ok(())
        })
    }

    pub fn station_remove(&self, id: i64) -> Result<()> {
        self.with(|conn| {
            conn.execute("DELETE FROM stations WHERE id = ?1", [id])?;
            Ok(())
        })
    }

    /// Saved station ids in list order, for Next and Previous.
    pub fn station_ids(&self) -> Result<Vec<i64>> {
        self.with(|conn| {
            let mut stmt = conn.prepare("SELECT id FROM stations ORDER BY position, id")?;
            let rows = stmt.query_map([], |r| r.get(0))?;
            Ok(rows.collect::<rusqlite::Result<_>>()?)
        })
    }
}
