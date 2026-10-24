use super::Db;
use crate::plugins::{parse_permissions, PluginGrant};
use anyhow::Result;
use rusqlite::OptionalExtension;
use std::collections::HashMap;

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PluginStateRow {
    pub desired_enabled: bool,
    pub failure_count: i64,
    pub last_error: Option<String>,
    /// Startup-initialization failures since the last clean start.
    pub init_failures: i64,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PluginPendingRow {
    pub plugin_id: String,
    pub operation: String,
    pub staged_path: String,
    pub target_version: Option<String>,
    pub package_hash: Option<String>,
    pub remove_data: bool,
}

impl Db {
    pub fn plugin_state_get(&self, plugin_id: &str) -> Result<Option<PluginStateRow>> {
        self.with(|conn| {
            Ok(conn
                .query_row(
                    "SELECT desired_enabled, failure_count, last_error, init_failures
                     FROM plugin_state WHERE plugin_id = ?1",
                    [plugin_id],
                    |row| {
                        Ok(PluginStateRow {
                            desired_enabled: row.get::<_, i64>(0)? != 0,
                            failure_count: row.get(1)?,
                            last_error: row.get(2)?,
                            init_failures: row.get(3)?,
                        })
                    },
                )
                .optional()?)
        })
    }

    pub fn plugin_state_ensure(&self, plugin_id: &str) -> Result<()> {
        self.with(|conn| {
            conn.execute(
                "INSERT INTO plugin_state(plugin_id) VALUES (?1)
                 ON CONFLICT(plugin_id) DO NOTHING",
                [plugin_id],
            )?;
            Ok(())
        })
    }

    pub fn plugin_set_desired_enabled(&self, plugin_id: &str, enabled: bool) -> Result<()> {
        self.with(|conn| {
            conn.execute(
                "INSERT INTO plugin_state(plugin_id, desired_enabled) VALUES (?1, ?2)
                 ON CONFLICT(plugin_id) DO UPDATE SET desired_enabled = excluded.desired_enabled",
                rusqlite::params![plugin_id, i64::from(enabled)],
            )?;
            Ok(())
        })
    }

    pub fn plugin_record_error(&self, plugin_id: &str, error: Option<&str>) -> Result<()> {
        self.with(|conn| {
            match error {
                Some(error) => {
                    conn.execute(
                        "INSERT INTO plugin_state(plugin_id, desired_enabled, failure_count, last_error)
                         VALUES (?1, 0, 1, ?2)
                         ON CONFLICT(plugin_id) DO UPDATE SET
                           desired_enabled = 0,
                           failure_count = plugin_state.failure_count + 1,
                           last_error = excluded.last_error",
                        rusqlite::params![plugin_id, error],
                    )?;
                }
                None => {
                    conn.execute(
                        "INSERT INTO plugin_state(plugin_id) VALUES (?1)
                         ON CONFLICT(plugin_id) DO UPDATE SET
                           failure_count = 0, init_failures = 0, last_error = NULL",
                        [plugin_id],
                    )?;
                }
            }
            Ok(())
        })
    }

    /// Count one failed startup initialization toward auto-disable.
    pub fn plugin_note_init_failure(&self, plugin_id: &str) -> Result<()> {
        self.with(|conn| {
            conn.execute(
                "INSERT INTO plugin_state(plugin_id, init_failures) VALUES (?1, 1)
                 ON CONFLICT(plugin_id) DO UPDATE SET
                   init_failures = plugin_state.init_failures + 1",
                [plugin_id],
            )?;
            Ok(())
        })
    }

    pub fn plugin_record_error_preserving_state(&self, plugin_id: &str, error: &str) -> Result<()> {
        self.with(|conn| {
            conn.execute(
                "INSERT INTO plugin_state(plugin_id, desired_enabled, failure_count, last_error)
                 VALUES (?1, 0, 1, ?2)
                 ON CONFLICT(plugin_id) DO UPDATE SET
                   failure_count = plugin_state.failure_count + 1,
                   last_error = excluded.last_error",
                rusqlite::params![plugin_id, error],
            )?;
            Ok(())
        })
    }

    pub fn plugin_pending_all(&self) -> Result<Vec<PluginPendingRow>> {
        self.with(|conn| {
            let mut stmt = conn.prepare(
                "SELECT plugin_id, operation, staged_path, target_version, package_hash, remove_data
                 FROM plugin_pending ORDER BY plugin_id",
            )?;
            let rows = stmt.query_map([], |row| {
                Ok(PluginPendingRow {
                    plugin_id: row.get(0)?,
                    operation: row.get(1)?,
                    staged_path: row.get(2)?,
                    target_version: row.get(3)?,
                    package_hash: row.get(4)?,
                    remove_data: row.get::<_, i64>(5)? != 0,
                })
            })?;
            rows.collect::<rusqlite::Result<Vec<_>>>()
                .map_err(Into::into)
        })
    }

    pub fn plugin_schedule_pending(
        &self,
        row: &PluginPendingRow,
        desired_enabled: Option<bool>,
    ) -> Result<()> {
        self.with(|conn| {
            let tx = conn.transaction()?;
            tx.execute(
                "INSERT INTO plugin_pending(
                   plugin_id, operation, staged_path, target_version, package_hash, remove_data
                 ) VALUES (?1, ?2, ?3, ?4, ?5, ?6)
                 ON CONFLICT(plugin_id) DO UPDATE SET
                   operation = excluded.operation,
                   staged_path = excluded.staged_path,
                   target_version = excluded.target_version,
                   package_hash = excluded.package_hash,
                   remove_data = excluded.remove_data",
                rusqlite::params![
                    row.plugin_id,
                    row.operation,
                    row.staged_path,
                    row.target_version,
                    row.package_hash,
                    i64::from(row.remove_data),
                ],
            )?;
            if let Some(enabled) = desired_enabled {
                tx.execute(
                    "INSERT INTO plugin_state(plugin_id, desired_enabled) VALUES (?1, ?2)
                     ON CONFLICT(plugin_id) DO UPDATE SET desired_enabled = excluded.desired_enabled",
                    rusqlite::params![row.plugin_id, i64::from(enabled)],
                )?;
            }
            tx.commit()?;
            Ok(())
        })
    }

    pub fn plugin_pending_delete(&self, plugin_id: &str) -> Result<()> {
        self.with(|conn| {
            conn.execute(
                "DELETE FROM plugin_pending WHERE plugin_id = ?1",
                [plugin_id],
            )?;
            Ok(())
        })
    }

    pub fn plugin_pending_commit(&self, plugin_id: &str, remove_data: bool) -> Result<()> {
        self.with(|conn| {
            let tx = conn.transaction()?;
            tx.execute(
                "DELETE FROM plugin_pending WHERE plugin_id = ?1",
                [plugin_id],
            )?;
            if remove_data {
                // "Also remove plugin data" means all of it. Namespaced Wasm
                // storage is the largest part and was previously left behind,
                // so a reinstall inherited the old plugin's data. Grants go too:
                // they are pinned to a package that is being removed, and a
                // future reinstall must be approved afresh.
                tx.execute(
                    "DELETE FROM plugin_settings WHERE plugin_id = ?1",
                    [plugin_id],
                )?;
                tx.execute(
                    "DELETE FROM plugin_storage WHERE plugin_id = ?1",
                    [plugin_id],
                )?;
                tx.execute(
                    "DELETE FROM plugin_grants WHERE plugin_id = ?1",
                    [plugin_id],
                )?;
                tx.execute("DELETE FROM plugin_state WHERE plugin_id = ?1", [plugin_id])?;
            } else {
                // Even when data is kept, the grant does not outlive the
                // installed package it was pinned to.
                tx.execute(
                    "DELETE FROM plugin_grants WHERE plugin_id = ?1",
                    [plugin_id],
                )?;
            }
            tx.commit()?;
            Ok(())
        })
    }

    pub fn plugin_settings_get(
        &self,
        plugin_id: &str,
    ) -> Result<HashMap<String, serde_json::Value>> {
        self.with(|conn| {
            let mut stmt = conn.prepare(
                "SELECT setting_key, value_json FROM plugin_settings WHERE plugin_id = ?1",
            )?;
            let rows = stmt.query_map([plugin_id], |row| {
                Ok((row.get::<_, String>(0)?, row.get::<_, String>(1)?))
            })?;
            let mut values = HashMap::new();
            for row in rows {
                let (key, json) = row?;
                match serde_json::from_str(&json) {
                    Ok(value) => {
                        values.insert(key, value);
                    }
                    Err(error) => {
                        log::warn!("ignoring invalid stored setting {plugin_id}/{key}: {error}");
                    }
                }
            }
            Ok(values)
        })
    }

    pub fn plugin_setting_set(
        &self,
        plugin_id: &str,
        setting_key: &str,
        value: &serde_json::Value,
    ) -> Result<()> {
        let json = serde_json::to_string(value)?;
        self.with(|conn| {
            conn.execute(
                "INSERT INTO plugin_settings(plugin_id, setting_key, value_json)
                 VALUES (?1, ?2, ?3)
                 ON CONFLICT(plugin_id, setting_key) DO UPDATE SET value_json = excluded.value_json",
                rusqlite::params![plugin_id, setting_key, json],
            )?;
            Ok(())
        })
    }

    pub fn plugin_grant_get(&self, plugin_id: &str) -> Result<Option<PluginGrant>> {
        let row = self.with(|conn| {
            Ok(conn
                .query_row(
                    "SELECT permissions_json, network_domains_json, version, package_hash, granted_at
                     FROM plugin_grants WHERE plugin_id = ?1",
                    [plugin_id],
                    |row| {
                        Ok((
                            row.get::<_, String>(0)?,
                            row.get::<_, String>(1)?,
                            row.get::<_, String>(2)?,
                            row.get::<_, String>(3)?,
                            row.get::<_, i64>(4)?,
                        ))
                    },
                )
                .optional()?)
        })?;
        let Some((permissions_json, domains_json, version, package_hash, granted_at)) = row else {
            return Ok(None);
        };
        // A grant whose stored values no longer parse is treated as absent:
        // the plugin simply requires a fresh, explicit approval.
        let (permission_strings, domain_strings): (Vec<String>, Vec<String>) = match (
            serde_json::from_str(&permissions_json),
            serde_json::from_str(&domains_json),
        ) {
            (Ok(permissions), Ok(domains)) => (permissions, domains),
            _ => {
                log::warn!("ignoring invalid stored grant for {plugin_id}");
                return Ok(None);
            }
        };
        let (Ok(permissions), Ok(network_domains)) = (
            parse_permissions(&permission_strings),
            crate::plugins::parse_stored_network_domains(&domain_strings),
        ) else {
            log::warn!("ignoring invalid stored grant for {plugin_id}");
            return Ok(None);
        };
        Ok(Some(PluginGrant {
            plugin_id: plugin_id.to_string(),
            permissions,
            network_domains,
            version,
            package_hash,
            granted_at,
        }))
    }

    pub fn plugin_grant_set(&self, grant: &PluginGrant) -> Result<()> {
        let permissions: Vec<&str> = grant
            .permissions
            .iter()
            .map(|permission| permission.as_str())
            .collect();
        let permissions_json = serde_json::to_string(&permissions)?;
        let domains_json = serde_json::to_string(&grant.network_domains)?;
        self.with(|conn| {
            conn.execute(
                "INSERT INTO plugin_grants(
                   plugin_id, permissions_json, network_domains_json, version, package_hash, granted_at
                 ) VALUES (?1, ?2, ?3, ?4, ?5, ?6)
                 ON CONFLICT(plugin_id) DO UPDATE SET
                   permissions_json = excluded.permissions_json,
                   network_domains_json = excluded.network_domains_json,
                   version = excluded.version,
                   package_hash = excluded.package_hash,
                   granted_at = excluded.granted_at",
                rusqlite::params![
                    grant.plugin_id,
                    permissions_json,
                    domains_json,
                    grant.version,
                    grant.package_hash,
                    grant.granted_at,
                ],
            )?;
            Ok(())
        })
    }

    pub fn plugin_storage_get(&self, plugin_id: &str, key: &str) -> Result<Option<String>> {
        self.with(|conn| {
            Ok(conn
                .query_row(
                    "SELECT value FROM plugin_storage WHERE plugin_id = ?1 AND storage_key = ?2",
                    rusqlite::params![plugin_id, key],
                    |row| row.get(0),
                )
                .optional()?)
        })
    }

    pub fn plugin_storage_set(&self, plugin_id: &str, key: &str, value: &str) -> Result<()> {
        self.with(|conn| {
            conn.execute(
                "INSERT INTO plugin_storage(plugin_id, storage_key, value)
                 VALUES (?1, ?2, ?3)
                 ON CONFLICT(plugin_id, storage_key) DO UPDATE SET value = excluded.value",
                rusqlite::params![plugin_id, key, value],
            )?;
            Ok(())
        })
    }

    pub fn plugin_storage_delete(&self, plugin_id: &str, key: &str) -> Result<()> {
        self.with(|conn| {
            conn.execute(
                "DELETE FROM plugin_storage WHERE plugin_id = ?1 AND storage_key = ?2",
                rusqlite::params![plugin_id, key],
            )?;
            Ok(())
        })
    }

    pub fn plugin_storage_keys(&self, plugin_id: &str) -> Result<Vec<String>> {
        self.with(|conn| {
            let mut stmt = conn.prepare(
                "SELECT storage_key FROM plugin_storage WHERE plugin_id = ?1
                 ORDER BY storage_key",
            )?;
            let rows = stmt.query_map([plugin_id], |row| row.get(0))?;
            rows.collect::<rusqlite::Result<Vec<_>>>()
                .map_err(Into::into)
        })
    }

    /// Bytes currently used by one plugin's storage namespace (key + value
    /// bytes), for quota enforcement.
    pub fn plugin_storage_usage(&self, plugin_id: &str) -> Result<i64> {
        self.with(|conn| {
            Ok(conn.query_row(
                // Byte lengths, matching the Rust-side delta in wasm::storage.
                // Bare length() counts characters on TEXT, which would charge a
                // CJK-heavy value a third of what it costs.
                "SELECT COALESCE(SUM(length(CAST(storage_key AS BLOB))
                                   + length(CAST(value AS BLOB))), 0)
                 FROM plugin_storage WHERE plugin_id = ?1",
                [plugin_id],
                |row| row.get(0),
            )?)
        })
    }

    pub fn plugin_storage_clear(&self, plugin_id: &str) -> Result<()> {
        self.with(|conn| {
            conn.execute(
                "DELETE FROM plugin_storage WHERE plugin_id = ?1",
                [plugin_id],
            )?;
            Ok(())
        })
    }
}
