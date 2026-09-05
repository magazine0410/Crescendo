use crescendo_core::db::Db;

#[test]
fn history_is_paged_stably_and_independent_of_scrobbling() {
    let dir = tempfile::tempdir().unwrap();
    let db = Db::open(&dir.path().join("history.db")).unwrap();
    db.with(|c| {
        c.execute("INSERT INTO tracks(id,path,dir,filename,mtime,size,format,title,artist,albumartist,titlesort,albumartistsort) VALUES (1,'/a.flac','/','a.flac',0,0,'FLAC','歌','歌手','アルバム歌手','Uta','Album Kashu')", [])?;
        Ok(())
    }).unwrap();
    for i in 0..120 {
        db.record_play_at(1, 1700000000 + i).unwrap();
    }
    let page = db.song_history(None, 50).unwrap();
    assert_eq!(page.len(), 50);
    assert_eq!(page[0].played_at, 1700000119);
    assert_eq!(page[0].track.albumartist.as_deref(), Some("アルバム歌手"));
    assert_eq!(page[0].track.titlesort.as_deref(), Some("Uta"));
    let before = page.last().unwrap().id;
    db.record_play_at(1, 1700000999).unwrap();
    let next = db.song_history(Some(before), 50).unwrap();
    assert_eq!(next.len(), 50);
    assert_eq!(next[0].played_at, 1700000069);
    assert!(next.iter().all(|r| r.id < before));
    assert_eq!(
        db.song_history(Some(next.last().unwrap().id), 50)
            .unwrap()
            .len(),
        20
    );
    assert_eq!(db.song_history(None, usize::MAX).unwrap().len(), 100);
    assert_eq!(db.track_by_id(1).unwrap().unwrap().play_count, 121);
    db.with(|c| {
        let pending: i64 = c.query_row("SELECT COUNT(*) FROM lastfm_outbox", [], |r| r.get(0))?;
        assert_eq!(pending, 0);
        let (cached, history): (i64, i64) = c.query_row(
            "SELECT last_played, (SELECT MAX(played_at) FROM play_history) FROM tracks WHERE id=1",
            [],
            |r| Ok((r.get(0)?, r.get(1)?)),
        )?;
        assert_eq!(cached, history);
        Ok(())
    })
    .unwrap();
}

#[test]
fn migration_preserves_existing_history() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("history.db");
    {
        let db = Db::open(&path).unwrap();
        db.with(|c| {
            c.execute_batch("DROP TABLE lastfm_outbox; PRAGMA user_version=10;
                INSERT INTO tracks(id,path,dir,filename,mtime,size,format) VALUES (1,'/a','/','a',0,0,'FLAC');")?;
            Ok(())
        }).unwrap();
        db.record_play_at(1, 1700000000).unwrap();
    }
    let db = Db::open(&path).unwrap();
    assert_eq!(db.song_history(None, 50).unwrap()[0].played_at, 1700000000);
    db.with(|c| {
        assert_eq!(
            c.query_row("SELECT COUNT(*) FROM lastfm_outbox", [], |r| r
                .get::<_, i64>(0))?,
            0
        );
        Ok(())
    })
    .unwrap();
}
