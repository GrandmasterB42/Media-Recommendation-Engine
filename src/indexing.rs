use std::{
    ffi::OsStr,
    path::{Path, PathBuf},
    rc::Rc,
    time::Duration,
};

use rusqlite::{functions::FunctionFlags, params, types::Value, Connection, OptionalExtension};
use tracing::{debug, info, warn};

use crate::{
    database::{Database, DatabaseResult},
    utils::HandleErr,
};

pub async fn periodic_indexing(db: Database) -> ! {
    loop {
        // TODO: Handle this error better?
        indexing(&db).log_err_with_msg("Failed the indexing");
        // TODO: Setting for this duration?
        tokio::time::sleep(Duration::from_secs(60 * 5)).await;
    }
}
//TODO: Indexing is quite slow, try reducing the amount of database queries/do more stuff in the database if possible
fn indexing(db: &Database) -> DatabaseResult<()> {
    let locations: Vec<String> = db.run(|conn| {
        let mut stmt = conn.prepare("SELECT path FROM storage_locations")?;
        let paths = stmt
            .query_map([], |row| row.get(0))?
            .collect::<Result<Vec<_>, _>>()?;
        Ok(paths)
    })?;

    let filesystem = locations
        .into_iter()
        .map(PathBuf::from)
        .flat_map(scan_dir)
        .collect::<Vec<_>>();

    let database_registered: Vec<PathBuf> = db.run(|conn| {
        conn.prepare("SELECT path from data_files")?
            .query_map([], |row| Ok(PathBuf::from(row.get::<usize, String>(0)?)))?
            .collect()
    })?;

    // Note: There is probably a faster? way to do these two loops, but this is good enough for now
    // TODO: Also track changes and handle that
    let conn = db.get()?;

    let mut insertion_stmt =
        conn.prepare("INSERT INTO data_files (path) VALUES (?1) RETURNING id")?;
    for file in &filesystem {
        if !database_registered.contains(file) {
            let id = insertion_stmt.query_row([path_to_db(file)], |row| row.get(0))?;
            classify_new_file(file, id, db)?;
        }
    }

    // TODO: Removals will probably have to do more than just remove the data_file
    // Important: Don't delete any directly user facing info, is still important for recommendation, maybe consider deletion when recommending?
    for file in &database_registered {
        if !filesystem.contains(file) {
            debug!("Want to remove {file:?}")
        }
    }

    info!("Finished indexing once");
    Ok(())
}

// TODO: Rewrite + Consider Franchise, theme, ...
// TODO: The updating of names doesn't feel right, look into that during the rewrite
// NOTE: Something about the first episode of season 0 doesnt delete it's name properly?
// Honestly the removing should just be a post-processing step, so it doesn't have per-file overhead and sets stuff to the most common name properly
/// This tries to insert the file into the database as best as possible
fn classify_new_file(path: &Path, id: u64, db: &Database) -> DatabaseResult<()> {
    let filetype = file_type(path);
    if filetype.is_none() {
        warn!("failed to decode extension for {path:?}, aborting evaluation");
        return Ok(());
    }

    match filetype.unwrap() {
        FileType::Unknown => {
            warn!("cannot work with the filetype of {path:?}, not adding to database");
            return Ok(());
        }
        FileType::Audio => {
            if os_str_conversion(path.file_name().unwrap()) == "theme.mp3" {
                debug!("trying to add a theme, returning for now, as this is not implemented yet");
                return Ok(());
            } else {
                warn!(
                    r#""theme.mp3" is the only audio file supported for now, as the current focus is on movies and series, anything else will be implemented at later date"#
                );
                return Ok(());
            }
        }
        FileType::Video => {}
    }

    let conn = db.get()?;

    // TODO: Try to infer stuff from further down the path
    match infer_from_filename(path) {
        Classification::Movie { title } => {
            conn.execute(
                "INSERT INTO movies (videoid, title) VALUES (?1, ?2)",
                params![id, title],
            )?;
        }
        Classification::Episode {
            title,
            season,
            episode,
        } => {
            let episode = episode.unwrap_or_else(|| {
                    warn!("Episode number not infered from anywhere else right now, defaulting to 1, this might be incorrect");
                    1
                });

            let season = season.unwrap_or_else(|| {
                    warn!("Season number not infered from anywhere else right now, defaulting to 1, this might be incorrect");
                    1
                });

            let mut similar_datafiles_stmt =
                conn.prepare("SELECT id FROM data_files WHERE path LIKE (?1 || '%') AND id != ?2")?;

            let mut all_similar = Vec::new();
            for parent in path.ancestors().skip(1) {
                let similar_ids = similar_datafiles_stmt
                    .query_map(params![path_to_db(parent), id], |r| r.get(0))?
                    .collect::<Result<Vec<u64>, _>>()?;
                if !similar_ids.is_empty() {
                    all_similar.extend(similar_ids);
                    break;
                }
            }

            rusqlite::vtab::array::load_module(&conn)?;

            let values: Rc<Vec<Value>> = Rc::new(
                all_similar
                    .iter()
                    .map(|x| Value::Integer(*x as i64))
                    .collect(),
            );

            let mut seasons_stmt =
                conn.prepare("SELECT DISTINCT seasonid FROM episodes WHERE videoid IN rarray(?1)")?;

            let seasonids = seasons_stmt
                .query_map([&values], |r| r.get(0))?
                .collect::<Result<Vec<u64>, _>>()?;

            match seasonids.len() {
                0 => {
                    create_completely_new_episode(&conn, id, episode, season, title)?;
                    return Ok(());
                }
                1.. => {
                    let mut season_id = None;
                    if seasonids.len() > 1 {
                        conn.create_scalar_function(
                            "common",
                            2,
                            FunctionFlags::SQLITE_DETERMINISTIC | FunctionFlags::SQLITE_UTF8,
                            |ctx| {
                                assert_eq!(
                                    ctx.len(),
                                    2,
                                    "called with unexpected number of arguments"
                                );
                                let s1: Option<String> = ctx.get(0)?;
                                let s2: Option<String> = ctx.get(1)?;
                                let common =
                                    common(&s1.unwrap_or_default(), &s2.unwrap_or_default());
                                Ok(common)
                            },
                        )?;

                        let series_id: Option<u64> = conn.query_row(
                            "SELECT DISTINCT seriesid, MAX(common(name, ?1)) as similarity FROM seasons WHERE id IN (
                            SELECT DISTINCT seasonid FROM episodes WHERE videoid IN rarray(?2)
                        ) ORDER BY similarity DESC LIMIT 1",
                            params![title, values],
                            |r| r.get(0),
                        )?;

                        if let Some(series_id) = series_id {
                            let matching_season: Option<u64> = conn
                                .query_row(
                                    "SELECT id FROM seasons WHERE seriesid=?1 AND season=?2",
                                    [series_id, season],
                                    |r| r.get(0),
                                )
                                .optional()?;

                            if let Some(matching) = matching_season {
                                season_id = Some(matching);
                            }
                        }
                    };

                    let season_id = season_id.unwrap_or(seasonids[0]);
                    let (db_season, series_id): (u64, u64) = conn.query_row(
                        "SELECT season, seriesid FROM seasons WHERE id=?1",
                        [season_id],
                        |r| Ok((r.get(0)?, r.get(1)?)),
                    )?;
                    let series_title: String =
                        conn.query_row("SELECT title FROM series WHERE id=?1", [series_id], |r| {
                            r.get(0)
                        })?;

                    match (db_season == season, series_title == title) {
                        (true, true) => {
                            let season_name: Option<String> = conn.query_row(
                                "SELECT name FROM seasons WHERE id=?1",
                                [season_id],
                                |r| r.get(0),
                            )?;

                            if season_name.is_some_and(|season_name| season_name == title) {
                                conn.execute("INSERT INTO episodes (seasonid, videoid, episode) VALUES (?1, ?2, ?3)",params![season_id, id, episode],)?;
                                conn.execute(
                                    "UPDATE episodes SET name=NULL WHERE seasonid=?1 AND name=?2",
                                    params![season_id, title],
                                )?;
                                conn.execute(
                                    "UPDATE seasons SET name=NULL WHERE id=?1",
                                    params![season_id],
                                )?;
                            } else {
                                conn.execute("INSERT INTO episodes (seasonid, videoid, episode) VALUES (?1, ?2, ?3)",params![season_id, id, episode],)?;
                            }
                        }
                        (true, false) => {
                            let common = common(&series_title, &title);
                            if similarity(&series_title, &title) > 0.5 {
                                conn.execute(
                                    "UPDATE series SET title=?1 WHERE id=?2",
                                    params![common, series_id],
                                )?;
                                conn.execute(
                                    "INSERT INTO episodes (seasonid, videoid, episode) VALUES (?1, ?2, ?3)",
                                    params![season_id, id, episode],
                                )?;
                            } else {
                                create_completely_new_episode(&conn, id, episode, season, title)?;
                            }
                        }
                        (false, true) => {
                            conn.execute(
                                "UPDATE seasons SET name=NULL WHERE seriesid=?1 AND name=?2",
                                params![series_id, title],
                            )?;
                            conn.execute(
                                "INSERT INTO seasons (seriesid, season) VALUES (?1, ?2)",
                                params![series_id, season],
                            )?;
                            conn.execute(
                                "INSERT INTO episodes (seasonid, videoid, episode) VALUES (last_insert_rowid(), ?1, ?2)",
                                params![id, episode],
                            )?;
                        }
                        (false, false) => {
                            if similarity(&series_title, &title) > 0.5 {
                                let common = common(&series_title, &title);
                                conn.execute(
                                    "UPDATE series SET title=?1 WHERE id=?2",
                                    params![common, series_id],
                                )?;
                                conn.execute(
                                    "INSERT INTO seasons (seriesid, season, name) VALUES (?1, ?2, ?3)",
                                    params![series_id, season, title],
                                )?;
                                conn.execute(
                                    "INSERT INTO episodes (seasonid, videoid, episode) VALUES (last_insert_rowid(), ?1, ?2)",
                                    params![id, episode],
                                )?;
                            } else {
                                create_completely_new_episode(&conn, id, episode, season, title)?
                            }
                        }
                    }
                }
                _ => unreachable!(),
            }
        }
    }

    Ok(())
}

fn create_completely_new_episode(
    db: &Connection,
    videoid: u64,
    episode: u64,
    season: u64,
    title: String,
) -> DatabaseResult<()> {
    db.execute("INSERT INTO series (title) VALUES (?1)", params![title])?;
    db.execute(
        "INSERT INTO seasons (seriesid, season, name) VALUES (last_insert_rowid(), ?1, ?2)",
        params![season, title],
    )?;
    db.execute(
        "INSERT INTO episodes (seasonid, videoid, episode, name) VALUES (last_insert_rowid(), ?1, ?2, ?3)",
        params![videoid, episode, title],
    )?;

    Ok(())
}

// TODO: There need to be tests for this
fn infer_from_filename(path: &Path) -> Classification {
    let file_stem = os_str_conversion(path.file_stem().unwrap());
    let mut split = file_stem.split('-');
    let metadata = split.next_back().unwrap();

    let (mut season, mut episode) = (None, None);

    [('s', &mut season), ('e', &mut episode)].map(|(delim, var)| {
        if let Ok(x) = metadata
            .chars()
            .skip_while(|&c| c != delim)
            .skip(1)
            .take_while(char::is_ascii_digit)
            .collect::<String>()
            .parse::<u64>()
        {
            *var = Some(x);
        }
    });

    let is_movie = season.is_none() && episode.is_none();

    let mut rest = String::new();

    for part in split {
        rest.push_str(part);
        rest.push('-');
    }

    if is_movie {
        rest.push_str(metadata);
    } else {
        rest.pop();
    }

    let rest = rest.trim_end_matches(char::is_whitespace);
    let mut whitespace_seperated = rest.split_whitespace();
    let mut name = String::new();
    let last = whitespace_seperated.next_back();
    if last.is_none() {
        panic!("didn't work on {file_stem}, rest: {rest}")
    }
    let last = last.unwrap();

    if let Ok(year) = last
        .chars()
        .skip(1)
        .take_while(char::is_ascii_digit)
        .collect::<String>()
        .parse::<u16>()
    {
        let _ = year; // TODO: use year at some point

        for part in whitespace_seperated {
            name.push_str(part);
            name.push(' ');
        }
        name.pop();
    } else {
        name = rest.to_string();
    }

    if is_movie {
        Classification::Movie { title: name }
    } else {
        Classification::Episode {
            title: name,
            season,
            episode,
        }
    }
}

enum Classification {
    Movie {
        title: String,
    },
    Episode {
        title: String,
        season: Option<u64>,
        episode: Option<u64>,
    },
}

// TODO: Make recursive a setting maybe?
fn scan_dir(path: PathBuf) -> Vec<PathBuf> {
    path.read_dir().map_or(Vec::new(), |read_dir| {
        let mut out = Vec::new();

        for entry in read_dir {
            if let Some(entry) =
                entry.log_err_with_msg("Encountered IO Error while scanning directory")
            {
                let path = entry.path();
                if path.is_dir() {
                    out.extend(scan_dir(path));
                } else {
                    out.push(path);
                }
            }
        }
        out
    })
}

#[derive(Debug, PartialEq, Eq)]
pub enum FileType {
    Video,
    Audio,
    Unknown,
}

/// Returns the filetype classified into what is known by the system
/// Returns None if the path has no file extension or if it isn't valid utf-8
fn file_type(path: &Path) -> Option<FileType> {
    match path.extension() {
        Some(ext) => match os_str_conversion(ext) {
            "mp4" => Some(FileType::Video),
            "mp3" => Some(FileType::Audio),
            _ => Some(FileType::Unknown),
        },
        None => None,
    }
}

// TODO: Switch to a more sophisticated algorithm
fn similarity(s1: &str, s2: &str) -> f32 {
    common(s1, s2).len() as f32 / s1.len().max(s2.len()) as f32
}

fn common(s1: &str, s2: &str) -> String {
    s1.chars()
        .zip(s2.chars())
        .take_while(|(c1, c2)| c1 == c2)
        .map(|(c, _)| c)
        .collect()
}

// TODO: These conversions are implemented here so i stay consistent with what i use -> find a better approach
// They unwrap and I am not sure how relevant it is, might need to be fixed, can utf-8 validity be assumed?
fn path_to_db(path: &Path) -> &str {
    path.to_str().unwrap()
}

fn os_str_conversion(os_str: &OsStr) -> &str {
    os_str.to_str().unwrap()
}
