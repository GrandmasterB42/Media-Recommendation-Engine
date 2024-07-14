use std::convert::Infallible;

use axum::{
    extract::{Path, State},
    response::{
        sse::{Event, KeepAlive},
        IntoResponse, Sse,
    },
    routing::get,
    Router,
};

use futures_util::{Stream, StreamExt};
use rusqlite::params;
use serde::Deserialize;
use tokio_stream::wrappers::WatchStream;

use crate::{
    database::{
        Connection, Database, QueryRowGetConnExt, QueryRowIntoConnExt, QueryRowIntoStmtExt,
    },
    indexing::{resolve_video, CollectionType, ContentType, TableId},
    state::{AppError, AppResult, AppState, Shutdown},
    utils::{
        frontend_redirect, frontend_redirect_explicit,
        streaming::StreamingSessions,
        templates::{GridElement, LargeImage, Library, PreviewTemplate},
        HXTarget,
    },
};

pub fn library() -> Router<AppState> {
    Router::new()
        .route("/library", get(get_library))
        .route("/sessions", get(stream_sessions))
        .route("/preview/:preview/:id", get(preview))
}

async fn get_library(State(db): State<Database>) -> AppResult<impl IntoResponse> {
    let conn = db.get()?;

    let franchises = conn
        .prepare("SELECT collection.id, franchise.title FROM collection, franchise WHERE collection.reference = franchise.id AND collection.type = ?1")?
        .query_map_into([CollectionType::Franchise])?
        .collect::<Result<Vec<(u64, String)>, _>>()?
        .iter()
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

    Ok(Library { franchises })
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
) -> AppResult<Vec<(&'static str, Vec<GridElement>)>> {
    fn inner(
        conn: &Connection,
        id: u64,
        prev: Preview,
    ) -> AppResult<Vec<(&'static str, Vec<GridElement>)>> {
        match prev {
            Preview::Franchise => {
                let movies: Vec<(String, u64)> = conn
                    .prepare("SELECT movie.title, movie.id FROM movie, collection, collection_contains, content
                                WHERE content.reference = movie.id
                                AND content.type = ?1
                                AND collection.id = collection_contains.collection_id
                                AND collection.type = ?2
                                AND collection_contains.collection_id = ?3
                                AND collection_contains.type = ?4
                                AND collection_contains.reference = content.id")?
                    .query_map_into(params![ContentType::Movie, CollectionType::Franchise, id, TableId::Content])?
                    .collect::<Result<_, _>>()?;

                let series: Vec<(u64, String)> = conn
                    .prepare("SELECT collection.id, series.title FROM series, collection, collection_contains
                                WHERE collection.reference = series.id
                                AND collection.type = ?1
                                AND collection_contains.collection_id = ?2
                                AND collection_contains.type = ?3
                                AND collection_contains.reference = collection.id")?
                    .query_map_into(params![CollectionType::Series, id, TableId::Collection])?
                    .collect::<Result<_, _>>()?;

                let mut out = Vec::new();
                let movies = match movies.len() {
                    0 => Vec::new(),
                    1.. => {
                        let items = movies
                            .into_iter()
                            .map(|(title, movie_id)| {
                                let video_id = resolve_video(conn, movie_id, ContentType::Movie)?;
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
                            .collect::<AppResult<Vec<GridElement>>>()?;
                        vec![("<h1> Movies </h1>", items)]
                    }
                };
                out.extend(movies);

                let series = match series.len() {
                    0 => Vec::new(),
                    1 => inner(conn, series[0].0, Preview::Series)?,
                    2.. => {
                        let items = series
                            .into_iter()
                            .map(|(series_id, title)| GridElement {
                                title,
                                redirect_entire: frontend_redirect(
                                    &format!("/preview/Series/{series_id}"),
                                    HXTarget::Content,
                                ),
                                redirect_img: String::new(),
                                redirect_title: String::new(),
                            })
                            .collect::<Vec<GridElement>>();
                        vec![("<h1> Series </h1>", items)]
                    }
                };
                out.extend(series);

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
                    2.. => {
                        let items = conn.prepare(
                            "SELECT collection.id, season.title, season.season FROM season, collection_contains, collection
                                WHERE collection_contains.collection_id = ?1
                                AND collection_contains.type = ?2
                                AND collection.type = ?3
                                AND collection_contains.reference = collection.id
                                AND collection.reference = season.id
                                ORDER BY season.season ASC")?   
                    .query_map_into(params![id, TableId::Collection, CollectionType::Season])?
                    .collect::<Result<Vec<(u64, Option<String>, u64)>, _>>()?
                    .into_iter()
                    .map(|(season_id, title, season)| {
                            let title = title.unwrap_or(format!("Season {season}"));
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
                        Ok(vec![("<h2> Seasons </h2>", items)])
                    }
                }
            }
            Preview::Season => {
                let items = conn.prepare(
                    "SELECT episode.id, episode.title, episode.episode FROM episode, collection, collection_contains, content
                    WHERE content.reference = episode.id
                    AND content.type = ?4
                    AND collection.type = ?1
                    AND collection.id = collection_contains.collection_id
                    AND collection_contains.collection_id = ?2
                    AND collection_contains.type = ?3
                    AND collection_contains.reference = content.id
                    ORDER BY episode.episode ASC")?
                .query_map_into(params![CollectionType::Season, id, TableId::Content, ContentType::Episode])?
                .collect::<Result<Vec<(u64, String, u64,)>, _>>()?
                .into_iter()
                .map(|(data_id, name, episode)| {
                    let name = format!("{name} - Episode {episode}");
                    let video_id = resolve_video(conn, data_id, ContentType::Episode)?;
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
                .collect::<Result<Vec<GridElement>, AppError>>()?;
                Ok(vec![("<h2> Episodes </h2>", items)])
            }
            Preview::Episode | Preview::Movie => Ok(Vec::new()),
        }
    }

    let conn = db.get()?;
    inner(&conn, id, prev)
}

/*
<dialog open="" style="
    background-color: transparent;
    border-color: transparent;
    right: 0px;
    margin-right: 0;
    position: fixed;
"> <audio src="/content/13" autoplay="" controls="" loop=""> </audio></dialog>
*/
