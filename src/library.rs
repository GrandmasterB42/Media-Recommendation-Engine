use axum::{
    body::Body,
    extract::Path,
    http::Request,
    response::{Html, IntoResponse},
    routing::get,
    Extension, Router,
};

use serde::Deserialize;
use tower::util::ServiceExt;
use tower_http::services::ServeFile;

use crate::database::{Connection, Database, DatabaseResult};

// TODO: The naming of this file does not match its responsibility, either restructure or rename

pub fn library() -> Router {
    Router::new()
        .route("/library", get(get_library))
        .route(
            "/preview/:preview/:id",
            get(
                |db: Extension<Database>, Path((prev, id)): Path<(Preview, u64)>| async move {
                    preview(db, prev, id)
                },
            ),
        )
        .route(
            "/video/:id",
            get(|Path(id): Path<u64>| async move {
                Html(format!(
                    r#"
                    <link rel="stylesheet" href="/styles/default.css">
                    <video src=/content/{id} controls autoplay width="100%" height=auto> </video>"#
                ))
            }),
        )
        .route("/content/:id", get(content))
}

// TODO: This will at some point need to be a different streaming solution, probably using ffmpeg or similar
async fn content(
    Path(id): Path<u64>,
    db: Extension<Database>,
    request: Request<Body>,
) -> DatabaseResult<impl IntoResponse> {
    let conn = db.get()?;
    let path: String = conn.query_row("SELECT path FROM data_files WHERE id=?1", [id], |row| {
        row.get(0)
    })?;
    let serve_file = ServeFile::new(path);
    Ok(serve_file.oneshot(request).await)
}

async fn get_library(db: Extension<Database>) -> DatabaseResult<impl IntoResponse> {
    let conn = db.get()?;

    let mut html = String::new();

    let mut stmt = conn.prepare("SELECT id, title FROM franchise")?;
    let franchises = stmt
        .query_map([], |row| Ok((row.get(0)?, row.get(1)?)))?
        .collect::<Result<Vec<(u64, String)>, _>>()?;

    html.push_str(r#"<div class="gridcontainer">"#);
    for (id, title) in franchises {
        html.push_str(&format!(
            r#"<div hx-get="/preview/Franchise/{id}" hx-target=#content class="gridcell">
                    <img width="200" height="300">
                    <a title="{title}" class="name"> {title} </a>
                </div>"#,
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

fn preview(db: Extension<Database>, prev: Preview, id: u64) -> DatabaseResult<impl IntoResponse> {
    let mut conn = db.get()?;
    let mut html = String::new();

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
        let (season_title, season, seriesid): (Option<String>, u64, u64) = conn.query_row(
            "SELECT title, season, seriesid FROM seasons WHERE id=?1",
            [season_id],
            |row| Ok((row.get(0)?, row.get(1)?, row.get(2)?)),
        )?;

        let title = season_title.unwrap_or({
            let series_title: String =
                conn.query_row("SELECT title FROM series WHERE id=?1", [seriesid], |row| {
                    row.get(0)
                })?;
            format!("{series_title} Season {season}")
        });
        Ok(title)
    }

    let (name, image_interaction): (String, String) = match prev {
        Preview::Franchise => (
            conn.query_row("SELECT title FROM franchise WHERE id=?1", [id], |row| {
                row.get(0)
            })?,
            "".to_owned(),
        ),
        Preview::Movie => {
            let (video_id, reference_flag, title): (u64, u64, String) = conn.query_row(
                "SELECT videoid, referenceflag, title FROM movies WHERE id=?1",
                [id],
                |row| Ok((row.get(0)?, row.get(1)?, row.get(2)?)),
            )?;
            let video_id = resolve_video(conn, video_id, reference_flag)?;
            (
                title,
                format!(r#"hx-get="/redirect/video/{video_id}" hx-target=#content"#),
            )
        }
        Preview::Series => (
            conn.query_row("SELECT title FROM series WHERE id=?1", [id], |row| {
                row.get(0)
            })?,
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
            ) = conn.query_row(
                "SELECT title, episode, videoid, referenceflag, seasonid FROM episodes WHERE id=?1",
                [id],
                |row| {
                    Ok((
                        row.get(0)?,
                        row.get(1)?,
                        row.get(2)?,
                        row.get(3)?,
                        row.get(4)?,
                    ))
                },
            )?;

            let season_title = season_title(conn, season_id)?;
            let title = title.unwrap_or(format!("{season_title} - Episode {episode}"));

            let video_id = resolve_video(conn, video_id, reference_flag)?;

            (
                title,
                format!(r#"hx-get="/redirect/video/{video_id}" hx-target=#content"#),
            )
        }
    };

    Ok(format!(
        r#"
    <div style="padding: 15px; display: flex; flex-wrap: wrap; align-items: flex-start; justify-content: flex-start;">
        <img width="250" height="375" {image_interaction}>
        <h1 style="position:relative; left: 30px; text-align: left; flex: 1;"> {name} </h1>
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
            let movies = conn
                .prepare(
                    "SELECT videoid, referenceflag, title, id FROM movies WHERE franchiseid=?1",
                )?
                .query_map([id], |row| {
                    Ok((row.get(0)?, row.get(1)?, row.get(2)?, row.get(3)?))
                })?
                .collect::<Result<Vec<(u64, u64, String, u64)>, _>>()?;

            let series = conn
                .prepare("SELECT series.id, series.title FROM series WHERE franchiseid=?1")?
                .query_map([id], |row| Ok((row.get(0)?, row.get(1)?)))?
                .collect::<Result<Vec<(u64, String)>, _>>()
                .unwrap();

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
                        <img hx-get="/redirect/video/{video_id}" width="200" height="300">
                        <a title="{name}" class="name" hx-get="/preview/Movie/{id}" hx-target=#content> {name} </a>
                    </div>"##,
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
                    <div hx-get="/preview/Series/{series_id}" hx-target="#content" class="gridcell">
                        <img width="200" height="300">
                        <a title="{name}" class="name"> {name} </a>
                    </div>"##,
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
            let season_count: u64 = conn.query_row(
                "SELECT COUNT(*) FROM seasons WHERE seriesid=?1",
                [id],
                |row| row.get(0),
            )?;

            match season_count {
                0 => Ok(Vec::new()),
                1 => {
                    let season_id: u64 =
                        conn.query_row("SELECT id FROM seasons WHERE seriesid=?1", [id], |r| {
                            r.get(0)
                        })?;
                    preview_categories(conn, season_id, &Preview::Season)
                }
                2.. => {
                    let items = conn.prepare("SELECT id, title, season FROM seasons WHERE seriesid=?1 ORDER BY season ASC")?
                    .query_map([id], |row| Ok((row.get(0)?, row.get(1)?, row.get(2)?)))?
                    .collect::<Result<Vec<(u64, Option<String>, u64)>, _>>()?
                    .into_iter()
                    .map(|(season_id, name, season)| {
                        let name = name.unwrap_or(format!("Season {season}"));
                        format!(
                        r##"
                            <div hx-get="/preview/Season/{season_id}" hx-target="#content" class="gridcell">
                                <img width="200" height="300">
                                <a title="{name}" class="name"> {name} </a>
                            </div>
                        "##,
                        )}
                    ).collect::<Vec<String>>();
                    Ok(vec![("<h2> Seasons </h2>", items)])
                }
            }
        }
        Preview::Season => {
            let items = conn.prepare("SELECT videoid, title, episode, id FROM episodes WHERE seasonid=?1 AND referenceflag = 0 ORDER BY episode ASC")?
                .query_map([id], |row| Ok((row.get(0)?, row.get(1)?, row.get(2)?, row.get(3)?)))?
                .collect::<Result<Vec<(u64, Option<String>, u64, u64)>, _>>()?
                .into_iter()
                .map(|(videoid, name, episode, id)| {
                    let name = name.unwrap_or(format!("Episode {episode}"));
                    format!(
                        r##"
                <div class="gridcell">
                    <img width="200" height="300" hx-get="/redirect/video/{videoid}">
                    <a title="{name}" class="name" hx-get="/preview/Episode/{id}" hx-target=#content> {name} </a>
                </div>
                "##,
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
        Ok(conn.query_row(
            "SELECT videoid FROM multipart WHERE id = ?1 AND part = 1",
            [video_id],
            |row| row.get(0),
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
