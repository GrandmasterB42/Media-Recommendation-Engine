use std::convert::Infallible;

use axum::{
    extract::{Path, Query, State},
    response::{
        sse::{Event, KeepAlive},
        IntoResponse, Sse,
    },
    routing::get,
    Router,
};

use futures_util::{Stream, StreamExt};
use rusqlite::{params, OptionalExtension};
use serde::Deserialize;
use tokio_stream::wrappers::WatchStream;

use crate::{
    database::{
        Connection, Database, QueryRowGetConnExt, QueryRowGetStmtExt, QueryRowIntoConnExt,
        QueryRowIntoStmtExt,
    },
    indexing::{resolve_video, CollectionType, ContentType, TableId},
    state::{AppError, AppResult, AppState, Shutdown},
    utils::{
        frontend_redirect, frontend_redirect_explicit,
        streaming::StreamingSessions,
        templates::{
            GridElement, LargeImage, Library, LoadNext, PaginationResponse, PreviewTemplate,
        },
        HXTarget,
    },
};

pub fn library() -> Router<AppState> {
    Router::new()
        .route("/library", get(get_library))
        .route("/sessions", get(stream_sessions))
        .route("/preview/:preview/:id", get(preview))
        .route("/library/:preview/:id", get(get_preview_items))
}

#[derive(Deserialize)]
struct Pagination {
    page: u64,
    per_page: u64,
}

async fn get_library() -> AppResult<impl IntoResponse> {
    Ok(Library {
        load_next: LoadNext::new("/library/Franchise/0".to_string(), 0, 20),
    })
}

async fn stream_sessions(
    State(sessions): State<StreamingSessions>,
    State(shutdown): State<Shutdown>,
) -> Sse<impl Stream<Item = Result<Event, Infallible>>> {
    let resolve = |shutdown: Shutdown| async move { shutdown.cancelled().await };
    let stream = WatchStream::new(sessions.render_receiver())
        .map(|content| {
            let content = content.replace('\r', "");
            Ok(Event::default().data(content))
        })
        .take_until(resolve(shutdown));
    Sse::new(stream).keep_alive(KeepAlive::default())
}

#[derive(Debug, Clone, Copy, Deserialize)]
enum Preview {
    Franchise,
    Movie,
    Series,
    Season,
    Episode,
}

async fn preview(
    State(db): State<Database>,
    Path((prev, id)): Path<(Preview, u64)>,
) -> AppResult<impl IntoResponse> {
    Ok(PreviewTemplate {
        top: top_preview(db.clone(), id, prev)?,
        categories: preview_categories(&db, id, prev)?,
    })
}

fn top_preview(conn: Database, id: u64, prev: Preview) -> AppResult<LargeImage> {
    let conn = conn.get()?;

    let (title, image_interaction) = match prev {
        Preview::Franchise => (
            conn.query_row_get(
                "SELECT franchise.title FROM franchise, collection
                WHERE collection.reference = franchise.id
                AND collection.id=?1
                AND collection.type = ?2",
                params![id, CollectionType::Franchise],
            )?,
            String::new(),
        ),
        Preview::Movie => {
            let title: String =
                conn.query_row_get("SELECT movie.title FROM movie WHERE movie.id=?1", [id])?;

            let video_id = resolve_video(&conn, id, ContentType::Movie)?;
            (
                title,
                frontend_redirect_explicit(&format!("/video/{video_id}"), HXTarget::All, None),
            )
        }
        Preview::Series => (
            conn.query_row_get(
                "SELECT series.title FROM series, collection
                    WHERE collection.reference = series.id
                    AND collection.type = ?1
                    AND collection.id = ?2",
                params![CollectionType::Series, id],
            )?,
            String::new(),
        ),
        Preview::Season => {
            let title = conn.query_row_get(
                "SELECT season.title FROM season, collection
                    WHERE collection.reference = season.id
                    AND collection.type = ?1
                    AND collection.id = ?2",
                params![CollectionType::Season, id],
            )?;

            (title, String::new())
        }
        Preview::Episode => {
            let (title, episode): (String, u64) = conn.query_row_into(
                "SELECT episode.title, episode.episode FROM episode WHERE episode.id = ?1",
                [id],
            )?;

            let video_id = resolve_video(&conn, id, ContentType::Episode)?;

            (
                format!("{title} - Episode {episode}"),
                frontend_redirect_explicit(&format!("/video/{video_id}"), HXTarget::All, None),
            )
        }
    };

    Ok(LargeImage {
        title,
        image_interaction,
    })
}

fn preview_categories(
    db: &Database,
    id: u64,
    prev: Preview,
) -> AppResult<Vec<(&'static str, LoadNext)>> {
    fn inner(
        conn: &Connection,
        id: u64,
        prev: Preview,
    ) -> AppResult<Vec<(&'static str, LoadNext)>> {
        let mut out = Vec::new();

        match prev {
            Preview::Franchise => {
                let movie_count: u64 = conn.query_row_get(
                    "SELECT COUNT(*) FROM movie, collection, collection_contains, content
                                WHERE content.reference = movie.id
                                AND content.type = ?1
                                AND collection.id = collection_contains.collection_id
                                AND collection.type = ?2
                                AND collection_contains.collection_id = ?3
                                AND collection_contains.type = ?4
                                AND collection_contains.reference = content.id",
                    params![
                        ContentType::Movie,
                        CollectionType::Franchise,
                        id,
                        TableId::Content
                    ],
                )?;

                if movie_count > 0 {
                    out.push((
                        "<h1> Movies </h1>",
                        LoadNext::new(format!("/library/Movie/{id}"), 0, 20),
                    ));
                }

                let series_ids: Vec<u64> = conn
                    .prepare(
                        "SELECT collection.id FROM series, collection, collection_contains
                            WHERE collection.reference = series.id
                            AND collection.type = ?1
                            AND collection_contains.collection_id = ?2
                            AND collection_contains.type = ?3
                            AND collection_contains.reference = collection.id",
                    )?
                    .query_map_get(params![CollectionType::Series, id, TableId::Collection])?
                    .collect::<Result<Vec<_>, _>>()?;

                match series_ids.len() {
                    0 => {}
                    1 => {
                        let season_load = inner(conn, series_ids[0], Preview::Series)?;
                        out.extend(season_load);
                    }
                    2.. => {
                        out.push((
                            "<h1> Series </h1>",
                            LoadNext::new(format!("/library/Series/{id}"), 0, 20),
                        ));
                    }
                };

                Ok(out)
            }
            Preview::Series => {
                let season_count: u64 = conn.query_row_get(
                    "SELECT COUNT(*) FROM collection_contains, collection
                        WHERE collection_contains.collection_id = ?1
                        AND collection_contains.type = ?2
                        AND collection.type = ?3
                        AND collection_contains.reference = collection.id",
                    params![id, TableId::Collection, CollectionType::Season],
                )?;

                match season_count {
                    0 => Ok(Vec::new()),
                    1 => {
                        let season_id: u64 = conn.query_row_get(
                            "SELECT id FROM collection, collection_contains
                                WHERE collection_contains.collection_id = ?1
                                AND collection_contains.type = ?2
                                AND collection.type = ?3
                                AND collection_contains.reference = collection.id",
                            params![id, TableId::Collection, CollectionType::Season],
                        )?;
                        inner(conn, season_id, Preview::Season)
                    }
                    2.. => Ok(vec![(
                        "<h2> Seasons </h2>",
                        LoadNext::new(format!("/library/Season/{id}"), 0, 20),
                    )]),
                }
            }
            Preview::Season => Ok(vec![(
                "<h2> Episodes </h2>",
                LoadNext::new(format!("/library/Episode/{id}"), 0, 20),
            )]),
            Preview::Episode | Preview::Movie => Ok(Vec::new()),
        }
    }

    let conn = db.get()?;
    inner(&conn, id, prev)
}

async fn get_preview_items(
    State(db): State<Database>,
    Path((returned, id)): Path<(Preview, u64)>,
    Query(pagination): Query<Pagination>,
) -> AppResult<impl IntoResponse> {
    let conn = db.get()?;

    let elements = match returned {
        Preview::Franchise => {
            let franchises = conn
                .prepare(
                    "SELECT collection.id, franchise.title FROM collection, franchise
                        WHERE collection.reference = franchise.id 
                        AND collection.type = ?1
                        ORDER BY franchise.title ASC
                        LIMIT ?2 OFFSET ?3",
                )?
                .query_map_into(params![
                    CollectionType::Franchise,
                    pagination.per_page,
                    pagination.page * pagination.per_page
                ])
                .optional()?
                .map_or_else(
                    || Ok(Vec::new()),
                    |rows| rows.collect::<Result<Vec<(u64, String)>, _>>(),
                )?
                .into_iter()
                .map(|(id, title)| GridElement {
                    title: title.clone(),
                    redirect_entire: frontend_redirect(
                        &format!("/preview/Franchise/{id}"),
                        HXTarget::Content,
                    ),
                    redirect_img: String::new(),
                    redirect_title: String::new(),
                })
                .collect::<Vec<_>>();

            Ok(franchises)
        }
        Preview::Movie => {
            let items = conn
                .prepare(
                    "SELECT movie.title, movie.id FROM movie, collection_contains, content, collection
                        WHERE content.reference = movie.id
                        AND content.type = ?1
                        AND collection.type = ?2
                        AND collection_contains.collection_id = collection.id
                        AND collection_contains.collection_id = ?3
                        AND collection_contains.type = ?4
                        AND collection_contains.reference = content.id
                        ORDER BY movie.title ASC
                        LIMIT ?5 OFFSET ?6",
                )?
                .query_map_into::<(String, u64)>(params![
                    ContentType::Movie,
                    CollectionType::Franchise,
                    id,
                    TableId::Content,
                    pagination.per_page,
                    pagination.page * pagination.per_page
                ])
                .optional()?
                .map_or_else(|| Ok(Vec::new()), |rows| rows.collect())?
                .into_iter()
                .map(|(title, movie_id)| {
                    let video_id = resolve_video(&conn, movie_id, ContentType::Movie)?;
                    Ok(GridElement {
                        title,
                        redirect_entire: String::new(),
                        redirect_img: frontend_redirect_explicit(
                            &format!("/video/{video_id}"),
                            HXTarget::All,
                            None,
                        ),
                        redirect_title: frontend_redirect(
                            &format!("/preview/Movie/{movie_id}"),
                            HXTarget::Content,
                        ),
                    })
                })
                .collect::<AppResult<Vec<_>>>()?;

            Ok::<_, AppError>(items)
        }
        Preview::Series => {
            let items = conn.prepare("SELECT collection.id, series.title FROM series, collection, collection_contains
                        WHERE collection.reference = series.id
                        AND collection.type = ?1
                        AND collection_contains.collection_id = ?2
                        AND collection_contains.type = ?3
                        AND collection_contains.reference = collection.id
                        ORDER BY series.title ASC
                        LIMIT ?4 OFFSET ?5")?
            .query_map_into(params![CollectionType::Series, id, TableId::Collection, pagination.per_page, pagination.page * pagination.per_page])?
            .collect::<Result<Vec<(u64, String)>, _>>()?
            .into_iter()
            .map(|(series_id, title)| {
                GridElement {
                    title,
                    redirect_entire: frontend_redirect(
                        &format!("/preview/Series/{series_id}"),
                        HXTarget::Content,
                    ),
                    redirect_img: String::new(),
                    redirect_title: String::new(),
                }
            })
            .collect::<Vec<GridElement>>();

            Ok(items)
        }
        Preview::Season => {
            let items = conn.prepare(
                        "SELECT collection.id, season.title FROM season, collection_contains, collection
                            WHERE collection_contains.collection_id = ?1
                            AND collection_contains.type = ?2
                            AND collection.type = ?3
                            AND collection_contains.reference = collection.id
                            AND collection.reference = season.id
                            ORDER BY season.season ASC
                            LIMIT ?4 OFFSET ?5")?
                .query_map_into::<(u64, String)>(params![id, TableId::Collection, CollectionType::Season, pagination.per_page, pagination.page * pagination.per_page])
                .optional()?
                .map_or_else(|| Ok(Vec::new()), |rows| rows.collect())?
                .into_iter()
                .map(|(season_id, title)| {
                        GridElement {
                            title,
                            redirect_entire: frontend_redirect(
                                &format!("/preview/Season/{season_id}"),
                                HXTarget::Content,
                            ),
                            redirect_img: String::new(),
                            redirect_title: String::new(),
                        }
                    }
                ).collect::<Vec<GridElement>>();
            Ok(items)
        }
        Preview::Episode => {
            let items = conn.prepare(
                "SELECT episode.id, episode.title, episode.episode FROM episode, collection, collection_contains, content
                WHERE content.reference = episode.id
                AND content.type = ?4
                AND collection.type = ?1
                AND collection.id = collection_contains.collection_id
                AND collection_contains.collection_id = ?2
                AND collection_contains.type = ?3
                AND collection_contains.reference = content.id
                ORDER BY episode.episode ASC
                LIMIT ?5 OFFSET ?6")?
            .query_map_into::<(u64, String, u64)>(params![CollectionType::Season, id, TableId::Content, ContentType::Episode, pagination.per_page, pagination.page * pagination.per_page])
            .optional()?
            .map_or_else(|| Ok(Vec::new()), |rows| rows.collect())?
            .into_iter()
            .map(|(data_id, name, episode)| {
                let name = format!("{name} - Episode {episode}");
                let video_id = resolve_video(&conn, data_id, ContentType::Episode)?;
                Ok(GridElement {
                    title: name,
                    redirect_entire: String::new(),
                    redirect_img: frontend_redirect_explicit(
                        &format!("/video/{video_id}"),
                        HXTarget::All,
                        None,
                    ),
                    redirect_title: frontend_redirect(
                        &format!("/preview/Episode/{data_id}"),
                        HXTarget::Content,
                    ),
                })
            })
            .collect::<AppResult<Vec<_>>>()?;
            Ok(items)
        }
    }?;

    let load_next = if elements.len() < pagination.per_page as usize {
        None
    } else {
        let preview = match returned {
            Preview::Franchise => "Franchise",
            Preview::Movie => "Movie",
            Preview::Series => "Series",
            Preview::Season => "Season",
            Preview::Episode => "Episode",
        };

        Some(LoadNext::new(
            format!("/library/{preview}/{id}"),
            pagination.page + 1,
            pagination.per_page,
        ))
    };

    Ok(PaginationResponse {
        elements,
        load_next,
    })
}
