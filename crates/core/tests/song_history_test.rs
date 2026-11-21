use crescendo_core::db::Db;
use crescendo_core::songlink::Service;

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
        let pending: i64 =
            c.query_row("SELECT COUNT(*) FROM listenbrainz_outbox", [], |r| r.get(0))?;
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
            c.execute_batch("DROP TABLE listenbrainz_outbox; PRAGMA user_version=10;
                INSERT INTO tracks(id,path,dir,filename,mtime,size,format) VALUES (1,'/a','/','a',0,0,'FLAC');")?;
            Ok(())
        }).unwrap();
        db.record_play_at(1, 1700000000).unwrap();
    }
    let db = Db::open(&path).unwrap();
    assert_eq!(db.song_history(None, 50).unwrap()[0].played_at, 1700000000);
    db.with(|c| {
        assert_eq!(
            c.query_row("SELECT COUNT(*) FROM listenbrainz_outbox", [], |r| r
                .get::<_, i64>(0))?,
            0
        );
        Ok(())
    })
    .unwrap();
}

/// What a database from before ListenBrainz holds: a track page Last.fm's API
/// returned, cached; a scrobble it accepted; and one it skipped because
/// Last.fm had no page for the song.
#[test]
fn last_fm_pages_are_kept_for_plays_from_before_the_switch() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("history.db");
    let track = |artist: &str, title: &str| {
        format!(
            r#"{{"id":0,"path":"/a","title":"{title}","artist":"{artist}","album":null,
                "albumartist":null,"titlesort":null,"artistsort":null,"albumartistsort":null,
                "genre":null,"grouping":null,"composer":null,"arranger":null,"lyricist":null,
                "vocalist":null,"catalognumber":null,"event":null,"bpm":null,"track_no":null,
                "disc_no":null,"year":null,"duration_ms":1,"format":"FLAC","album_id":null,
                "rating":null,"play_count":0,"missing":false}}"#
        )
    };
    {
        let db = Db::open(&path).unwrap();
        db.with(|c| {
            c.execute_batch(
                "DROP TABLE lastfm_links; DROP TABLE web_cache; DROP TABLE listenbrainz_outbox;
                 DELETE FROM meta WHERE key = 'listenbrainz_since';
                 PRAGMA user_version = 16;
                 INSERT INTO tracks(id,path,dir,filename,mtime,size,format,title,artist) VALUES
                   (1,'/1','/','1',0,0,'FLAC','夜の歌','歌手'),
                   (2,'/2','/','2',0,0,'FLAC','Bird in a cage','NAGISA'),
                   (3,'/3','/','3',0,0,'FLAC','Nowhere','Nobody'),
                   (4,'/4','/','4',0,0,'FLAC',NULL,NULL);
                 INSERT INTO lastfm_cache(key, json, fetched_at) VALUES
                   ('track:[\"歌手\",\"夜の歌\"]', '\"https://www.last.fm/music/Kashu/_/Yoru\"', 1),
                   ('track:[\"Nobody\",\"Nowhere\"]', 'null', 1),
                   ('track:[\"Evil\",\"Link\"]', '\"https://evil.test/music/x\"', 1),
                   ('artist\u{1f}歌手', '{\"artist\":{}}', 1);",
            )?;
            c.execute(
                "INSERT INTO lastfm_outbox(account, track_json, started_at, state) VALUES
                   ('alice', ?1, 1, 'submitted'), ('alice', ?2, 2, 'submitted'),
                   ('alice', ?3, 3, 'skipped'), ('alice', 'not a track', 4, 'submitted')",
                [
                    track("歌手", "夜の歌"),
                    track("NAGISA", "Bird in a cage"),
                    track("Nobody", "Nowhere"),
                ],
            )?;
            Ok(())
        })
        .unwrap();
        for id in 1..=4 {
            db.record_play_at(id, 1_700_000_000 + id).unwrap();
        }
    }
    let db = Db::open(&path).unwrap();
    // A play made after the switch, of a song Last.fm had a page for.
    let later = 4_000_000_000;
    db.record_play_at(1, later).unwrap();
    let rows = db.song_history(None, 50).unwrap();
    let link = |played_at: i64| {
        let row = rows.iter().find(|r| r.played_at == played_at).unwrap();
        (row.service, row.url.as_deref())
    };
    assert_eq!(link(later), (Service::Listenbrainz, None));
    // The address Last.fm returned wins over one rebuilt from the tags.
    assert_eq!(
        link(1_700_000_001),
        (
            Service::Lastfm,
            Some("https://www.last.fm/music/Kashu/_/Yoru")
        )
    );
    // Scrobbled, so Last.fm has the page, at the address the tags give.
    assert_eq!(
        link(1_700_000_002),
        (
            Service::Lastfm,
            Some("https://www.last.fm/music/NAGISA/_/Bird+in+a+cage")
        )
    );
    // Not on Last.fm, and a track without tags: no page.
    assert_eq!(link(1_700_000_003), (Service::Lastfm, None));
    assert_eq!(link(1_700_000_004), (Service::Lastfm, None));
    // The Last.fm tables are left as they were.
    db.with(|c| {
        let (cache, outbox): (i64, i64) = c.query_row(
            "SELECT (SELECT COUNT(*) FROM lastfm_cache), (SELECT COUNT(*) FROM lastfm_outbox)",
            [],
            |r| Ok((r.get(0)?, r.get(1)?)),
        )?;
        assert_eq!((cache, outbox), (4, 4));
        Ok(())
    })
    .unwrap();
}

/// A new library has no Last.fm plays: everything is a ListenBrainz play.
#[test]
fn a_new_library_starts_on_listenbrainz() {
    let dir = tempfile::tempdir().unwrap();
    let db = Db::open(&dir.path().join("history.db")).unwrap();
    db.with(|c| {
        c.execute(
            "INSERT INTO tracks(id,path,dir,filename,mtime,size,format) VALUES (1,'/a','/','a',0,0,'FLAC')",
            [],
        )?;
        Ok(())
    })
    .unwrap();
    db.record_play_at(1, 4_000_000_000).unwrap();
    assert_eq!(
        db.song_history(None, 1).unwrap()[0].service,
        Service::Listenbrainz
    );
}
