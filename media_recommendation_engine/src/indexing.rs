use std::{
    ffi::OsStr,
    path::{Component, Path, PathBuf},
};

use itertools::Itertools;
use rusqlite::{params, Connection, OptionalExtension};
use tracing::{debug, info, warn};

use crate::{
    database::{Database, QueryRowGetConnExt, QueryRowGetStmtExt, QueryRowIntoConnExt},
    state::AppResult,
    utils::{HandleErr, Ignore, ParseBetween, ParseUntil, ServerSettings},
};

pub async fn periodic_indexing(db: Database, settings: ServerSettings) -> ! {
    loop {
        let conn = db
            .get()
            .expect("Failed to get database connection for indexing");
        conn.call(|conn| Ok(indexing(conn).log_err_with_msg("Failed the indexing")))
            .await
            .log_err_with_msg("failed to index");
        settings.wait_configured_time().await;
    }
}

fn indexing(conn: &rusqlite::Connection) -> AppResult<()> {
    let locations: Vec<String> = conn
        .prepare("SELECT path FROM storage_locations")?
        .query_map_get([])?
        .collect::<Result<Vec<_>, _>>()?;

    let filesystem = locations
        .into_iter()
        .map(PathBuf::from)
        .flat_map(|path: std::path::PathBuf| scan_dir(&path))
        .collect::<Vec<_>>();

    let database_registered: Vec<PathBuf> = {
        conn.prepare("SELECT path from data_files")?
            .query_map([], |row| Ok(PathBuf::from(row.get::<usize, String>(0)?)))?
            .collect::<Result<Vec<_>, _>>()
    }?;

    // Note: There is probably a faster? way to do these two loops, but this is good enough for now
    // TODO: Also track changes and handle that

    let mut insertion_stmt =
        conn.prepare("INSERT INTO data_files (path) VALUES (?1) RETURNING id")?;

    let files = filesystem
        .iter()
        .filter(|file| !database_registered.contains(file))
        .map(|file| {
            Ok((
                file.clone(),
                insertion_stmt.query_row_get([path_to_db(file)])?,
            ))
        })
        .collect::<AppResult<Vec<_>>>()?;

    classify_new_files(conn, files)?;

    // TODO: Removals will probably have to do more than just remove the data_file
    // Important: Don't delete any directly user facing info, is still important for recommendation, maybe consider deletion when recommending?
    for file in &database_registered {
        if !filesystem.contains(file) {
            debug!("Want to remove {file:?}");
        }
    }

    info!("Finished indexing once");
    Ok(())
}

// TODO: Consider theme, ...
/// This tries to insert the file into the database as best as possible
fn classify_new_files(db: &Connection, data_files: Vec<(PathBuf, u64)>) -> AppResult<()> {
    let groups = data_files
        .into_iter()
        .map(|(path, id)| (file_type(&path), path, id))
        .filter_map(|(file_type, path, id)| {
            if let Some(file_type) = file_type {
                return Some((file_type, path, id));
            }
            warn!("Filetype of {path:?} is currently unknown");
            None
        })
        .group_by(|(file_type, _, _)| file_type.clone());

    groups
        .into_iter()
        .map(|(file_type, group)| match file_type {
            FileType::Video => classify_video(db, group),
            FileType::Audio => classify_audio(db, group),
            FileType::Unknown => handle_unknown(db, group),
        })
        .collect::<AppResult<Vec<_>>>()?;
    Ok(())
}

fn classify_video(
    db: &Connection,
    video_files: impl Iterator<Item = (FileType, PathBuf, u64)>,
) -> AppResult<()> {
    for (_file_type, path, data_id) in video_files {
        let file_classification = infer_from_video_filename(&path);
        let path_classification = infer_from_video_path(&path);

        match file_classification {
            Classification::Movie { title, part } => {
                let (video_id, flags) = resolve_part(db, part, data_id)?;

                let franchise = match path_classification {
                    PathClassification::Movie { franchise, .. }
                    | PathClassification::Episode { franchise, .. } => franchise,
                }
                .unwrap_or(title);

                // find franchise or insert new
                let franchise_id: u64 = {
                    let id = db
                        .query_row_get("SELECT id FROM franchise WHERE title = ?1", [&franchise])
                        .optional()?;

                    if let Some(id) = id {
                        id
                    } else {
                        db.query_row_get(
                            "INSERT INTO franchise (title) VALUES (?1) RETURNING id",
                            [&franchise],
                        )?
                    }
                };

                db.execute(
                    "INSERT INTO movies (videoid, franchiseid, referenceflag, title) VALUES (?1, ?2, ?3, ?4)",
                    params![video_id, franchise_id, flags, title],
                )?;
            }
            Classification::Episode {
                title: _,
                season,
                episode,
                part,
            } => {
                let mut classification = EpisodeClassification {
                    episode_title: None,
                    episode,
                    season_title: None,
                    season,
                    series_title: None,
                    part,
                    franchise: None,
                };

                if let PathClassification::Episode {
                    episode_title,
                    episode,
                    season_title,
                    season,
                    series_title,
                    franchise,
                } = path_classification
                {
                    classification.episode_title = classification.episode_title.or(episode_title);
                    classification.episode = classification.episode.or(episode);
                    classification.season_title = classification.season_title.or(season_title);
                    classification.season = classification.season.or(season);
                    classification.series_title = classification.series_title.or(series_title);
                    classification.franchise =
                        classification.franchise.or(franchise).or(series_title);
                }

                // find franchise or insert new
                let franchise_id: u64 = {
                    let id = db
                        .query_row_get(
                            "SELECT id FROM franchise WHERE title = ?1",
                            [&classification.franchise],
                        )
                        .optional()?;

                    if let Some(id) = id {
                        id
                    } else {
                        db.query_row_get(
                            "INSERT INTO franchise (title) VALUES (?1) RETURNING id",
                            [&classification.franchise],
                        )?
                    }
                };

                // Find series or insert new
                let series_id: u64 = {
                    let id = db
                        .query_row_get(
                            "SELECT id FROM series WHERE title = ?1",
                            [&classification.series_title],
                        )
                        .optional()?;

                    if let Some(id) = id {
                        id
                    } else {
                        db.query_row_get(
                            "INSERT INTO series (franchiseid, title) VALUES (?1, ?2) RETURNING id",
                            params![franchise_id, &classification.series_title],
                        )?
                    }
                };

                // Find season or insert new
                let season_id: u64 = {
                    let id = db
                        .query_row_get(
                            "SELECT id FROM seasons WHERE seriesid = ?1 AND season = ?2",
                            params![&series_id, &classification.season],
                        )
                        .optional()?;

                    if let Some(id) = id {
                        id
                    } else {
                        db.query_row_get(
                            "INSERT INTO seasons (seriesid, season, title) VALUES (?1, ?2, ?3) RETURNING id",
                            params![&series_id, &classification.season, &classification.season_title],
                        )?
                    }
                };

                // Insert Episode accordingly
                let (multipart_id, flags) = resolve_part(db, classification.part, data_id)?;

                let info: Option<(u64, u64, u64)> = db
                    .query_row_into(
                        "SELECT id, videoid, referenceflag FROM episodes WHERE seasonid = ?1 AND episode = ?2 AND title = ?3",
                        params![&season_id, &classification.episode, &classification.episode_title],
                    )
                    .optional()?;

                if let Some((id, video_id, referenceflag)) = info {
                    if referenceflag == 0 {
                        db.execute(
                            "UPDATE episodes SET videoid=?1, referenceflag=?2 WHERE id=?3",
                            params![&multipart_id, &flags, &id],
                        )?;
                        db.execute(
                            "INSERT INTO multipart (id, videoid, part) VALUES (?1, ?2, ?3)",
                            params![&id, &video_id, 0],
                        )?;
                    } else if referenceflag == 1 {
                        db.execute(
                            "UPDATE multipart SET id=?1 WHERE id=?2",
                            [video_id, multipart_id],
                        )?;
                    }
                } else {
                    db.execute(
                            "INSERT INTO episodes (seasonid, videoid, referenceflag, episode, title) VALUES (?1, ?2, ?3, ?4, ?5)",
                            params![&season_id, &multipart_id, &flags, &classification.episode, &classification.episode_title]
                        )?;
                }
            }
        }
    }
    Ok(())
}

struct EpisodeClassification<'a> {
    episode_title: Option<&'a str>,
    episode: Option<u64>,
    season_title: Option<&'a str>,
    season: Option<u64>,
    series_title: Option<&'a str>,
    part: Option<u64>,
    franchise: Option<&'a str>,
}

fn classify_audio(
    _db: &Connection,
    audio_files: impl Iterator<Item = (FileType, PathBuf, u64)>,
) -> AppResult<()> {
    for (filetype, path, _data_id) in audio_files {
        if !os_str_conversion(path.file_name().unwrap()).contains("theme")
            && FileType::Audio == filetype
        {
            warn!(
                r#"{path:?} could not be handled, only "theme.mp3" can be inserted into the dataset"#
            );
            continue;
        }
        warn!("adding theme not yet supported");
    }
    Ok(())
}

fn handle_unknown(
    _db: &Connection,
    unknown_files: impl Iterator<Item = (FileType, PathBuf, u64)>,
) -> AppResult<()> {
    for (_, path, _) in unknown_files {
        warn!("Could not handle {path:?}");
    }
    Ok(())
}

fn infer_from_video_path(path: &Path) -> PathClassification {
    let mut components = path
        .components()
        .rev()
        .take_while(|comp| matches!(comp, Component::Normal(_)))
        .map(|comp| os_str_conversion(comp.as_os_str()));

    let (mut episode_title, mut episode, mut season_title, mut season, mut series_title) =
        (None, None, None, None, None);
    let mut names: Vec<&str> = Vec::new();

    let file_name = components.next().unwrap();

    names.push(match infer_from_video_filename(Path::new(file_name)) {
        Classification::Movie { title, .. } => title,
        Classification::Episode {
            title,
            episode: e,
            season: s,
            ..
        } => {
            (season, episode) = (s, e);
            title
        }
    });

    // Most verbose allowed format: /series_name/"season x"/season_title/"episode x"/episode_title/file_name
    let mut i = 0;
    // take episode_title and maybe the season and episode
    while i < 4 {
        if let Some(dir_name) = components.next() {
            let lower_dir_name: &str = &dir_name.to_ascii_lowercase();
            if lower_dir_name.starts_with("episode") && i < 2 {
                lower_dir_name
                    .parse_between("episode ", |c: char| !c.is_ascii_digit())
                    .map(|e| {
                        episode = Some(e);
                        i = 1;
                    })
                    .ignore();
            } else if lower_dir_name.starts_with("season") && i < 4 {
                lower_dir_name
                    .parse_between("season ", |c: char| !c.is_ascii_digit())
                    .map(|s| {
                        season = Some(s);
                        i = 3;
                    })
                    .ignore();
            } else {
                names.push(dir_name);
            }
        }
        i += 1;
    }

    // Take potential series name
    components
        .next()
        .map(|dir_name| names.push(dir_name))
        .ignore();

    if season.is_some() || episode.is_some() {
        names.dedup();

        names
            .into_iter()
            .rev()
            .zip(&mut [&mut series_title, &mut season_title, &mut episode_title])
            .map(|(name, var)| {
                **var = Some(name);
            })
            .for_each(drop);

        PathClassification::Episode {
            episode_title,
            episode,
            season_title,
            season,
            series_title,
            franchise: series_title,
        }
    } else {
        let mut names = names.into_iter().rev();
        let _category = names.next();
        let franchise = names.find(|&name| {
            let file_name = os_str_conversion(Path::new(file_name).file_stem().unwrap());
            name != file_name
        });
        PathClassification::Movie { franchise }
    }
}

#[derive(Debug)]
enum PathClassification<'a> {
    Movie {
        franchise: Option<&'a str>,
    },
    Episode {
        episode_title: Option<&'a str>,
        episode: Option<u64>,
        season_title: Option<&'a str>,
        season: Option<u64>,
        series_title: Option<&'a str>,
        franchise: Option<&'a str>,
    },
}

// TODO: There need to be tests for this
fn infer_from_video_filename(path: &Path) -> Classification {
    let file_stem = os_str_conversion(path.file_stem().unwrap());
    let (begin, metadata) = file_stem.rsplit_once('-').unwrap_or((file_stem, ""));

    let (mut season, mut episode, mut part) = (None, None, None);

    [('s', &mut season), ('e', &mut episode), ('p', &mut part)].map(|(delim, var)| {
        metadata
            .parse_between(delim, |c: char| !c.is_ascii_digit())
            .map(|num| *var = Some(num))
            .ignore();
    });

    let is_movie = season.is_none() && episode.is_none();
    let rest = if is_movie { file_stem } else { begin }.trim_end();

    let (begin, last) = rest.rsplit_once(char::is_whitespace).unwrap_or((rest, ""));
    // TODO: use year at some point
    // 1 is skipped because the year is expected to be in brackets
    let (name, _year): (_, Option<u16>) = (&last[1..])
        .parse_until(|c: char| !c.is_ascii_digit())
        .map_or_else(|_e| (rest, None), |year| (begin, Some(year)));

    if is_movie {
        return Classification::Movie { title: name, part };
    }
    Classification::Episode {
        title: name,
        season,
        episode,
        part,
    }
}

#[derive(Debug)]
enum Classification<'a> {
    Movie {
        title: &'a str,
        part: Option<u64>,
    },
    Episode {
        title: &'a str,
        season: Option<u64>,
        episode: Option<u64>,
        part: Option<u64>,
    },
}

// TODO: Make recursive a setting maybe?
fn scan_dir(path: &Path) -> Vec<PathBuf> {
    path.read_dir().map_or(Vec::new(), |read_dir| {
        let mut out = Vec::new();

        for entry in read_dir {
            if let Some(entry) =
                entry.log_err_with_msg("Encountered IO Error while scanning directory")
            {
                let path = entry.path();
                if path.is_dir() {
                    out.extend(scan_dir(&path));
                } else {
                    out.push(path);
                }
            }
        }
        out
    })
}

fn resolve_part(db: &Connection, part: Option<u64>, data_id: u64) -> AppResult<(u64, u64)> {
    Ok(if let Some(part) = part {
        (
            db.query_row_get(
                "INSERT INTO multipart (id, videoid, part) VALUES 
                (
                    IFNULL((SELECT MAX(id) + 1 FROM multipart), 0),
                    ?1, ?2
                ) 
                RETURNING id",
                [data_id, part],
            )?,
            1,
        )
    } else {
        (data_id, 0)
    })
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

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum FileType {
    Video,
    Audio,
    Unknown,
}

// TODO: These conversions are implemented here so i stay consistent with what i use -> find a better approach
// They unwrap and I am not sure how relevant it is, might need to be fixed, can utf-8 validity be assumed?
fn path_to_db(path: &Path) -> &str {
    path.to_str().unwrap()
}

fn os_str_conversion(os_str: &OsStr) -> &str {
    os_str.to_str().unwrap()
}
