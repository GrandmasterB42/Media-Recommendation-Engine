use axum::{
    body::Body, extract::Path, http::Request, response::Html, routing::get, Extension, Router,
};
use tower::util::ServiceExt;
use tower_http::services::ServeFile;

use crate::database::{Connection, Database};

// TODO: The naming of this file does not match its responsibility, either restructure or rename

pub fn library() -> Router {
    Router::new().route("/library", get(|db: Extension<Database>| async move {
        db.run(|conn: Connection| {
            let mut html = String::new();

            let mut stmt = conn.prepare("SELECT id, title FROM franchise")?;
            let franchises = stmt
                .query_map([], |row| Ok((row.get(0)?, row.get(1)?)))?
                .collect::<Result<Vec<(u64, String)>, _>>()?;

            html.push_str(r#"<div class="gridcontainer">"#);
            for (id, title) in franchises {
                html.push_str(&format!(
                    r#"<div hx-get="/preview/franchise/{id}" hx-target=#content class="gridcell">
                    <img width="200" height="300">
                    <a title="{title}" class="name"> {title} </a>
                    </div>"#,
                ));
            }

            Ok(Html(format!("<div> {html} </div>")))
        })
    }))
    // TODO: These all need to short-circuit to the next layer down if there isn't enough content, will not nother with it for franchise for now
        .route(
            "/preview/franchise/:id",
            get(|Path(id): Path<u64>, db: Extension<Database>| async move {
                db.run(move |conn: Connection| {
                    let mut stmt = conn.prepare("SELECT videoid, referenceflag, title FROM movies WHERE franchiseid=?1")?;
                    let movies = stmt
                        .query_map([id], |row| {
                            Ok((row.get(0)?, row.get(1)?, row.get(2)?))
                        })?
                        .collect::<Result<Vec<(u64, u64, String)>, _>>()?;

                    let mut stmt = conn.prepare("SELECT series.id, series.title FROM series WHERE franchiseid=?1")?;
                    let series = stmt
                        .query_map([id], |row| Ok((row.get(0)?, row.get(1)?)))?
                        .collect::<Result<Vec<(u64, String)>, _>>()
                        .unwrap();

                    let franchise_name: String = conn.query_row("SELECT title FROM franchise WHERE id=?1", [id], |row| row.get(0))?;

                    let mut html = String::new();

                    html.push_str(&format!(
                        r#"
                    <div style="padding: 15px; display: flex; flex-wrap: wrap; align-items: flex-start; justify-content: flex-start;">
                        <img width="250" height="375">
                        <h1 style="position:relative; left: 30px; text-align: left; flex: 1;"> {franchise_name} </h1>
                    </div>
                    "#,
                    ));

                    if !movies.is_empty() {
                        html.push_str("<h1> Movies </h1>");
                        html.push_str(r#"<div class="gridcontainer">"#);
                        for (mut id, reference_flag,  name) in movies {
                            if reference_flag == 1 {
                                id = conn.query_row("SELECT videoid FROM multipart WHERE id = ?1 AND part = 1", [id], |row| row.get(0))?;
                            }
                            html.push_str(&format!(
                            r#"<div hx-get="/redirect/video/{id}" hx-target=#content class="gridcell">
                            <img width="200" height="300">
                            <a title="{name}" class="name"> {name} </a>
                            </div>"#,
                        ));
                        }
                        html.push_str("</div>");
                    }

                    if !series.is_empty() {
                        html.push_str("<h1> Series </h1>");
                        html.push_str(r#"<div class="gridcontainer">"#);
                        for (series_id, name) in series {
                            html.push_str(&format!(
                                r##"<div hx-get="/preview/series/{series_id}" hx-target="#content" class="gridcell">
                            <img width="200" height="300">
                            <a title="{name}" class="name"> {name} </a>
                            </div>"##,
                            ));
                        }
                        html.push_str("</div>");
                    }

                    Ok(Html(format!("<div> {html} </div>")))
                })
            }),
        )
        // TODO: Combine the preview and add one for episodes
        .route(
            "/preview/series/:id",
            get(|Path(id): Path<u64>, db: Extension<Database>| async move {
                db.run(move |conn| {
                    let series_name: String =
                        conn.query_row("SELECT title FROM series WHERE id=?1", [id], |row| {
                            row.get(0)
                        })?;

                    let season_count: u64 = conn.query_row(
                        "SELECT COUNT(*) FROM seasons WHERE seriesid=?1",
                        [id],
                        |row| row.get(0),
                    )?;

                    let mut html = String::new();
                    html.push_str(&format!(
                        r#"
                    <div style="padding: 15px; display: flex; flex-wrap: wrap; align-items: flex-start; justify-content: flex-start;">
                        <img width="250" height="375">
                        <h1 style="position:relative; left: 30px; text-align: left; flex: 1;"> {series_name} </h1>
                    </div>
                    "#,
                    ));

                    match season_count {
                        1 => {
                            html.push_str("<h2> Episodes </h2>");
                            let season_id: u64 = conn.query_row("SELECT id FROM seasons WHERE seriesid=?1", [id], |r| r.get(0))?;

                            let mut stmt = conn.prepare("SELECT videoid, title, episode FROM episodes WHERE seasonid=?1 ORDER BY episode ASC")?;
                            let episodes = stmt
                                .query_map([season_id], |row| Ok((row.get(0)?, row.get(1)?, row.get(2)?)))?
                                .collect::<Result<Vec<(u64, Option<String>, u64)>, _>>()
                                .unwrap();

                            html.push_str(r#"<div class="gridcontainer">"#);
                            for (videoid, name, episode) in episodes {
                                let name = name.unwrap_or(format!("Episode {episode}"));
                                html.push_str(&format!(
                                    r##"
                                    <div hx-get="/redirect/video/{videoid}" class="gridcell">
                                        <img width="200" height="300">
                                        <a title="{name}" class="name"> {name} </a>
                                    </div>
                                    "##,
                                ));
                            }
                        },
                        2.. => {
                            html.push_str("<h2> Seasons </h2>");
                            let mut stmt = conn.prepare("SELECT id, title, season FROM seasons WHERE seriesid=?1 ORDER BY season ASC")?;
                            let seasons = stmt
                                .query_map([id], |row| Ok((row.get(0)?, row.get(1)?, row.get(2)?)))?
                                .collect::<Result<Vec<(u64, Option<String>, u64)>, _>>()
                                .unwrap();

                            html.push_str(r#"<div class="gridcontainer">"#);
                            for (season_id, name, season) in seasons {
                                let name = name.unwrap_or(format!("{series_name} {season}"));
                                html.push_str(&format!(
                                    r##"
                                    <div hx-get="/preview/season/{season_id}" hx-target="#content" class="gridcell">
                                        <img width="200" height="300">
                                        <a title="{name}" class="name"> {name} </a>
                                    </div>
                                    "##,
                                ));
                            }
                            html.push_str("</div>");
                        }
                        _ => {}
                    }

                    Ok(Html(format!("<div> {html} </div>")))
                })
            }),
        )
        .route("/preview/season/:id", get(|Path(id): Path<u64>, db : Extension<Database>| async move  {
            db.run(move |conn| {
                let mut html = String::new();

                let (mut season_title, season): (Option<String>, u64) = conn.query_row("SELECT title, season FROM seasons WHERE id=?1", [id], |row| Ok((row.get(0)?, row.get(1)?)))?;

                if season_title.is_none() {
                    let series_id: u64 = conn.query_row("SELECT seriesid FROM seasons WHERE id=?1", [id], |row| row.get(0))?;
                    let series_title: String = conn.query_row("SELECT title FROM series WHERE id=?1", [series_id], |row| row.get(0))?;
                    season_title = Some(format!("{series_title} Season {season}")); 
                }
                let season_title = season_title.unwrap();

                html.push_str(&format!(
                    r#"
                    <div style="padding: 15px; display: flex; flex-wrap: wrap; align-items: flex-start; justify-content: flex-start;">
                        <img width="250" height="375">
                        <h1 style="position:relative; left: 30px; text-align: left; flex: 1;"> {season_title} </h1>
                    </div>
                    "#,
                ));

                html.push_str("<h2> Episodes </h2>");
                let mut stmt = conn.prepare("SELECT videoid, title, episode FROM episodes WHERE seasonid=?1 AND referenceflag = 0 ORDER BY episode ASC")?;
                let episodes = stmt
                    .query_map([id], |row| Ok((row.get(0)?, row.get(1)?, row.get(2)?)))?
                    .collect::<Result<Vec<(u64, Option<String>, u64)>, _>>()?;

                html.push_str(r#"<div class="gridcontainer">"#);
                for (videoid, name, episode) in episodes {
                    let name = name.unwrap_or(format!("Episode {episode}"));
                    html.push_str(&format!(
                        r##"
                        <div hx-get="/redirect/video/{videoid}" class="gridcell">
                            <img width="200" height="300">
                            <a title="{name}" class="name"> {name} </a>
                        </div>
                        "##,
                    ));
                }

                Ok(Html(format!("<div> {html} </div>")))
            })
        }))
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
        // TODO: This will at some point need to be a different streaming solution, probably using ffmpeg or similar
        .route(
            "/content/:id",
            get(
                |Path(id): Path<u64>, db: Extension<Database>, request: Request<Body>| async move {
                    let path: String = db
                        .run(move |conn| {
                            conn.query_row("SELECT path FROM data_files WHERE id=?1", [id], |row| {
                                row.get(0)
                            })
                        })
                        .unwrap();
                    let serve_file = ServeFile::new(path);
                    serve_file.oneshot(request).await
                },
            ),
        )
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
