use askama::Template;
use axum::{
    extract::{Path, State},
    response::IntoResponse,
    routing::get,
    Router,
};

use serde::Deserialize;

use crate::{
    database::{Database, QueryRowGetConnExt, QueryRowIntoConnExt, QueryRowIntoStmtExt},
    routes::HXTarget,
    state::{AppResult, AppState},
    utils::frontend_redirect,
};

use super::StreamingSessions;

pub fn library() -> Router<AppState> {
    Router::new()
        .route("/library", get(get_library))
        .route("/sessions", get(get_sessions))
        .route("/preview/:preview/:id", get(preview))
}

#[derive(Template)]
#[template(path = "../frontend/content/library.html")]
struct Library {
    sessions: Vec<GridElement>,
    franchises: Vec<GridElement>,
}

#[derive(Template)]
#[template(path = "../frontend/content/grid_element.html")]
struct GridElement {
    title: String,
    redirect_entire: String,
    redirect_img: String,
    redirect_title: String,
}

async fn get_library(
    State(sessions): State<StreamingSessions>,
    State(db): State<Database>,
) -> AppResult<impl IntoResponse> {
    let conn = db.get()?;

    let sessions = render_sessions(sessions).await;
    let franchises = conn
        .call(|conn| {
            Ok(conn
                .prepare("SELECT id, title FROM franchise")?
                .query_map_into([])?
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
                .collect::<Vec<_>>())
        })
        .await?;

    Ok(Library {
        sessions,
        franchises,
    })
}

async fn get_sessions(State(sessions): State<StreamingSessions>) -> AppResult<impl IntoResponse> {
    render_sessions(sessions)
        .await
        .iter()
        .map(|el| el.render())
        .collect::<Result<String, _>>()
        .map_err(Into::into)
}

async fn render_sessions(sessions: StreamingSessions) -> Vec<GridElement> {
    sessions
        .get_sessions()
        .await
        .map(|(id, _session)| GridElement {
            title: format!("Session {id}"),
            redirect_entire: frontend_redirect(&format!("/video/session/{id}"), HXTarget::All),
            redirect_img: String::new(),
            redirect_title: String::new(),
        })
        .collect::<Vec<_>>()
}

#[derive(Debug, Clone, Copy, Deserialize)]
enum Preview {
    Franchise,
    Movie,
    Series,
    Season,
    Episode,
}

#[derive(Template)]
#[template(path = "../frontend/content/preview.html")]
struct PreviewTemplate<'a> {
    top: LargeImage,
    categories: Vec<(&'a str, Vec<GridElement>)>,
}

async fn preview(
    State(db): State<Database>,
    Path((prev, id)): Path<(Preview, u64)>,
) -> AppResult<impl IntoResponse> {
    Ok(PreviewTemplate {
        top: top_preview(db.clone(), id, prev).await?,
        categories: preview_categories(&db, id, prev).await?,
    })
}

#[derive(Template)]
#[template(path = "../frontend/content/large_preview_image.html")]
struct LargeImage {
    title: String,
    image_interaction: String,
}

async fn top_preview(conn: Database, id: u64, prev: Preview) -> AppResult<LargeImage> {
    fn season_title(
        conn: &rusqlite::Connection,
        season_id: u64,
    ) -> Result<String, rusqlite::Error> {
        let (season_title, season, seriesid): (Option<String>, u64, u64) = conn.query_row_into(
            "SELECT title, season, seriesid FROM seasons WHERE id=?1",
            [season_id],
        )?;

        let title = season_title.unwrap_or({
            let series_title: String =
                conn.query_row_get("SELECT title FROM series WHERE id=?1", [seriesid])?;
            format!("{series_title} Season {season}")
        });
        Ok(title)
    }

    let conn = conn.get()?;

    let (title, image_interaction) = conn
        .call(
            move |conn: &mut rusqlite::Connection| -> Result<(String, String), tokio_rusqlite::Error> {
                match prev {
                    Preview::Franchise => Ok((
                        conn.query_row_get("SELECT title FROM franchise WHERE id=?1", [id])?,
                        String::new(),
                    )),
                    Preview::Movie => {
                        let (video_id, reference_flag, title): (u64, u64, String) = conn
                            .query_row_into(
                                "SELECT videoid, referenceflag, title FROM movies WHERE id=?1",
                                [id],
                            )?;
                        let video_id = resolve_video(conn, video_id, reference_flag)?;
                        Ok((
                            title,
                            frontend_redirect(&format!("/video/{video_id}"), HXTarget::All),
                        ))
                    }
                    Preview::Series => Ok((
                        conn.query_row_get("SELECT title FROM series WHERE id=?1", [id])?,
                        String::new(),
                    )),
                    Preview::Season => Ok((season_title(conn, id)?, String::new())),
                    Preview::Episode => {
                        let (title, episode, video_id, reference_flag, season_id): (
                Option<String>,
                u64,
                u64,
                u64,
                u64,
            ) = conn.query_row_into(
                "SELECT title, episode, videoid, referenceflag, seasonid FROM episodes WHERE id=?1",
                [id],
            )?;

                        let season_title = season_title(conn, season_id)?;
                        let title = title.unwrap_or(format!("{season_title} - Episode {episode}"));

                        let video_id = resolve_video(conn, video_id, reference_flag)?;

                        Ok((
                            title,
                            frontend_redirect(&format!("/video/{video_id}"), HXTarget::All),
                        ))
                    }
                }
            },
        )
        .await?;

    Ok(LargeImage {
        title,
        image_interaction,
    })
}

async fn preview_categories(
    db: &Database,
    id: u64,
    prev: Preview,
) -> AppResult<Vec<(&'static str, Vec<GridElement>)>> {
    fn inner(
        conn: &rusqlite::Connection,
        id: u64,
        prev: Preview,
    ) -> AppResult<Vec<(&'static str, Vec<GridElement>)>> {
        match prev {
            Preview::Franchise => {
                let movies: Vec<(u64, u64, String, u64)> = conn
                    .prepare(
                        "SELECT videoid, referenceflag, title, id FROM movies WHERE franchiseid=?1",
                    )?
                    .query_map_into([id])?
                    .collect::<Result<_, _>>()?;

                let series: Vec<(u64, String)> = conn
                    .prepare("SELECT series.id, series.title FROM series WHERE franchiseid=?1")?
                    .query_map_into([id])?
                    .collect::<Result<_, _>>()?;

                let mut out = Vec::new();
                let movies = match movies.len() {
                    0 => Vec::new(),
                    1.. => {
                        let items = movies
                            .into_iter()
                            .map(|(video_id, reference_flag, title, id)| {
                                let video_id = resolve_video(conn, video_id, reference_flag)?;
                                Ok(GridElement {
                                    title,
                                    redirect_entire: String::new(),
                                    redirect_img: frontend_redirect(
                                        &format!("/video/{video_id}"),
                                        HXTarget::All,
                                    ),
                                    redirect_title: frontend_redirect(
                                        &format!("/preview/Movie/{id}"),
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
                let season_count: u64 =
                    conn.query_row_get("SELECT COUNT(*) FROM seasons WHERE seriesid=?1", [id])?;

                match season_count {
                    0 => Ok(Vec::new()),
                    1 => {
                        let season_id: u64 =
                            conn.query_row_get("SELECT id FROM seasons WHERE seriesid=?1", [id])?;
                        inner(conn, season_id, Preview::Season)
                    }
                    2.. => {
                        let items = conn.prepare("SELECT id, title, season FROM seasons WHERE seriesid=?1 ORDER BY season ASC")?
                    .query_map_into([id])?
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
                let items = conn.prepare("SELECT videoid, title, episode, id FROM episodes WHERE seasonid=?1 AND referenceflag = 0 ORDER BY episode ASC")?
                .query_map_into([id])?
                .collect::<Result<Vec<(u64, Option<String>, u64, u64)>, _>>()?
                .into_iter()
                .map(|(videoid, name, episode, id)| {
                    let name = name.unwrap_or(format!("Episode {episode}"));
                    GridElement {
                        title: name,
                        redirect_entire: String::new(),
                        redirect_img: frontend_redirect(
                            &format!("/video/{videoid}"),
                            HXTarget::All,
                        ),
                        redirect_title: frontend_redirect(
                            &format!("/preview/Episode/{id}"),
                            HXTarget::Content,
                        ),
                    }
                })
                .collect::<Vec<GridElement>>();
                Ok(vec![("<h2> Episodes </h2>", items)])
            }
            Preview::Episode | Preview::Movie => Ok(Vec::new()),
        }
    }

    let conn = db.get()?;
    conn.call(move |conn| Ok(inner(conn, id, prev))).await?
}

fn resolve_video(
    conn: &rusqlite::Connection,
    video_id: u64,
    reference_flag: u64,
) -> Result<u64, rusqlite::Error> {
    if reference_flag == 1 {
        Ok(conn.query_row_get(
            "SELECT videoid FROM multipart WHERE id = ?1 AND part = 1",
            [video_id],
        )?)
    } else {
        Ok(video_id)
    }
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
