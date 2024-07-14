use std::{collections::HashSet, ffi::OsStr, path::Path};

use anyhow::Context;
use rusqlite::{params, OptionalExtension};
use tracing::warn;

use crate::{
    database::{
        Connection, QueryRowGetConnExt, QueryRowGetStmtExt, QueryRowIntoConnExt,
        QueryRowIntoStmtExt,
    },
    state::AppResult,
    utils::{Ignore, ParseBetween, ParseUntil},
};

use super::{
    db::{CollectionType, ContentType, TableId},
    file_handling::{AsDBString, FileType, PathExt},
};

pub struct Classification {
    pub title: String,
    pub part: u64,
    pub category: ClassificationCategory,
    pub collectionhint: CollectionHint,
}

impl Classification {
    fn empty() -> Self {
        Classification {
            title: String::new(),
            part: 0,
            category: ClassificationCategory::Other,
            collectionhint: CollectionHint::None,
        }
    }

    fn new(
        title: String,
        category: ClassificationCategory,
        collectionhint: CollectionHint,
    ) -> Self {
        Classification {
            title,
            part: 0,
            category,
            collectionhint,
        }
    }
}

pub enum ClassificationCategory {
    Other,
    Movie,
    Episode { episode: u64 },
    Song,
}

pub enum CollectionHint {
    None,
    Movie(Movie),
    Franchise(Franchise),
    Series(Series),
    Season(Season),
    ThemeTarget { inner: Box<CollectionHint> },
}

impl CollectionHint {
    fn franchise(title: String) -> Self {
        CollectionHint::Franchise(Franchise { title })
    }

    fn movie(title: String, franchise: Option<Franchise>) -> Self {
        CollectionHint::Movie(Movie { title, franchise })
    }

    fn series(title: String, franchise: Option<Franchise>) -> Self {
        CollectionHint::Series(Series { title, franchise })
    }

    fn season(title: String, season: u64, series: Option<Series>) -> Self {
        CollectionHint::Season(Season {
            title,
            season,
            series,
        })
    }
}

pub struct Franchise {
    pub title: String,
}

pub struct Movie {
    pub title: String,
    pub franchise: Option<Franchise>,
}

pub struct Series {
    pub title: String,
    pub franchise: Option<Franchise>,
}

pub struct Season {
    pub title: String,
    pub season: u64,
    pub series: Option<Series>,
}

impl Classification {
    pub fn content_type(&self) -> ContentType {
        match self.category {
            ClassificationCategory::Other { .. } => ContentType::Other,
            ClassificationCategory::Movie { .. } => ContentType::Movie,
            ClassificationCategory::Episode { .. } => ContentType::Episode,
            ClassificationCategory::Song { .. } => ContentType::Song,
        }
    }
}

pub fn classify(path: &Path, db: &Connection) -> AppResult<Classification> {
    let Some(file_type) = path.file_type() else {
        warn!("Faulty file path: \"{path:?}\"");
        let mut classification = Classification::empty();
        classification.title = path
            .file_stem()
            .map_or_else(|| path.as_db_string(), OsStr::as_db_string)
            .to_string();
        return Ok(classification);
    };

    match file_type {
        FileType::Video => classify_video(path, db),
        FileType::Audio => classify_audio(path, db),
        FileType::Unknown => Ok(classify_unknown(path, db)),
    }
}

fn classify_audio(path: &Path, db: &Connection) -> AppResult<Classification> {
    let file_name = path.file_stem().unwrap_or_default().as_db_string();

    let collection = if file_name.contains("theme") {
        let hint = infer_collection(path, db)?;
        CollectionHint::ThemeTarget {
            inner: Box::new(hint),
        }
    } else {
        infer_collection(path, db)?
    };

    let (title, _year) = strip_year(&file_name);
    Ok(Classification::new(
        title.to_owned(),
        ClassificationCategory::Song,
        collection,
    ))
}

fn classify_video(path: &Path, db: &Connection) -> AppResult<Classification> {
    let title = path.file_stem().unwrap_or_default().as_db_string();
    let (title, info) = strip_info(&title);
    let (title, _year) = strip_year(title);

    let mut c_part = 0;
    let mut c_season = None;

    let category = match info {
        Info {
            season,
            episode: Some(episode),
            part,
        } => {
            if let Some(part) = part {
                c_part = part;
            }
            c_season = season;
            ClassificationCategory::Episode { episode }
        }
        _ => ClassificationCategory::Movie,
    };

    let mut hint = infer_collection(path, db)?;
    if let CollectionHint::Season(Season {
        ref mut season,
        title: _,
        series: _,
    }) = hint
    {
        if let Some(c_season) = c_season {
            *season = c_season;
        }
    }
    Ok(Classification {
        title: title.to_owned(),
        part: c_part,
        category,
        collectionhint: hint,
    })
}

fn classify_unknown(path: &Path, _db: &rusqlite::Connection) -> Classification {
    warn!("Could not handle \"{path:?}\"");
    Classification::empty()
}

fn infer_collection(path: &Path, db: &Connection) -> AppResult<CollectionHint> {
    let database_inferred = infer_collection_from_database(db, path)?;
    let path_inferred = infer_collection_from_path(path)?;

    match (database_inferred, path_inferred) {
        (CollectionHint::None, path_inferred) => Ok(path_inferred),
        (database_inferred, CollectionHint::None) => Ok(database_inferred),
        (CollectionHint::Movie(_), hint @ CollectionHint::Movie(_))
        | (CollectionHint::Franchise(_), hint @ CollectionHint::Franchise(_))
        | (CollectionHint::Series(_), hint @ CollectionHint::Series(_))
        | (CollectionHint::Season(_), hint @ CollectionHint::Season(_)) => {
            // The path is just assumed to be the ground truth for now
            // There might need to be more logic here
            Ok(hint)
        }
        // If only a movie as is found, that is probably movie adjacent content (theme or similar)
        (hint @ CollectionHint::Movie(_), _) | (_, hint @ CollectionHint::Movie(_)) => Ok(hint),
        // Season is more granular than series and series is more granular than franchise
        (
            hint @ CollectionHint::Season(_),
            CollectionHint::Series(_) | CollectionHint::Franchise(_),
        )
        | (
            CollectionHint::Series(_) | CollectionHint::Franchise(_),
            hint @ CollectionHint::Season(_),
        )
        | (CollectionHint::Franchise(_), hint @ CollectionHint::Series(_))
        | (hint @ CollectionHint::Series(_), CollectionHint::Franchise(_)) => Ok(hint),
        (CollectionHint::ThemeTarget { .. }, _) | (_, CollectionHint::ThemeTarget { .. }) => {
            unreachable!("This should be excluded by the database query")
        }
    }
}

fn infer_collection_from_database(db: &Connection, path: &Path) -> AppResult<CollectionHint> {
    let mut all_is_movie = db.prepare_cached(
        "SELECT DISTINCT content.id FROM content, data_file
        WHERE content.data_id = data_file.id
        AND data_file.path LIKE ?1 || '%'
        AND content.type = ?2",
    )?;

    let mut all_direct_matches = db.prepare_cached(
        "SELECT DISTINCT collection.id FROM content, data_file, collection ,collection_contains 
            WHERE content.data_id = data_file.id
            AND data_file.path LIKE ?1 || '%' 
            AND collection_contains.collection_id = collection.id 
            AND collection_contains.reference = content.id
            AND collection.type != ?2
            AND collection_contains.type = ?3",
    )?;

    let mut all_indirect_matches = db.prepare_cached(
        "SELECT DISTINCT collection.id FROM collection, collection_contains
            WHERE collection.type != ?1
            AND collection_contains.collection_id = collection.id
            AND collection_contains.type = ?2
            AND collection_contains.reference = ?3",
    )?;

    let mut collection_id: Option<u64> = None;
    for ancestor in path.ancestors() {
        let direct_matches = all_direct_matches
            .query_map_get::<u64>(params![
                ancestor.as_db_string(),
                CollectionType::Theme,
                TableId::Content
            ])?
            .collect::<Result<HashSet<_>, _>>()?;

        if direct_matches.is_empty() {
            let found_movies = all_is_movie
                .query_map_get::<u64>(params![path.as_db_string(), ContentType::Movie])?
                .collect::<Result<Vec<_>, _>>()?;

            if found_movies.len() == 1 {
                collection_id = Some(found_movies[0]);
                break;
            }

            continue;
        } else if direct_matches.len() == 1 {
            let found_movies = all_is_movie
                .query_map_get::<u64>(params![path.as_db_string(), ContentType::Movie])?
                .collect::<Result<Vec<_>, _>>()?;

            if found_movies.len() == 1 {
                collection_id = Some(found_movies[0]);
                break;
            }

            collection_id = Some(*direct_matches.iter().next().unwrap());
            break;
        }

        // direct_matches.len() > 1
        let mut indirect_matches = HashSet::new();
        for direct in direct_matches {
            let indirect_matches_local = all_indirect_matches
                .query_map_get::<u64>(params![CollectionType::Theme, TableId::Collection, direct])?
                .collect::<Result<HashSet<_>, _>>()?;

            indirect_matches.extend(indirect_matches_local);
        }

        while indirect_matches.len() > 1 {
            let mut new_indirect_matches = HashSet::new();
            for indirect in indirect_matches {
                let new_indirect_matches_local = all_indirect_matches
                    .query_map_get::<u64>(params![
                        CollectionType::Theme,
                        TableId::Collection,
                        indirect
                    ])?
                    .collect::<Result<HashSet<_>, _>>()?;
                new_indirect_matches.extend(new_indirect_matches_local);
            }
            indirect_matches = new_indirect_matches;
        }

        if indirect_matches.len() == 1 {
            collection_id = Some(*indirect_matches.iter().next().unwrap());
            break;
        }
    }

    let Some(collection_id) = collection_id else {
        return Ok(CollectionHint::None);
    };

    let (typ, reference) = db.query_row_into::<(CollectionType, u64)>(
        "SELECT type, reference FROM collection WHERE id = ?1",
        [collection_id],
    )?;

    let hint = match typ {
        CollectionType::UserCollection | CollectionType::Theme => CollectionHint::None, // No Info for now
        CollectionType::Franchise => {
            let title =
                db.query_row_get("SELECT title FROM franchise WHERE id = ?1", [reference])?;
            CollectionHint::franchise(title)
        }
        CollectionType::Season => {
            let (season, title) = db.query_row_into(
                "SELECT season, title FROM season WHERE id = ?1",
                [reference],
            )?;
            CollectionHint::season(
                title,
                season,
                get_series_with_collection(db, collection_id)?,
            )
        }
        CollectionType::Series => {
            let title = db.query_row_get("SELECT title FROM series WHERE id = ?1", [reference])?;
            CollectionHint::series(title, get_franchise_with_collection(db, collection_id)?)
        }
    };

    Ok(hint)
}

fn infer_collection_from_path(path: &Path) -> AppResult<CollectionHint> {
    let preserved_title = path.file_stem().unwrap_or_default().as_db_string();
    let (title, _) = strip_info(&preserved_title);
    let (original_title, _) = strip_year(title);

    let mut directories = path
        .ancestors()
        .skip(1)
        .filter_map(Path::file_name)
        .map(OsStr::as_db_string)
        .take_while(|s| !s.contains("!noclassify"));

    /*
    The current format is very strict:
    - if the first directory does not start with "season", it is a franchise, no further questions asked
    - if it starts with "season" then there can be a whitespace and a number, denoting the season. then there is also a "-" allowed, after which is the title of the season
    - if it was classified as a season, the next directory up is the title of the series and the one after that is the franchise

    This should permit more variations in the future, but I don't even like the datastrutures, so this will do
    */

    let hint = if let Some(next) = directories.next() {
        let lowercase = next.to_lowercase();

        if lowercase.starts_with("season") {
            let season_num = lowercase
                .trim_start_matches("season")
                .trim_start()
                .parse_until(|c: char| !c.is_ascii_digit())
                .with_context(|| format!("Failed to parse season number from \"{next}\""))?;

            let title = next.split_once('-').unwrap_or(("", &next)).1.trim();

            let (series, franchise) = (directories.next(), directories.next());

            match (series, franchise) {
                (Some(series), Some(franchise)) => CollectionHint::season(
                    title.to_string(),
                    season_num,
                    Some(Series {
                        title: series.to_string(),
                        franchise: Some(Franchise {
                            title: franchise.to_string(),
                        }),
                    }),
                ),

                (Some(series), None) => CollectionHint::season(
                    title.to_string(),
                    season_num,
                    Some(Series {
                        title: series.to_string(),
                        franchise: Some(Franchise {
                            title: series.to_string(),
                        }),
                    }),
                ),

                (None, Some(_)) => unreachable!("I don't think this can happen"),
                (None, None) => CollectionHint::season(title.to_string(), season_num, None),
            }
        } else if next == preserved_title {
            if let Some(after_that) = directories.next() {
                CollectionHint::movie(
                    original_title.to_string(),
                    Some(Franchise {
                        title: after_that.to_string(),
                    }),
                )
            } else {
                CollectionHint::movie(
                    original_title.to_string(),
                    Some(Franchise {
                        title: original_title.to_string(),
                    }),
                )
            }
        } else if original_title.starts_with(&*next) {
            CollectionHint::movie(
                original_title.to_string(),
                Some(Franchise {
                    title: next.to_string(),
                }),
            )
        } else {
            CollectionHint::None
        }
    } else {
        CollectionHint::None
    };

    Ok(hint)
}

fn strip_year(title: &str) -> (&str, Option<u32>) {
    let Some((left, right)) = title.rsplit_once('(') else {
        return (title, None);
    };

    if let Ok(year) = right.parse_until(')') {
        return (left, Some(year));
    }

    (title, None)
}

struct Info {
    season: Option<u64>,
    episode: Option<u64>,
    part: Option<u64>,
}

fn strip_info(title: &str) -> (&str, Info) {
    let Some((begin, metadata)) = title.rsplit_once('-') else {
        return (
            title,
            Info {
                season: None,
                episode: None,
                part: None,
            },
        );
    };

    let (mut season, mut episode, mut part) = (None, None, None);

    [('s', &mut season), ('e', &mut episode), ('p', &mut part)].map(|(delim, var)| {
        metadata
            .parse_between(delim, |c: char| !c.is_ascii_digit())
            .map(|num| *var = Some(num))
            .ignore();
    });

    (
        begin.trim_end(),
        Info {
            season,
            episode,
            part,
        },
    )
}

// Get the series data for a collection that contains that season
fn get_series_with_collection(db: &Connection, collection_id: u64) -> AppResult<Option<Series>> {
    let mut get_info = db.prepare_cached(
        "
    SELECT collection.id, series.title
    FROM collection, collection_contains, series
    WHERE collection.id = collection_contains.collection_id
    AND collection_contains.type = ?1 AND collection_contains.reference = ?2 AND collection.type = ?3
    AND collection.reference = series.id
    ",
    )?;

    let info = get_info
        .query_row_into::<(u64, String)>(params![
            TableId::Collection,
            collection_id,
            CollectionType::Series
        ])
        .optional()?;

    if let Some((id, title)) = info {
        let franchise = get_franchise_with_collection(db, id)?;
        Ok(Some(Series { title, franchise }))
    } else {
        Ok(None)
    }
}

// Get the franchise data for the franchise that contains that collection
fn get_franchise_with_collection(
    db: &Connection,
    collection_id: u64,
) -> AppResult<Option<Franchise>> {
    let mut get_info = db.prepare_cached(
        "
    SELECT franchise.title
    FROM collection, collection_contains, franchise
    WHERE collection.id = collection_contains.collection_id
    AND collection_contains.type = ?1 AND collection_contains.reference = ?2 AND collection.type = ?3
    AND collection.reference = franchise.id
    ",
    )?;

    let info = get_info
        .query_row_get(params![
            TableId::Collection,
            collection_id,
            CollectionType::Franchise
        ])
        .optional()?;

    Ok(info.map(|title| Franchise { title }))
}
