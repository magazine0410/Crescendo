use crescendo_core::db::Db;

#[test]
fn prune_removes_only_expired_rows() {
    let dir = tempfile::tempdir().unwrap();
    let db = Db::open(&dir.path().join("cache.db")).unwrap();
    db.web_cache_put("fresh", r#"{"artist":{}}"#).unwrap();
    db.web_cache_put("expired", r#"{"album":{}}"#).unwrap();
    db.with(|conn| {
        conn.execute(
            "UPDATE web_cache SET fetched_at = fetched_at - 7200 WHERE key = 'expired'",
            [],
        )?;
        Ok(())
    })
    .unwrap();

    db.web_cache_prune(3600).unwrap();
    let keys: Vec<String> = db
        .with(|conn| {
            let mut stmt = conn.prepare("SELECT key FROM web_cache ORDER BY key")?;
            let rows = stmt.query_map([], |row| row.get(0))?;
            rows.collect::<rusqlite::Result<_>>().map_err(Into::into)
        })
        .unwrap();
    assert_eq!(keys, ["fresh"]);
}
