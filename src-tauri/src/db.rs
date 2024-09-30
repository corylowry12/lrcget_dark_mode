use rusqlite::{Connection, named_params, params};
use tauri::AppHandle;
use std::fs;
use anyhow::Result;
use indoc::indoc;
use crate::persistent_entities::{PersistentTrack, PersistentAlbum, PersistentArtist, PersistentConfig};
use crate::fs_track;
use crate::utils::prepare_input;
use regex::Regex;

const CURRENT_DB_VERSION: u32 = 4;

/// Initializes the database connection, creating the .sqlite file if needed, and upgrading the database
/// if it's out of date.
pub fn initialize_database(app_handle: &AppHandle) -> Result<Connection, rusqlite::Error> {
  let app_dir = app_handle.path_resolver().app_data_dir().expect("The app data directory should exist.");
  fs::create_dir_all(&app_dir).expect("The app data directory should be created.");
  let sqlite_path = app_dir.join("db.sqlite3");

  println!("Database file path: {}", sqlite_path.display());

  let mut db = Connection::open(sqlite_path)?;

  let mut user_pragma = db.prepare("PRAGMA user_version")?;
  let existing_user_version: u32 = user_pragma.query_row([], |row| { Ok(row.get(0)?) })?;
  drop(user_pragma);

  upgrade_database_if_needed(&mut db, existing_user_version)?;

  Ok(db)
}

/// Upgrades the database to the current version.
pub fn upgrade_database_if_needed(db: &mut Connection, existing_version: u32) -> Result<(), rusqlite::Error> {
  println!("Existing database version: {}", existing_version);

  if existing_version < CURRENT_DB_VERSION {
    if existing_version <= 0 {
      println!("Mirgate database version 1...");
      db.pragma_update(None, "journal_mode", "WAL")?;

      let tx = db.transaction()?;

      tx.pragma_update(None, "user_version", 1)?;

      tx.execute_batch(indoc! {"
      CREATE TABLE directories (
        id INTEGER PRIMARY KEY,
        path TEXT
      );

      CREATE TABLE library_data (
        id INTEGER PRIMARY KEY,
        init BOOLEAN
      );

      CREATE TABLE config_data (
        id INTEGER PRIMARY KEY,
        skip_not_needed_tracks BOOLEAN,
        try_embed_lyrics BOOLEAN
      );

      CREATE TABLE artists (
        id INTEGER PRIMARY KEY,
        name TEXT
      );

      CREATE TABLE albums (
        id INTEGER PRIMARY KEY,
        name TEXT,
        artist_id INTEGER,
        image_path TEXT,
        FOREIGN KEY(artist_id) REFERENCES artists(id)
      );

      CREATE TABLE tracks (
        id INTEGER PRIMARY KEY,
        file_path TEXT,
        file_name TEXT,
        title TEXT,
        album_id INTEGER,
        artist_id INTEGER,
        duration FLOAT,
        lrc_lyrics TEXT,
        FOREIGN KEY(artist_id) REFERENCES artists(id),
        FOREIGN KEY(album_id) REFERENCES albums(id)
      );

      INSERT INTO library_data (init) VALUES (0);
      INSERT INTO config_data (skip_not_needed_tracks, try_embed_lyrics) VALUES (1, 0);
      "})?;

      tx.commit()?;
    }

    if existing_version <= 1 {
      println!("Mirgate database version 2...");
      db.pragma_update(None, "journal_mode", "WAL")?;

      let tx = db.transaction()?;

      tx.pragma_update(None, "user_version", 2)?;

      tx.execute_batch(indoc! {"
      ALTER TABLE tracks ADD txt_lyrics TEXT;
      CREATE INDEX idx_tracks_title ON tracks(title);
      CREATE INDEX idx_albums_name ON albums(name);
      CREATE INDEX idx_artists_name ON artists(name);
      "})?;
      tx.commit()?;
    }

    if existing_version <= 2 {
      println!("Mirgate database version 3...");
      let tx = db.transaction()?;

      tx.pragma_update(None, "user_version", 3)?;

      tx.execute_batch(indoc! {"
      ALTER TABLE tracks ADD instrumental BOOLEAN;
      "})?;
      tx.commit()?;
    }

    if existing_version <= 3 {
      println!("Mirgate database version 4...");
      let tx = db.transaction()?;

      tx.pragma_update(None, "user_version", 4)?;

      tx.execute_batch(indoc! {"
      ALTER TABLE tracks ADD title_lower TEXT;
      ALTER TABLE albums ADD name_lower TEXT;
      ALTER TABLE artists ADD name_lower TEXT;
      CREATE INDEX idx_tracks_title_lower ON tracks(title_lower);
      CREATE INDEX idx_albums_name_lower ON albums(name_lower);
      CREATE INDEX idx_artists_name_lower ON artists(name_lower);
      "})?;

      // Update all tracks with the lower case title
      {
        let mut stmt = tx.prepare("SELECT id, title FROM tracks")?;
        let tracks: Vec<(i64, String)> = stmt.query_map([], |row| {
          Ok((row.get(0)?, row.get(1)?))
        })?.collect::<Result<Vec<_>, rusqlite::Error>>()?;
        for (id, title) in tracks {
          let title_lower = prepare_input(&title);
          tx.execute("UPDATE tracks SET title_lower = ? WHERE id = ?", (&title_lower, &id))?;
        }
      }

      // Update all albums with the lower case name
      {
        let mut stmt = tx.prepare("SELECT id, name FROM albums")?;
        let albums: Vec<(i64, String)> = stmt.query_map([], |row| {
          Ok((row.get(0)?, row.get(1)?))
        })?.collect::<Result<Vec<_>, rusqlite::Error>>()?;
        for (id, name) in albums {
          let name_lower = prepare_input(&name);
          tx.execute("UPDATE albums SET name_lower = ? WHERE id = ?", (&name_lower, &id))?;
        }
      }

      // Update all artists with the lower case name
      {
        let mut stmt = tx.prepare("SELECT id, name FROM artists")?;
        let artists: Vec<(i64, String)> = stmt.query_map([], |row| {
          Ok((row.get(0)?, row.get(1)?))
        })?.collect::<Result<Vec<_>, rusqlite::Error>>()?;
        for (id, name) in artists {
          let name_lower = prepare_input(&name);
          tx.execute("UPDATE artists SET name_lower = ? WHERE id = ?", (&name_lower, &id))?;
        }
      }

      tx.commit()?;
    }
  }

  Ok(())
}

pub fn get_directories(db: &Connection) -> Result<Vec<String>> {
  let mut statement = db.prepare("SELECT * FROM directories")?;
  let mut rows = statement.query([])?;
  let mut directories: Vec<String> = Vec::new();
  while let Some(row) = rows.next()? {
    let path: String = row.get("path")?;

    directories.push(path);
  }

  Ok(directories)
}

pub fn set_directories(directories: Vec<String>, db: &Connection) -> Result<()> {
  db.execute("DELETE FROM directories WHERE 1", ())?;
  let mut statement = db.prepare("INSERT INTO directories (path) VALUES (@path)")?;
  for directory in directories.iter() {
    statement.execute(named_params! { "@path": directory })?;
  }

  Ok(())
}

pub fn get_init(db: &Connection) -> Result<bool> {
  let mut statement = db.prepare("SELECT init FROM library_data LIMIT 1")?;
  let init: bool = statement.query_row([], |r| r.get(0))?;
  Ok(init)
}

pub fn set_init(init: bool, db: &Connection) -> Result<()> {
  let mut statement = db.prepare("UPDATE library_data SET init = ? WHERE 1")?;
  statement.execute([init])?;
  Ok(())
}

pub fn get_config(db: &Connection) -> Result<PersistentConfig> {
  let mut statement = db.prepare("SELECT skip_not_needed_tracks, try_embed_lyrics FROM config_data LIMIT 1")?;
  let row = statement.query_row([], |r| {
    Ok(PersistentConfig {
      skip_not_needed_tracks: r.get("skip_not_needed_tracks")?,
      try_embed_lyrics: r.get("try_embed_lyrics")?
    })
  })?;
  Ok(row)
}

pub fn set_config(skip_not_needed_tracks: bool, try_embed_lyrics: bool, db: &Connection) -> Result<()> {
  let mut statement = db.prepare("UPDATE config_data SET skip_not_needed_tracks = ?, try_embed_lyrics = ? WHERE 1")?;
  statement.execute([skip_not_needed_tracks, try_embed_lyrics])?;
  Ok(())
}

pub fn find_artist(name: &str, db: &Connection) -> Result<i64> {
  let mut statement = db.prepare("SELECT id FROM artists WHERE name = ?")?;
  let id: i64 = statement.query_row([name], |r| r.get(0))?;
  Ok(id)
}

pub fn add_artist(name: &str, db: &Connection) -> Result<i64> {
  let mut statement = db.prepare("INSERT INTO artists (name, name_lower) VALUES (?, ?)")?;
  let row_id = statement.insert((name, prepare_input(name)))?;
  Ok(row_id)
}

pub fn find_album(name: &str, artist_id: i64, db: &Connection) -> Result<i64> {
  let mut statement = db.prepare("SELECT id FROM albums WHERE name = ? AND artist_id = ?")?;
  let id: i64 = statement.query_row((name, artist_id), |r| r.get(0))?;
  Ok(id)
}

pub fn add_album(name: &str, artist_id: i64, db: &Connection) -> Result<i64> {
  let mut statement = db.prepare("INSERT INTO albums (name, name_lower, artist_id) VALUES (?, ?, ?)")?;
  let row_id = statement.insert((name, prepare_input(name), artist_id))?;
  Ok(row_id)
}

pub fn get_track_by_id(id: i64, db: &Connection) -> Result<PersistentTrack> {
  let query = indoc! {"
    SELECT
        tracks.id,
        file_path,
        file_name,
        title,
        artists.name AS artist_name,
        tracks.artist_id,
        albums.name AS album_name,
        album_id,
        duration,
        albums.image_path,
        txt_lyrics,
        lrc_lyrics,
        instrumental
    FROM tracks
    JOIN albums ON tracks.album_id = albums.id
    JOIN artists ON tracks.artist_id = artists.id
    WHERE tracks.id = ?
    LIMIT 1
  "};

  let mut statement = db.prepare(query)?;
  let row = statement.query_row(
    [id],
    |row| {
      let is_instrumental: Option<bool> = row.get("instrumental")?;

      Ok(PersistentTrack {
        id: row.get("id")?,
        file_path: row.get("file_path")?,
        file_name: row.get("file_name")?,
        title: row.get("title")?,
        artist_name: row.get("artist_name")?,
        artist_id: row.get("artist_id")?,
        album_name: row.get("album_name")?,
        album_id: row.get("album_id")?,
        duration: row.get("duration")?,
        txt_lyrics: row.get("txt_lyrics")?,
        lrc_lyrics: row.get("lrc_lyrics")?,
        image_path: row.get("image_path")?,
        instrumental: is_instrumental.unwrap_or(false)
      })
    })?;
  Ok(row)
}

pub fn update_track_synced_lyrics(id: i64, synced_lyrics: &str, plain_lyrics: &str, db: &Connection) -> Result<PersistentTrack> {
  let mut statement = db.prepare("UPDATE tracks SET lrc_lyrics = ?, txt_lyrics = ?, instrumental = false WHERE id = ?")?;
  statement.execute((synced_lyrics, plain_lyrics, id))?;

  Ok(get_track_by_id(id, db)?)
}

pub fn update_track_plain_lyrics(id: i64, plain_lyrics: &str, db: &Connection) -> Result<PersistentTrack> {
  let mut statement = db.prepare("UPDATE tracks SET txt_lyrics = ?, lrc_lyrics = null, instrumental = false WHERE id = ?")?;
  statement.execute((plain_lyrics, id))?;

  Ok(get_track_by_id(id, db)?)
}

pub fn update_track_null_lyrics(id: i64, db: &Connection) -> Result<PersistentTrack> {
  let mut statement = db.prepare("UPDATE tracks SET txt_lyrics = null, lrc_lyrics = null, instrumental = false WHERE id = ?")?;
  statement.execute([id])?;

  Ok(get_track_by_id(id, db)?)
}

pub fn update_track_instrumental(id: i64, db: &Connection) -> Result<PersistentTrack> {
  let mut statement = db.prepare("UPDATE tracks SET txt_lyrics = null, lrc_lyrics = ?, instrumental = true WHERE id = ?")?;
  statement.execute(params!["[au: instrumental]", id])?;

  Ok(get_track_by_id(id, db)?)
}

pub fn add_tracks(tracks: &Vec<fs_track::FsTrack>, db: &mut Connection) -> Result<()> {
  let tx = db.transaction()?;

  for track in tracks.iter() {
    add_track(track, &tx)?;
  }

  tx.commit()?;

  Ok(())
}

pub fn add_track(track: &fs_track::FsTrack, db: &Connection) -> Result<()> {
  let artist_result = find_artist(&track.artist(), db);
  let artist_id = match artist_result {
    Ok(artist_id) => artist_id,
    Err(_) => {
      add_artist(&track.artist(), db)?
    }
  };

  let album_result = find_album(&track.album(), artist_id, db);
  let album_id = match album_result {
    Ok(album_id) => album_id,
    Err(_) => {
      add_album(&track.album(), artist_id, db)?
    }
  };

  // Create a regex to match "[au: instrumental]" or "[au:instrumental]"
  let re = Regex::new(r"\[au:\s*instrumental\]").expect("Invalid regex");
  let is_instrumental = track.lrc_lyrics().as_ref().map_or(false, |lyrics| re.is_match(lyrics));

  let query = indoc! {"
    INSERT INTO tracks (
        file_path,
        file_name,
        title,
        title_lower,
        album_id,
        artist_id,
        duration,
        txt_lyrics,
        lrc_lyrics,
        instrumental
    ) VALUES (?, ?, ?, ?, ?, ?, ?, ?, ?, ?)
  "};
  let mut statement = db.prepare(query)?;
  statement.execute((
    track.file_path(),
    track.file_name(),
    track.title(),
    prepare_input(&track.title()),
    album_id,
    artist_id,
    track.duration(),
    track.txt_lyrics(),
    track.lrc_lyrics(),
    is_instrumental
  ))?;

  Ok(())
}

pub fn get_tracks(db: &Connection) -> Result<Vec<PersistentTrack>> {
  let query = indoc! {"
      SELECT
          tracks.id, file_path, file_name, title,
          artists.name AS artist_name, tracks.artist_id,
          albums.name AS album_name, album_id, duration,
          albums.image_path, txt_lyrics, lrc_lyrics, instrumental
      FROM tracks
      JOIN albums ON tracks.album_id = albums.id
      JOIN artists ON tracks.artist_id = artists.id
      ORDER BY title_lower ASC
  "};
  let mut statement = db.prepare(query)?;
  let mut rows = statement.query([])?;
  let mut tracks: Vec<PersistentTrack> = Vec::new();

  while let Some(row) = rows.next()? {
    let is_instrumental: Option<bool> = row.get("instrumental")?;

    let track = PersistentTrack {
      id: row.get("id")?,
      file_path: row.get("file_path")?,
      file_name: row.get("file_name")?,
      title: row.get("title")?,
      artist_name: row.get("artist_name")?,
      artist_id: row.get("artist_id")?,
      album_name: row.get("album_name")?,
      album_id: row.get("album_id")?,
      duration: row.get("duration")?,
      txt_lyrics: row.get("txt_lyrics")?,
      lrc_lyrics: row.get("lrc_lyrics")?,
      image_path: row.get("image_path")?,
      instrumental: is_instrumental.unwrap_or(false)
    };

    tracks.push(track);
  }

  Ok(tracks)
}

pub fn get_track_ids(db: &Connection) -> Result<Vec<i64>> {
  let mut statement = db.prepare("SELECT id FROM tracks ORDER BY title_lower ASC")?;
  let mut rows = statement.query([])?;
  let mut track_ids: Vec<i64> = Vec::new();

  while let Some(row) = rows.next()? {
    track_ids.push(row.get("id")?);
  }

  Ok(track_ids)
}

pub fn get_search_track_ids(query_str: &String, db: &Connection) -> Result<Vec<i64>> {
    let query = indoc! {"
      SELECT tracks.id
      FROM tracks
      JOIN artists ON tracks.artist_id = artists.id
      JOIN albums ON tracks.album_id = albums.id
      WHERE artists.name_lower LIKE ?
      OR albums.name_lower LIKE ?
      OR tracks.title_lower LIKE ?
      ORDER BY title_lower ASC
  "};

    let mut statement = db.prepare(query)?;
    let formatted_query_str = format!("%{}%", prepare_input(query_str));
    let mut rows = statement.query(params![
        formatted_query_str,
        formatted_query_str,
        formatted_query_str
    ])?;
    let mut track_ids: Vec<i64> = Vec::new();

    while let Some(row) = rows.next()? {
        track_ids.push(row.get("id")?);
    }

    Ok(track_ids)
}

pub fn get_no_lyrics_track_ids(db: &Connection) -> Result<Vec<i64>> {
  let mut statement = db.prepare("SELECT id FROM tracks WHERE lrc_lyrics IS NULL AND instrumental != true ORDER BY title_lower ASC")?;
  let mut rows = statement.query([])?;
  let mut track_ids: Vec<i64> = Vec::new();

  while let Some(row) = rows.next()? {
    track_ids.push(row.get("id")?);
  }

  Ok(track_ids)
}

pub fn get_albums(db: &Connection) -> Result<Vec<PersistentAlbum>> {
  let mut statement = db.prepare(indoc! {"
      SELECT albums.id, albums.name, artists.name AS artist_name, albums.artist_id,
            COUNT(tracks.id) AS tracks_count, image_path
      FROM albums
      JOIN artists ON artists.id = albums.artist_id
      JOIN tracks ON tracks.album_id = albums.id
      GROUP BY albums.id, albums.name, artist_name, albums.artist_id, image_path
      ORDER BY albums.name_lower ASC
  "})?;
  let mut rows = statement.query([])?;
  let mut albums: Vec<PersistentAlbum> = Vec::new();

  while let Some(row) = rows.next()? {
    let album = PersistentAlbum {
      id: row.get("id")?,
      name: row.get("name")?,
      image_path: row.get("image_path")?,
      artist_name: row.get("artist_name")?,
      artist_id: row.get("artist_id")?,
      tracks_count: row.get("tracks_count")?,
    };

    albums.push(album);
  }

  Ok(albums)
}

pub fn get_album_by_id(id: i64, db: &Connection) -> Result<PersistentAlbum> {
  let mut statement = db.prepare(indoc! {"
      SELECT
          albums.id,
          albums.name,
          artists.name AS artist_name,
          albums.artist_id,
          COUNT(tracks.id) AS tracks_count,
          image_path
      FROM albums
      JOIN artists ON artists.id = albums.artist_id
      JOIN tracks ON tracks.album_id = albums.id
      WHERE albums.id = ?
      GROUP BY
          albums.id,
          albums.name,
          artist_name,
          albums.artist_id,
          image_path
      LIMIT 1
  "})?;
  let row = statement.query_row(
    [id],
    |row|
    Ok(PersistentAlbum {
      id: row.get("id")?,
      name: row.get("name")?,
      image_path: row.get("image_path")?,
      artist_name: row.get("artist_name")?,
      artist_id: row.get("artist_id")?,
      tracks_count: row.get("tracks_count")?,
    })
  )?;
  Ok(row)
}

pub fn get_album_ids(db: &Connection) -> Result<Vec<i64>> {
  let mut statement = db.prepare("SELECT id FROM albums ORDER BY name_lower ASC")?;
  let mut rows = statement.query([])?;
  let mut album_ids: Vec<i64> = Vec::new();

  while let Some(row) = rows.next()? {
    album_ids.push(row.get("id")?);
  }

  Ok(album_ids)
}

pub fn get_artists(db: &Connection) -> Result<Vec<PersistentArtist>> {
  let mut statement = db.prepare(indoc! {"
      SELECT artists.id, artists.name AS name, COUNT(tracks.id) AS tracks_count
      FROM artists
      JOIN tracks ON tracks.artist_id = artists.id
      GROUP BY artists.id, artists.name
      ORDER BY artists.name_lower ASC
  "})?;
  let mut rows = statement.query([])?;
  let mut artists: Vec<PersistentArtist> = Vec::new();

  while let Some(row) = rows.next()? {
    let artist = PersistentArtist {
      id: row.get("id")?,
      name: row.get("name")?,
      // albums_count: row.get("albums_count")?,
      tracks_count: row.get("tracks_count")?,
    };

    artists.push(artist);
  }

  Ok(artists)
}

pub fn get_artist_by_id(id: i64, db: &Connection) -> Result<PersistentArtist> {
  let mut statement = db.prepare(indoc! {"
      SELECT artists.id,
            artists.name AS name,
            COUNT(tracks.id) AS tracks_count
      FROM artists
      JOIN tracks ON tracks.artist_id = artists.id
      WHERE artists.id = ?
      GROUP BY artists.id, artists.name
      LIMIT 1
  "})?;
  let row = statement.query_row(
    [id],
    |row|
    Ok(PersistentArtist {
      id: row.get("id")?,
      name: row.get("name")?,
      // albums_count: row.get("albums_count")?,
      tracks_count: row.get("tracks_count")?,
    })
  )?;
  Ok(row)
}

pub fn get_artist_ids(db: &Connection) -> Result<Vec<i64>> {
  let mut statement = db.prepare("SELECT id FROM artists ORDER BY name_lower ASC")?;
  let mut rows = statement.query([])?;
  let mut artist_ids: Vec<i64> = Vec::new();

  while let Some(row) = rows.next()? {
    artist_ids.push(row.get("id")?);
  }

  Ok(artist_ids)
}

pub fn get_album_tracks(album_id: i64, db: &Connection) -> Result<Vec<PersistentTrack>> {
  let mut statement = db.prepare(indoc! {"
      SELECT
          tracks.id,
          file_path,
          file_name,
          title,
          artists.name AS artist_name,
          tracks.artist_id,
          albums.name AS album_name,
          album_id,
          duration,
          albums.image_path,
          txt_lyrics,
          lrc_lyrics,
          instrumental
      FROM tracks
      JOIN albums ON tracks.album_id = albums.id
      JOIN artists ON tracks.artist_id = artists.id
      WHERE tracks.album_id = ?
      ORDER BY title_lower ASC
  "})?;
  let mut rows = statement.query([album_id])?;
  let mut tracks: Vec<PersistentTrack> = Vec::new();

  while let Some(row) = rows.next()? {
    let is_instrumental: Option<bool> = row.get("instrumental")?;

    let track = PersistentTrack {
      id: row.get("id")?,
      file_path: row.get("file_path")?,
      file_name: row.get("file_name")?,
      title: row.get("title")?,
      artist_name: row.get("artist_name")?,
      artist_id: row.get("artist_id")?,
      album_name: row.get("album_name")?,
      album_id: row.get("album_id")?,
      duration: row.get("duration")?,
      txt_lyrics: row.get("txt_lyrics")?,
      lrc_lyrics: row.get("lrc_lyrics")?,
      image_path: row.get("image_path")?,
      instrumental: is_instrumental.unwrap_or(false)
    };

    tracks.push(track);
  }

  Ok(tracks)
}

pub fn get_album_track_ids(album_id: i64, db: &Connection) -> Result<Vec<i64>> {
  let mut statement = db.prepare(indoc! {"
      SELECT tracks.id
      FROM tracks
      JOIN albums ON tracks.album_id = albums.id
      WHERE tracks.album_id = ?
      ORDER BY title_lower ASC
  "})?;
  let mut rows = statement.query([album_id])?;
  let mut tracks: Vec<i64> = Vec::new();

  while let Some(row) = rows.next()? {
    tracks.push(row.get("id")?);
  }

  Ok(tracks)
}

pub fn get_artist_tracks(artist_id: i64, db: &Connection) -> Result<Vec<PersistentTrack>> {
  let mut statement = db.prepare(indoc! {"
      SELECT tracks.id, file_path, file_name, title, artists.name AS artist_name,
             tracks.artist_id, albums.name AS album_name, album_id, duration,
             albums.image_path, txt_lyrics, lrc_lyrics, instrumental
      FROM tracks
      JOIN albums ON tracks.album_id = albums.id
      JOIN artists ON tracks.artist_id = artists.id
      WHERE tracks.artist_id = ?
      ORDER BY title_lower ASC
  "})?;
  let mut rows = statement.query([artist_id])?;
  let mut tracks: Vec<PersistentTrack> = Vec::new();

  while let Some(row) = rows.next()? {
    let is_instrumental: Option<bool> = row.get("instrumental")?;

    let track = PersistentTrack {
      id: row.get("id")?,
      file_path: row.get("file_path")?,
      file_name: row.get("file_name")?,
      title: row.get("title")?,
      artist_name: row.get("artist_name")?,
      artist_id: row.get("artist_id")?,
      album_name: row.get("album_name")?,
      album_id: row.get("album_id")?,
      duration: row.get("duration")?,
      txt_lyrics: row.get("txt_lyrics")?,
      lrc_lyrics: row.get("lrc_lyrics")?,
      image_path: row.get("image_path")?,
      instrumental: is_instrumental.unwrap_or(false)
    };

    tracks.push(track);
  }

  Ok(tracks)
}

pub fn get_artist_track_ids(artist_id: i64, db: &Connection) -> Result<Vec<i64>> {
  let mut statement = db.prepare(indoc! {"
      SELECT tracks.id
      FROM tracks
      JOIN artists ON tracks.artist_id = artists.id
      WHERE tracks.artist_id = ?
      ORDER BY title_lower ASC
  "})?;
  let mut rows = statement.query([artist_id])?;
  let mut tracks: Vec<i64> = Vec::new();

  while let Some(row) = rows.next()? {
    tracks.push(row.get("id")?);
  }

  Ok(tracks)
}

pub fn clean_library(db: &Connection) -> Result<()> {
  db.execute("DELETE FROM tracks WHERE 1", ())?;
  db.execute("DELETE FROM albums WHERE 1", ())?;
  db.execute("DELETE FROM artists WHERE 1", ())?;
  Ok(())
}
