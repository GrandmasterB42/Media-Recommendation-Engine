use askama::Template;
use askama_axum::IntoResponse;
use axum::{debug_handler, extract::Query};
use serde::Deserialize;

use crate::{
    state::AppResult,
    utils::{
        frontend_redirect,
        templates::{Homepage, Index},
        HXTarget,
    },
};

#[derive(Debug, Deserialize)]
#[serde(untagged)]
pub enum Location {
    Content { content: String },
    All { all: String },
}

#[debug_handler]
pub async fn homepage(location: Option<Query<Location>>) -> AppResult<impl IntoResponse> {
    let mut body_html = Homepage {
        redirect_library: &frontend_redirect("/library", HXTarget::Content),
        redirect_explore: &frontend_redirect("/explore", HXTarget::Content),
        redirect_settings: &frontend_redirect("/settings", HXTarget::All),
        content: HXTarget::Content.as_str(),
        route: "",
    };

    let body = if let Some(Query(location)) = location {
        match location {
            Location::Content { content } => {
                body_html.route = &content;
                body_html.render()
            }
            Location::All { all } => Ok(format!(
                r#"<div hx-trigger="load" {redirect}> </div>"#,
                redirect = frontend_redirect(&all, HXTarget::All)
            )),
        }
    } else {
        body_html.route = "/library";
        body_html.render()
    }?;

    Ok(Index {
        body,
        all: HXTarget::All.as_str().to_owned(),
    }
    .into_response())
}
