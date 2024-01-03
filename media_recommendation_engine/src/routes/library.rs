use axum::{
    extract::{Path, State},
    response::{Html, IntoResponse},
    routing::get,
    Router,
};

use macros::template;
use serde::Deserialize;

use crate::{
    database::{
        Connection, Database, DatabaseResult, QueryRowGetConnExt, QueryRowIntoConnExt,
        QueryRowIntoStmtExt,
    },
    routes::HXTarget,
    state::AppState,
    templating::{Template, TemplatingEngine},
    utils::frontend_redirect,
};

use super::StreamingSessions;

pub fn library() -> Router<AppState> {
    Router::new()
        .route("/library", get(get_library))
        .route("/preview/:preview/:id", get(preview))
}

async fn get_library(
    State(sessions): State<StreamingSessions>,
    State(db): State<Database>,
    State(templating): State<TemplatingEngine>,
) -> DatabaseResult<impl IntoResponse> {
    let conn = db.get()?;

    template!(
        html,
        templating,
        "../frontend/content/library.html",
        LibraryTarget
    );
    template!(
        grid_element,
        templating,
        "../frontend/content/grid_element.html",
        GElement
    );

    let sessions = sessions
        .sessions
        .lock()
        .await
        .iter()
        .map(|(id, _session)| {
            grid_element.render_only_with(&[
                (
                    frontend_redirect(&format!("/video/session/{id}"), HXTarget::All),
                    GElement::RedirectEntire,
                ),
                (format!("session {id}"), GElement::Title),
                (format!("{id}"), GElement::DisplayTitle),
            ])
        })
        .collect::<Vec<_>>();

    if !sessions.is_empty() {
        html.insert(&[(sessions.as_slice(), LibraryTarget::Sessions)])
    }

    let franchises = conn
        .prepare("SELECT id, title FROM franchise")?
        .query_map_into([])?
        .collect::<Result<Vec<(u64, String)>, _>>()?
        .iter()
        .map(|(id, title)| {
            grid_element.render_only_with(&[
                (
                    frontend_redirect(&format!("/preview/Franchise/{id}"), HXTarget::Content),
                    GElement::RedirectEntire,
                ),
                (title.clone(), GElement::Title),
                (title.clone(), GElement::DisplayTitle),
            ])
        })
        .collect::<Vec<_>>();

    html.insert(&[(franchises.as_slice(), LibraryTarget::Franchises)]);

    Ok(Html(html.render()))
}

#[derive(Debug, Deserialize)]
enum Preview {
    Franchise,
    Movie,
    Series,
    Season,
    Episode,
}

async fn preview(
    State(db): State<Database>,
    State(templating): State<TemplatingEngine>,
    Path((prev, id)): Path<(Preview, u64)>,
) -> DatabaseResult<impl IntoResponse> {
    template!(
        preview,
        templating,
        "../frontend/content/preview.html",
        PreviewTarget
    );

    preview.insert(&[(
        top_preview(&db, &templating, id, &prev).await?,
        PreviewTarget::Top,
    )]);
    for (category, items) in preview_categories(&db, &templating, id, &prev).await? {
        preview.insert(&[(category, PreviewTarget::Grid)]);
        preview.insert(&[(r#"<div class="gridcontainer">"#, PreviewTarget::Grid)]);
        preview.insert(&[(items.as_slice(), PreviewTarget::Grid)]);
        preview.insert(&[("</div>", PreviewTarget::Grid)]);
    }
    Ok(Html(preview.render()))
}

async fn top_preview(
    conn: &Database,
    templating: &TemplatingEngine,
    id: u64,
    prev: &Preview,
) -> DatabaseResult<String> {
    let conn = &mut conn.get()?;

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
            String::new(),
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
            String::new(),
        ),
        Preview::Season => (season_title(conn, id)?, String::new()),
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

    template!(
        top_preview,
        templating,
        "../frontend/content/large_preview_image.html",
        ImageTarget
    );

    Ok(top_preview.render_only_with(&[
        (image_interaction, ImageTarget::ImageInteraction),
        (name, ImageTarget::Title),
    ]))
}

async fn preview_categories(
    db: &Database,
    templating: &TemplatingEngine,
    id: u64,
    prev: &Preview,
) -> DatabaseResult<Vec<(&'static str, Vec<String>)>> {
    let conn = &mut db.get()?;
    template!(
        grid_element,
        templating,
        "../frontend/content/grid_element.html",
        GElement
    );

    return inner(conn, id, prev, grid_element);

    fn inner(
        conn: Connection,
        id: u64,
        prev: &Preview,
        template: Template<GElement>,
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
                                Ok(template.render_only_with(&[
                                    (
                                        frontend_redirect(
                                            &format!("/video/{video_id}"),
                                            HXTarget::All,
                                        ),
                                        GElement::RedirectImg,
                                    ),
                                    (
                                        frontend_redirect(
                                            &format!("/preview/Movie/{id}"),
                                            HXTarget::Content,
                                        ),
                                        GElement::RedirectTitle,
                                    ),
                                    (name.clone(), GElement::Title),
                                    (name.clone(), GElement::DisplayTitle),
                                ]))
                            })
                            .collect::<DatabaseResult<Vec<String>>>()?;
                        vec![("<h1> Movies </h1>", items)]
                    }
                };
                out.extend(movies);

                let series = match series.len() {
                    0 => Vec::new(),
                    1 => inner(conn, series[0].0, &Preview::Series, template)?,
                    2.. => {
                        let items = series
                            .into_iter()
                            .map(|(series_id, name)| {
                                template.render_only_with(&[
                                    (
                                        frontend_redirect(
                                            &format!("/preview/Series/{series_id}"),
                                            HXTarget::Content,
                                        ),
                                        GElement::RedirectEntire,
                                    ),
                                    (name.clone(), GElement::Title),
                                    (name.clone(), GElement::DisplayTitle),
                                ])
                            })
                            .collect::<Vec<String>>();
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
                        inner(conn, season_id, &Preview::Season, template)
                    }
                    2.. => {
                        let items = conn.prepare("SELECT id, title, season FROM seasons WHERE seriesid=?1 ORDER BY season ASC")?
                    .query_map_into([id])?
                    .collect::<Result<Vec<(u64, Option<String>, u64)>, _>>()?
                    .into_iter()
                    .map(|(season_id, name, season)| {
                            let name = name.unwrap_or(format!("Season {season}"));
                            template.render_only_with(&[
                            (
                                frontend_redirect(
                                    &format!("/preview/Season/{season_id}"),
                                    HXTarget::Content,
                                ),
                                GElement::RedirectEntire,
                            ),
                            (name.clone(), GElement::Title),
                            (name.clone(), GElement::DisplayTitle),
                        ])
                        }
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
                    template.render_only_with(&[
                        (
                            frontend_redirect(
                                &format!("/video/{videoid}"),
                                HXTarget::All,
                            ),
                            GElement::RedirectImg,
                        ),
                        (
                            frontend_redirect(
                                &format!("/preview/Episode/{id}"),
                                HXTarget::Content,
                            ),
                            GElement::RedirectTitle,
                        ),
                        (name.clone(), GElement::Title),
                        (name.clone(), GElement::DisplayTitle),
                    ])
                })
                .collect::<Vec<String>>();
                Ok(vec![("<h2> Episodes </h2>", items)])
            }
            Preview::Episode | Preview::Movie => Ok(Vec::new()),
        }
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
