use std::{
    ffi::OsStr,
    path::{Component, Path, PathBuf},
    time::Duration,
};

use itertools::Itertools;
use rusqlite::{params, Connection, OptionalExtension};
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

    let files = filesystem
        .iter()
        .filter(|file| !database_registered.contains(file))
        .map(|file| {
            Ok((
                file.clone(),
                insertion_stmt.query_row([path_to_db(file)], |row| row.get(0))?,
            ))
        })
        .collect::<DatabaseResult<Vec<_>>>()?;

    classify_new_files(&conn, files)?;

    // TODO: Removals will probably have to do more than just remove the data_file
    // Important: Don't delete any directly user facing info, is still important for recommendation, maybe consider deletion when recommending?
    for file in &database_registered {
        if !filesystem.contains(file) {
            debug!("Want to rmove {file:?}")
        }
    }

    info!("Finished indexing once");
    Ok(())
}

// TODO: MULTIPART ON FRONTEND!!!
// TODO: Consider Franchise, theme, ...
/// This tries to insert the file into the database as best as possible
fn classify_new_files(db: &Connection, data_files: Vec<(PathBuf, u64)>) -> DatabaseResult<()> {
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

    let _ = groups
        .into_iter()
        .map(|(file_type, group)| match file_type {
            FileType::Video => classify_video(db, group),
            FileType::Audio => classify_audio(db, group),
            FileType::Unknown => handle_unknown(db, group),
        })
        .collect::<DatabaseResult<Vec<_>>>()?;
    Ok(())
}

fn classify_video(
    db: &Connection,
    video_files: impl Iterator<Item = (FileType, PathBuf, u64)>,
) -> DatabaseResult<()> {
    for (_file_type, path, data_id) in video_files {
        let file_classification = infer_from_video_filename(&path);
        let path_classification = infer_from_video_path(&path);

        match file_classification {
            Classification::Movie { title, part } => {
                let (video_id, flags) = resolve_part(db, part, data_id)?;
                db.execute(
                    "INSERT INTO movies (videoid, referenceflag, title) VALUES (?1, ?2, ?3)",
                    params![video_id, flags, title],
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
                };

                if let PathClassification::Episode {
                    episode_title,
                    episode,
                    season_title,
                    season,
                    series_title,
                } = path_classification
                {
                    if classification.episode_title.is_none() {
                        classification.episode_title = episode_title;
                    }
                    if classification.episode.is_none() {
                        classification.episode = episode;
                    }
                    if classification.season_title.is_none() {
                        classification.season_title = season_title;
                    }
                    if classification.season.is_none() {
                        classification.season = season;
                    }
                    if classification.series_title.is_none() {
                        classification.series_title = series_title;
                    }
                }

                // Find series or insert new
                let series_id: u64 = {
                    let id = db
                        .query_row(
                            "SELECT id FROM series WHERE title = ?1",
                            [&classification.series_title],
                            |row| row.get(0),
                        )
                        .optional()?;

                    if let Some(id) = id {
                        id
                    } else {
                        db.query_row(
                            "INSERT INTO series (title) VALUES (?1) RETURNING id",
                            [&classification.series_title],
                            |row| row.get(0),
                        )?
                    }
                };

                // Find season or insert new
                let season_id: u64 = {
                    let id = db
                        .query_row(
                            "SELECT id FROM seasons WHERE seriesid = ?1 AND season = ?2",
                            params![&series_id, &classification.season],
                            |row| row.get(0),
                        )
                        .optional()?;

                    if let Some(id) = id {
                        id
                    } else {
                        db.query_row(
                            "INSERT INTO seasons (seriesid, season, title) VALUES (?1, ?2, ?3) RETURNING id",
                            params![&series_id, &classification.season, &classification.season_title],
                            |row| row.get(0),
                        )?
                    }
                };

                // Insert Episode accordingly
                let (multipart_id, flags) = resolve_part(db, classification.part, data_id)?;

                let info: Option<(u64, u64, u64)> = db
                    .query_row(
                        "SELECT id, videoid, referenceflag FROM episodes WHERE seasonid = ?1 AND episode = ?2 AND title = ?3",
                        params![&season_id, &classification.episode, &classification.episode_title],
                        |row| Ok((row.get(0)?, row.get(1)?, row.get(2)?)),
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

struct EpisodeClassification {
    episode_title: Option<String>,
    episode: Option<u64>,
    season_title: Option<String>,
    season: Option<u64>,
    series_title: Option<String>,
    part: Option<u64>,
}

fn classify_audio(
    _db: &Connection,
    audio_files: impl Iterator<Item = (FileType, PathBuf, u64)>,
) -> DatabaseResult<()> {
    for (_, path, _data_id) in audio_files {
        if os_str_conversion(path.file_name().unwrap()) != "theme.mp3" {
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
) -> DatabaseResult<()> {
    for (_, path, _) in unknown_files {
        warn!("Could not handle {path:?}")
    }
    Ok(())
}

fn infer_from_video_path(path: &Path) -> PathClassification {
    let mut components = path
        .components()
        .rev()
        .take_while(|comp| matches!(comp, Component::Normal(_)))
        .map(|comp| os_str_conversion(comp.as_os_str()));

    struct IntermediateClassification {
        episode_title: Option<String>,
        episode: Option<u64>,
        season_title: Option<String>,
        season: Option<u64>,
        series_title: Option<String>,
    }

    let mut intermediate = IntermediateClassification {
        episode_title: None,
        episode: None,
        season_title: None,
        season: None,
        series_title: None,
    };

    let mut names: Vec<String> = Vec::new();

    if let Some(file_name) = components.next() {
        // TODO: Seperate the parsing of the format parsed in this function
        let classification = infer_from_video_filename(Path::new(file_name));
        names.push(match classification {
            Classification::Movie { title, .. } => title,
            Classification::Episode {
                title,
                episode,
                season,
                ..
            } => {
                intermediate.episode = episode;
                intermediate.season = season;
                title
            }
        })
    }

    // Most verbose allowed format: /series_name/"season x"/season_title/"episode x"/episode_title/file_name
    let mut i = 0;
    // take episode_title and maybe the season and episode
    while i < 4 {
        if let Some(dir_name) = components.next() {
            let lower_dir_name = dir_name.to_ascii_lowercase();
            if lower_dir_name.starts_with("episode") && i < 2 {
                if let Ok(episode) = lower_dir_name
                    .trim_start_matches("episode")
                    .trim_start()
                    .chars()
                    .take_while(char::is_ascii_digit)
                    .collect::<String>()
                    .parse::<u64>()
                {
                    intermediate.episode = Some(episode);
                    i = 1;
                }
            } else if lower_dir_name.starts_with("season") && i < 4 {
                if let Ok(season) = lower_dir_name
                    .trim_start_matches("season")
                    .trim_start()
                    .chars()
                    .take_while(char::is_ascii_digit)
                    .collect::<String>()
                    .parse::<u64>()
                {
                    intermediate.season = Some(season);
                    i = 3;
                }
            } else {
                names.push(dir_name.to_owned())
            }
        }

        i += 1;
    }

    // Take potential series name
    if let Some(dir_name) = components.next() {
        names.push(dir_name.to_owned())
    }

    if intermediate.season.is_some() || intermediate.episode.is_some() {
        names.dedup();

        names
            .into_iter()
            .rev()
            .zip(&mut [
                &mut intermediate.series_title,
                &mut intermediate.season_title,
                &mut intermediate.episode_title,
            ])
            .map(|(name, var)| {
                **var = Some(name);
            })
            .for_each(drop);
        PathClassification::Episode {
            episode_title: intermediate.episode_title,
            episode: intermediate.episode,
            season_title: intermediate.season_title,
            season: intermediate.season,
            series_title: intermediate.series_title,
        }
    } else {
        PathClassification::Movie {}
    }
}

#[derive(Debug)]
enum PathClassification {
    Movie {},
    Episode {
        episode_title: Option<String>,
        episode: Option<u64>,
        season_title: Option<String>,
        season: Option<u64>,
        series_title: Option<String>,
    },
}

// TODO: There need to be tests for this
// TODO: See if itertools can make this function better
fn infer_from_video_filename(path: &Path) -> Classification {
    let file_stem = os_str_conversion(path.file_stem().unwrap());
    let mut split = file_stem.split('-');
    let metadata = split.next_back().unwrap();

    let (mut season, mut episode, mut part) = (None, None, None);

    [('s', &mut season), ('e', &mut episode), ('p', &mut part)].map(|(delim, var)| {
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
        panic!("infering stuff didn't work on {file_stem}, rest: {rest}")
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
        Classification::Movie { title: name, part }
    } else {
        Classification::Episode {
            title: name,
            season,
            episode,
            part,
        }
    }
}

#[derive(Debug)]
enum Classification {
    Movie {
        title: String,
        part: Option<u64>,
    },
    Episode {
        title: String,
        season: Option<u64>,
        episode: Option<u64>,
        part: Option<u64>,
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

fn resolve_part(db: &Connection, part: Option<u64>, data_id: u64) -> DatabaseResult<(u64, u64)> {
    Ok(if let Some(part) = part {
        (
            db.query_row(
                "INSERT INTO multipart (id, videoid, part) VALUES 
                (
                    IFNULL((SELECT MAX(id) + 1 FROM multipart), 0),
                    ?1, ?2
                ) 
                RETURNING id",
                [data_id, part],
                |row| row.get(0),
            )?,
            1,
        )
    } else {
        (data_id, 0)
    })
}

#[derive(Debug, Clone, PartialEq, Eq)]
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

// TODO: These conversions are implemented here so i stay consistent with what i use -> find a better approach
// They unwrap and I am not sure how relevant it is, might need to be fixed, can utf-8 validity be assumed?
fn path_to_db(path: &Path) -> &str {
    path.to_str().unwrap()
}

fn os_str_conversion(os_str: &OsStr) -> &str {
    os_str.to_str().unwrap()
}
