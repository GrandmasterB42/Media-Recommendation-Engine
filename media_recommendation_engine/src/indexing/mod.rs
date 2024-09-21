mod classify;
mod db;
mod file_handling;

use std::{
    collections::HashSet,
    path::{Path, PathBuf},
    time::SystemTime,
};

use classify::{ClassificationCategory, CollectionHint, Franchise, Movie, Season, Series};
use rusqlite::{params, OptionalExtension};
use tracing::{debug, info, span, trace, warn, Level};

use crate::{
    database::{Connection, Database, QueryRowGetConnExt, QueryRowGetStmtExt, QueryRowIntoStmtExt},
    indexing::{
        classify::{classify, Classification},
        file_handling::{scan_dir, HashFile, PathExt},
    },
    state::{AppResult, IndexingTrigger, Shutdown},
    utils::{HandleErr, ServerSettings},
};

pub use db::{CollectionType, ContentType, TableId};
pub use file_handling::AsDBString;

pub async fn periodic_indexing(
    db: Database,
    settings: ServerSettings,
    trigger: IndexingTrigger,
    shutdown: Shutdown,
) {
    span!(Level::DEBUG, "Indexing");
    loop {
        let db = db.clone();
        let task = tokio::task::spawn_blocking(move || {
            indexing(&db).log_err_with_msg("Failed the indexing");
        });

        task.await
            .log_err_with_msg("Failed to wait for indexing task to finish");

        tokio::select! {
            _ = settings.wait_configured_time() => {}
            _ = trigger.notified() => debug!("Started indexing because it was requested"),
            _ = shutdown.cancelled() => return
        }
    }
}

// NOTE: There are some oversights in this entire process. I will iron it out as I use it more
fn indexing(db: &Database) -> AppResult<()> {
    let mut conn = db.get()?;

    let filesystem = conn
        .prepare("SELECT path, recurse FROM storage_locations")?
        .query_map_into::<(String, bool)>([])?
        .filter_map(|res| {
            res.log_warn()
                .map(|(path, recurse)| scan_dir(Path::new(&path), recurse))
        })
        .flatten()
        .collect::<HashSet<PathBuf>>();

    let tx = conn.transaction()?;

    let mut insert_stmt = tx.prepare("INSERT OR IGNORE INTO data_file (path) VALUES (?1)")?;
    for file in &filesystem {
        insert_stmt.execute([file.as_db_string()])?;
    }
    drop(insert_stmt);

    tx.commit()?;
    // All in the database are now a superset of what is in the filesystem

    let (both, only_database): (Vec<_>, Vec<_>) = conn
        .prepare("SELECT id, path from data_file")?
        .query_map_into::<(u64, String)>([])?
        .filter_map(|res| res.log_warn().map(|(id, path)| (id, PathBuf::from(path))))
        .collect::<Vec<_>>()
        .into_iter()
        .partition(|(_, path)| filesystem.contains(path));

    // Delete everything that is only in the database and update unassigned content entries

    let mut delete_stmt = conn.prepare("DELETE FROM data_file WHERE path = ?1 RETURNING id")?;
    let deleted_ids = only_database
        .iter()
        .map(|(_, file)| delete_stmt.query_row_get::<u64>([file.as_db_string()]))
        .collect::<Result<Vec<_>, _>>()?;
    drop(delete_stmt);

    let now = SystemTime::now()
        .duration_since(SystemTime::UNIX_EPOCH)
        .log_err_with_msg("Failed to get current time")
        .unwrap_or_default()
        .as_secs();

    let mut update_stmt =
        conn.prepare("UPDATE content SET data_id = NULL, last_changed = ?1 WHERE data_id = ?2")?;
    for id in deleted_ids {
        update_stmt.execute([now, id])?;
    }
    drop(update_stmt);

    // Seperate out which files have content associated with them
    let mut has_content_stmt = conn.prepare("SELECT CASE WHEN EXISTS (SELECT 1 FROM content LEFT JOIN data_file ON content.data_id = data_file.id WHERE data_file.path = ?1) THEN 1 ELSE 0 END")?;
    let (has_content, mut no_content): (Vec<_>, Vec<_>) =
        both.into_iter().partition(|(_, path)| {
            has_content_stmt
                .query_row_get::<bool>([path.as_db_string()])
                .unwrap_or_default()
        });
    drop(has_content_stmt);

    // Check stuff with valid file paths for changes
    // This aggressively removes anything that changed
    let mut get_content_stmt = conn.prepare("SELECT content.id, content.last_changed FROM content, data_file WHERE content.data_id = data_file.id AND data_file.path = ?1")?;
    for (_, path) in has_content {
        let (content_id, last_changed) =
            get_content_stmt.query_row_into::<(u64, u64)>([path.as_db_string()])?;

        let Some(last_modified) = path.last_modified() else {
            warn!("Failed to get last modified time for {path:?}");
            continue;
        };

        if last_changed == last_modified {
            continue;
        } else {
            //Remove the link between content and data_file and add the content to the no_content vec
            let data_id: u64 = conn
                .prepare_cached("UPDATE content SET data_id = NULL WHERE id = ?1 RETURNING id")?
                .query_row_get([content_id])?;

            let removed_path: String = conn
                .prepare_cached("SELECT path FROM data_file WHERE id = ?1")?
                .query_row_get([data_id])?;

            no_content.push((data_id, PathBuf::from(removed_path)));
        }
    }
    drop(get_content_stmt);

    let len = no_content.len();
    let (mut hashes, mut classifications) = (vec![vec![]; len], Vec::with_capacity(len));

    trace!("Started Hashing");
    // TODO: The hashes need to be computed differently (maybe concurrently or in parallel)
    // Try to reassign unassigned content or just create new content entries
    /*
    hashes.iter_mut().enumerate().for_each(|(i, entry)| {
        trace!("Hashing {:?}", no_content[i].1);
        *entry = no_content[i]
            .1
            .hash_file()
            .log_err_with_msg(&format!("failed to hash file: {:?}", no_content[i].1))
            .unwrap_or_default();
    });*/

    trace!("Started Classifying");
    for (_, path) in &no_content {
        classifications.push(classify(path, &conn));
    }

    let classifications: Vec<Classification> = classifications
        .into_iter()
        .collect::<Result<Vec<_>, _>>()
        .log_err_with_msg("Failed to generate classifications")
        .unwrap_or_default();

    // The path, hash and classification for all data files that don't have valid content
    let info = no_content
        .into_iter()
        .zip(hashes.into_iter().zip(classifications))
        .collect::<Vec<_>>();

    // This tries to, as best as it can, reassign or update anything previously removed
    for ((data_id, path), (hash, classification)) in &info {
        let content_id = conn
            .query_row_get::<u64>("SELECT id FROM content WHERE hash = ?1", [hash])
            .optional()?;

        // This should capture renaming
        if let Some(content_id) = content_id {
            let mut link_content =
                conn.prepare_cached("UPDATE content SET data_id = ?1 WHERE id = ?2")?;
            link_content.execute([data_id, &content_id])?;
        }

        trace!("trying to assign {path:?}");
        // Content Entry

        let reference_id: Option<u64> = match classification.category {
            ClassificationCategory::Other => None,
            ClassificationCategory::Movie => {
                let mut stmt =
                    conn.prepare_cached("INSERT INTO movie (title) VALUES (?1) RETURNING id")?;
                Some(stmt.query_row_get([&classification.title])?)
            }
            ClassificationCategory::Episode { episode } => {
                let mut stmt = conn.prepare_cached(
                    "INSERT INTO episode (title, episode) VALUES (?1, ?2) RETURNING id",
                )?;
                Some(stmt.query_row_get(params![&classification.title, episode])?)
            }
            ClassificationCategory::Song => {
                let mut stmt =
                    conn.prepare_cached("INSERT INTO song (title) VALUES (?1) RETURNING id")?;
                Some(stmt.query_row_get([&classification.title])?)
            }
        };

        let content_id: u64 =  conn.prepare_cached("INSERT INTO content (last_changed, hash, data_id, type, reference, part) VALUES (?1, ?2, ?3, ?4, ?5, ?6) RETURNING id")?.query_row_get(params![
            path.last_modified().unwrap_or_default(),
            hash,
            data_id,
            classification.content_type(),
            reference_id,
            classification.part
        ])?;

        // Collection assignment

        let collection_id: Option<u64> = match &classification.collectionhint {
            CollectionHint::None => {
                warn!("Do not know where to assign this media: {path:?}");
                continue;
            }
            CollectionHint::Franchise(franchise) => {
                Some(get_franchise_collection_or_insert_new(&conn, franchise)?)
            }
            CollectionHint::Series(series) => {
                Some(get_series_collection_or_insert_new(&conn, series)?)
            }
            CollectionHint::Season(season) => {
                Some(get_season_collection_or_insert_new(&conn, season)?)
            }
            CollectionHint::ThemeTarget { .. } => {
                // This is handled later
                continue;
            }
            CollectionHint::Movie(Movie {
                franchise,
                title: _,
            }) => {
                if let Some(franchise) = franchise {
                    Some(get_franchise_collection_or_insert_new(&conn, franchise)?)
                } else {
                    None
                }
            }
        };

        if let Some(collection_id) = collection_id {
            conn.prepare_cached(
            "INSERT INTO collection_contains (collection_id, type, reference) VALUES (?1, ?2, ?3)",
                )?
                .execute(params![collection_id, TableId::Content, content_id])?;
        }
    }

    // Try to find matches for themes after everything is assigned
    for ((data_id, path), (_, classification)) in info {
        let CollectionHint::ThemeTarget { .. } = classification.collectionhint else {
            continue;
        };

        let CollectionHint::ThemeTarget { inner } = classify(&path, &conn)?.collectionhint else {
            continue;
        };

        let Some(collection_id) = get_theme_collection_or_insert_new(&conn, &inner)? else {
            continue;
        };

        let content_id: u64 = conn
            .prepare_cached("SELECT content.id FROM content, data_file WHERE content.data_id = data_file.id AND data_file.id = ?1")? 
            .query_row_get([data_id])?;

        conn.prepare_cached(
            "INSERT INTO collection_contains (collection_id, type, reference) VALUES (?1, ?2, ?3)",
        )?
        .execute(params![collection_id, TableId::Content, content_id])?;
    }

    info!("Finished indexing once");
    Ok(())
}

fn get_franchise_collection_or_insert_new(
    conn: &Connection,
    franchise: &Franchise,
) -> AppResult<u64> {
    let franchise_id = conn
        .prepare_cached("SELECT id FROM franchise WHERE title LIKE ?1")?
        .query_row_get([&franchise.title])
        .optional()?;

    let franchise_id: u64 = match franchise_id {
        Some(id) => id,
        None => conn
            .prepare_cached("INSERT INTO franchise (title) VALUES (?1) RETURNING id")?
            .query_row_get([&franchise.title])?,
    };

    let collection_id = conn
        .prepare_cached("SELECT id FROM collection WHERE reference = ?1 AND type = ?2")?
        .query_row_get(params![franchise_id, CollectionType::Franchise])
        .optional()?;

    let collection_id: u64 = match collection_id {
        Some(id) => id,
        None => conn
            .prepare_cached(
                "INSERT INTO collection (type, reference) VALUES (?1, ?2) RETURNING id",
            )?
            .query_row_get(params![CollectionType::Franchise, franchise_id])?,
    };

    Ok(collection_id)
}

fn get_series_collection_or_insert_new(conn: &Connection, series: &Series) -> AppResult<u64> {
    let series_id: u64 = if let Some(franchise) = &series.franchise {
        let franchise_id = get_franchise_collection_or_insert_new(conn, franchise)?;

        let series_id = conn
            .prepare_cached(
                "
            SELECT collection.id FROM collection, series, collection_contains
            WHERE collection.reference = series.id 
            AND collection.type = ?1
            AND collection_contains.collection_id = ?2 
            AND collection_contains.type = ?3 
            AND collection_contains.reference = collection.id
            AND series.title = ?4",
            )?
            .query_row_get(params![
                CollectionType::Series,
                franchise_id,
                TableId::Collection,
                &series.title
            ])
            .optional()?;

        let series_id: u64 = if let Some(id) = series_id {
            id
        } else {
            let series_id: u64 = conn
                .prepare_cached("INSERT INTO series (title) VALUES (?1) RETURNING id")?
                .query_row_get([&series.title])?;

            let collection_id: u64 = conn
                .prepare_cached(
                    "INSERT INTO collection (type, reference) VALUES (?1, ?2) RETURNING id",
                )?
                .query_row_get(params![CollectionType::Series, series_id])?;

            collection_id
        };

        conn.prepare_cached(
            "INSERT INTO collection_contains (collection_id, type, reference) VALUES (?1, ?2, ?3)",
        )?
        .execute(params![franchise_id, TableId::Collection, series_id])?;

        series_id
    } else {
        let series_id: u64 = conn
            .prepare_cached("INSERT INTO series (title) VALUES (?1) RETURNING id")?
            .query_row_get([&series.title])?;

        conn.prepare_cached(
            "INSERT INTO collection (type, reference) VALUES (?1, ?2) RETURNING id",
        )?
        .query_row_get(params![CollectionType::Season, series_id])?
    };

    Ok(series_id)
}

fn get_season_collection_or_insert_new(conn: &Connection, season: &Season) -> AppResult<u64> {
    let season_id: u64 = if let Some(series) = &season.series {
        let series_id = get_series_collection_or_insert_new(conn, series)?;

        let season_id = conn
            .prepare_cached(
                "
                SELECT collection.id FROM collection, season, collection_contains
                WHERE collection.reference = season.id
                AND collection.type = ?1
                AND collection_contains.collection_id = ?2
                AND collection_contains.type = ?3
                AND collection_contains.reference = collection.id
                AND season.title = ?4
                AND season.season = ?5",
            )?
            .query_row_get(params![
                CollectionType::Season,
                series_id,
                TableId::Collection,
                &season.title,
                season.season
            ])
            .optional()?;

        let season_id: u64 = if let Some(id) = season_id {
            id
        } else {
            let season_id: u64 = conn
                .prepare_cached("INSERT INTO season (title, season) VALUES (?1, ?2) RETURNING id")?
                .query_row_get(params![&season.title, season.season])?;

            let collection_id: u64 = conn
                .prepare_cached(
                    "INSERT INTO collection (type, reference) VALUES (?1, ?2) RETURNING id",
                )?
                .query_row_get(params![CollectionType::Season, season_id])?;

            collection_id
        };

        conn.prepare_cached(
            "INSERT INTO collection_contains (collection_id, type, reference) VALUES (?1, ?2, ?3)",
        )?
        .execute(params![series_id, TableId::Collection, season_id])?;

        season_id
    } else {
        let season_id: u64 = conn
            .prepare_cached("INSERT INTO season (title, season) VALUES (?1, ?2) RETURNING id")?
            .query_row_get(params![&season.title, season.season])?;

        conn.prepare_cached(
            "INSERT INTO collection (type, reference) VALUES (?1, ?2) RETURNING id",
        )?
        .query_row_get(params![CollectionType::Season, season_id])?
    };

    Ok(season_id)
}

fn get_theme_collection_or_insert_new(
    conn: &Connection,
    target: &CollectionHint,
) -> AppResult<Option<u64>> {
    // Themes can only point at existing collections
    let theme_target: Option<(TableId, u64)> = match target {
        CollectionHint::None => None,
        CollectionHint::Movie(Movie {
            title,
            franchise: _,
        }) => conn
            .prepare_cached(
                "
                SELECT content.id FROM content, movie 
                WHERE content.reference = movie.id AND content.type = ?2
                AND movie.title = ?1",
            )?
            .query_row_get(params![title, ContentType::Movie])
            .optional()?
            .map(|content_id| (TableId::Content, content_id)),
        CollectionHint::Franchise(Franchise { title }) => conn
            .prepare_cached(
                "
                SELECT collection.id FROM collection, franchise 
                WHERE collection.reference = franchise.id AND collection.type = ?2
                AND franchise.title = ?1",
            )?
            .query_row_get(params![title, CollectionType::Franchise])
            .optional()?
            .map(|collection_id| (TableId::Collection, collection_id)),
        CollectionHint::Series(Series { title, franchise }) => {
            if let Some(franchise) = franchise {
                let franchise_id: Option<u64> = conn
                    .prepare_cached(
                        "SELECT collection.id FROM collection, franchise 
                    WHERE collection.reference = franchise.id AND collection.type = ?2 
                    AND franchise.title = ?1",
                    )?
                    .query_row_get(params![franchise.title, CollectionType::Franchise])
                    .optional()?;

                match franchise_id {
                    Some(id) => conn
                        .prepare_cached(
                            "SELECT collection.id FROM collection, series, collection_contains
                            WHERE collection.reference = series.id
                            AND collection.type = ?2 
                            AND series.title = ?1
                            AND collection_contains.collection_id = ?3
                            AND collection_contains.type = ?4
                            AND collection_contains.reference = collection.id",
                        )?
                        .query_row_get(params![
                            title,
                            CollectionType::Series,
                            id,
                            TableId::Collection
                        ])
                        .optional()?
                        .map(|collection_id| (TableId::Collection, collection_id)),
                    None => None,
                }
            } else {
                conn.prepare_cached(
                    "SELECT collection.id FROM collection, series 
                    WHERE collection.reference = series.id AND collection.type = ?2 
                    AND series.title = ?1",
                )?
                .query_row_get(params![title, CollectionType::Series])
                .optional()?
                .map(|collection_id| (TableId::Collection, collection_id))
            }
        }
        CollectionHint::Season(Season {
            season,
            title,
            series,
        }) => {
            if let Some(series) = series {
                if let Some(franchise) = &series.franchise {
                    let franchise_id: Option<u64> = conn
                        .prepare_cached(
                            "SELECT collection.id FROM collection, franchise 
                            WHERE collection.reference = franchise.id AND collection.type = ?2 
                            AND franchise.title = ?1",
                        )?
                        .query_row_get(params![franchise.title, CollectionType::Franchise])
                        .optional()?;

                    match franchise_id {
                        Some(id) => conn
                            .prepare_cached(
                                "SELECT collection.id FROM collection, series, collection_contains
                                WHERE collection.reference = series.id
                                AND collection.type = ?2 
                                AND series.title = ?1
                                AND collection_contains.collection_id = ?3
                                AND collection_contains.type = ?4
                                AND collection_contains.reference = collection.id",
                            )?
                            .query_row_get(params![
                                title,
                                CollectionType::Series,
                                id,
                                TableId::Collection
                            ])
                            .optional()?
                            .map(|collection_id| (TableId::Collection, collection_id)),
                        None => None,
                    }
                } else {
                    conn.prepare_cached(
                        "SELECT collection.id FROM collection, series, collection_contains
                            WHERE collection.reference = series.id
                            AND collection.type = ?2 
                            AND series.title = ?1
                            AND collection_contains.collection_id = ?3
                            AND collection_contains.type = ?4
                            AND collection_contains.reference = collection.id",
                    )?
                    .query_row_get(params![
                        title,
                        CollectionType::Series,
                        CollectionType::Season,
                        TableId::Collection
                    ])
                    .optional()?
                    .map(|collection_id| (TableId::Collection, collection_id))
                }
            } else {
                conn.prepare_cached(
                    "SELECT collection.id FROM collection, season WHERE collection.reference = season.id 
                        WHERE season.season = ?1 
                        AND season.title = ?2 
                        AND collection.type = ?3")? 
                    .query_row_get(params![season, title, CollectionType::Season])
                    .optional()?.map(|collection_id| (TableId::Collection, collection_id))
            }
        }
        CollectionHint::ThemeTarget { .. } => unreachable!("This should never be constructed!"),
    };

    let Some((table_id, theme_target)) = theme_target else {
        return Ok(None);
    };

    let theme_id = conn
        .prepare_cached("SELECT theme.id FROM theme WHERE type = ?1 AND theme_target = ?2")?
        .query_row_get(params![table_id, theme_target])
        .optional()?;

    let theme_id = if let Some(id) = theme_id {
        id
    } else {
        let theme_id: u64 = conn
            .prepare_cached("INSERT INTO theme (type, theme_target) VALUES (?1, ?2) RETURNING id")?
            .query_row_get(params![table_id, theme_target])?;
        theme_id
    };

    let collection_id = conn
        .prepare_cached("SELECT id FROM collection WHERE reference = ?1 AND type = ?2")?
        .query_row_get(params![theme_id, CollectionType::Theme])
        .optional()?;

    let collection_id = if let Some(id) = collection_id {
        id
    } else {
        let collection_id: u64 = conn
            .prepare_cached(
                "INSERT INTO collection (type, reference) VALUES (?2, ?1) RETURNING id",
            )?
            .query_row_get(params![theme_id, CollectionType::Theme])?;
        collection_id
    };

    Ok(Some(collection_id))
}

pub fn resolve_video(
    conn: &Connection,
    data_id: u64,
    content_type: ContentType,
) -> Result<u64, rusqlite::Error> {
    conn.query_row_get(
        "SELECT content.id FROM content
            WHERE content.reference = ?1
            AND content.type = ?2
            AND part = 0",
        params![data_id, content_type],
    )
}
