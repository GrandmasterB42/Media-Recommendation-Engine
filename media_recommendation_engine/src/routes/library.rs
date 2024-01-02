use axum::{
    extract::{Path, State},
    response::{Html, IntoResponse},
    routing::get,
    Router,
};

use serde::Deserialize;

use crate::{
    database::{
        Connection, Database, DatabaseResult, QueryRowGetConnExt, QueryRowIntoConnExt,
        QueryRowIntoStmtExt,
    },
    routes::HXTarget,
    state::AppState,
    utils::frontend_redirect,
};

use super::StreamingSessions;

pub fn library() -> Router<AppState> {
    Router::new().route("/library", get(get_library)).route(
        "/preview/:preview/:id",
        get(
            |State(db): State<Database>, Path((prev, id)): Path<(Preview, u64)>| async move {
                preview(db, prev, id)
            },
        ),
    )
}

async fn get_library(
    State(sessions): State<StreamingSessions>,
    State(db): State<Database>,
) -> DatabaseResult<impl IntoResponse> {
    let conn = db.get()?;

    let mut html = String::new();
    html.push_str(r#"<link href="/styles/library.css" rel="stylesheet"/> "#);

    let sessions = sessions.sessions.lock().await;
    if !sessions.is_empty() {
        html.push_str(r#"<div class="session_heading">"#);
        for (id, _session) in sessions.iter() {
            html.push_str(&format!(
                r#"<div class="gridcell"{redirect_video}>
                    <img width="200" height="300" >
                    <a title="session {id}" class="name"> {id} </a>
                </div>"#,
                redirect_video = frontend_redirect(&format!("/video/session/{id}"), HXTarget::All),
            ));
        }
        html.push_str("</div>");
    }

    let franchises = conn
        .prepare("SELECT id, title FROM franchise")?
        .query_map_into([])?
        .collect::<Result<Vec<(u64, String)>, _>>()?;

    html.push_str(r#"<div class="gridcontainer">"#);
    for (id, title) in franchises {
        html.push_str(&format!(
            r#"<div {redirect} class="gridcell">
                    <img width="200" height="300">
                    <a title="{title}" class="name"> {title} </a>
                </div>"#,
            redirect = frontend_redirect(&format!("/preview/Franchise/{id}"), HXTarget::Content),
        ));
    }

    Ok(Html(format!("<div> {html} </div>")))
}

#[derive(Debug, Deserialize)]
enum Preview {
    Franchise,
    Movie,
    Series,
    Season,
    Episode,
}

fn preview(db: Database, prev: Preview, id: u64) -> DatabaseResult<impl IntoResponse> {
    let mut conn = db.get()?;
    let mut html = String::new();

    html.push_str(r#"<link href="/styles/preview.css" rel="stylesheet"/>"#);
    html.push_str(r#"<link href="/styles/library.css" rel="stylesheet"/>"#);
    html.push_str(&top_preview(&mut conn, id, &prev)?);

    for (category, items) in preview_categories(&mut conn, id, &prev)? {
        html.push_str(category);
        html.push_str(r#"<div class="gridcontainer">"#);
        for item in &items {
            html.push_str(item);
        }
        html.push_str("</div>");
    }

    Ok(Html(format!("<div> {html} </div>")))
}

fn top_preview(conn: Connection, id: u64, prev: &Preview) -> DatabaseResult<String> {
    fn season_title(conn: Connection, season_id: u64) -> DatabaseResult<String> {
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

    let (name, image_interaction): (String, String) = match prev {
        Preview::Franchise => (
            conn.query_row_get("SELECT title FROM franchise WHERE id=?1", [id])?,
            "".to_owned(),
        ),
        Preview::Movie => {
            let (video_id, reference_flag, title): (u64, u64, String) = conn.query_row_into(
                "SELECT videoid, referenceflag, title FROM movies WHERE id=?1",
                [id],
            )?;
            let video_id = resolve_video(conn, video_id, reference_flag)?;
            (
                title,
                frontend_redirect(&format!("/video/{video_id}"), HXTarget::All),
            )
        }
        Preview::Series => (
            conn.query_row_get("SELECT title FROM series WHERE id=?1", [id])?,
            "".to_owned(),
        ),
        Preview::Season => (season_title(conn, id)?, "".to_owned()),
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

            (
                title,
                frontend_redirect(&format!("/video/{video_id}"), HXTarget::All),
            )
        }
    };

    Ok(format!(
        r#"<div class="preview_top">
        <img width="250" height="375" {image_interaction}>
        <h1 class="preview_top_title"> {name} </h1>
    </div>
    "#,
    ))
}

fn preview_categories(
    conn: Connection,
    id: u64,
    prev: &Preview,
) -> DatabaseResult<Vec<(&'static str, Vec<String>)>> {
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
                        .map(|(video_id, reference_flag, name, id)| {
                            let video_id = resolve_video(conn, video_id, reference_flag)?;
                            Ok(format!(
                                r##"
                    <div class="gridcell">
                        <img width="200" height="300" {redirect_video}>
                        <a title="{name}" class="name" {redirect_preview}> {name} </a>
                    </div>"##,
                                redirect_video =
                                    frontend_redirect(&format!("/video/{video_id}"), HXTarget::All),
                                redirect_preview = frontend_redirect(
                                    &format!("/preview/Movie/{id}"),
                                    HXTarget::Content
                                ),
                            ))
                        })
                        .collect::<DatabaseResult<Vec<String>>>()?;
                    vec![("<h1> Movies </h1>", items)]
                }
            };
            out.extend(movies);

            let series = match series.len() {
                0 => Vec::new(),
                1 => preview_categories(conn, series[0].0, &Preview::Series)?,
                2.. => {
                    let items = series
                        .into_iter()
                        .map(|(series_id, name)| {
                            format!(
                                r##"
                    <div {redirect}" class="gridcell">
                        <img width="200" height="300">
                        <a title="{name}" class="name"> {name} </a>
                    </div>"##,
                                redirect = frontend_redirect(
                                    &format!("/preview/Series/{series_id}"),
                                    HXTarget::Content
                                )
                            )
                        })
                        .collect::<Vec<String>>();
                    vec![("<h1> Series </h1>", items)]
                }
            };
            out.extend(series);

            Ok(out)
        }
        Preview::Movie => Ok(Vec::new()),
        Preview::Series => {
            let season_count: u64 =
                conn.query_row_get("SELECT COUNT(*) FROM seasons WHERE seriesid=?1", [id])?;

            match season_count {
                0 => Ok(Vec::new()),
                1 => {
                    let season_id: u64 =
                        conn.query_row_get("SELECT id FROM seasons WHERE seriesid=?1", [id])?;
                    preview_categories(conn, season_id, &Preview::Season)
                }
                2.. => {
                    let items = conn.prepare("SELECT id, title, season FROM seasons WHERE seriesid=?1 ORDER BY season ASC")?
                    .query_map_into([id])?
                    .collect::<Result<Vec<(u64, Option<String>, u64)>, _>>()?
                    .into_iter()
                    .map(|(season_id, name, season)| {
                        let name = name.unwrap_or(format!("Season {season}"));
                        format!(
                        r##"
                            <div class="gridcell" {redirect}>
                                <img width="200" height="300">
                                <a title="{name}" class="name"> {name} </a>
                            </div>
                        "##,
                        redirect = frontend_redirect(
                            &format!("/preview/Season/{season_id}"),
                            HXTarget::Content
                        ))}
                    ).collect::<Vec<String>>();
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
                    format!(
                        r##"
                <div class="gridcell">
                    <img width="200" height="300" {redirect_video}>
                    <a title="{name}" class="name" {redirect_preview}> {name} </a>
                </div>
                "##,
                redirect_video = frontend_redirect(&format!("/video/{videoid}"), HXTarget::All),
                redirect_preview = frontend_redirect(&format!("/preview/Episode/{id}"), HXTarget::Content),
                    )
                })
                .collect::<Vec<String>>();
            Ok(vec![("<h2> Episodes </h2>", items)])
        }
        Preview::Episode => Ok(Vec::new()),
    }
}

fn resolve_video(conn: Connection, video_id: u64, reference_flag: u64) -> DatabaseResult<u64> {
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
