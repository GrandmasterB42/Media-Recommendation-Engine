use askama::Template;
use askama_axum::IntoResponse;
use axum::{debug_handler, extract::Query};
use serde::Deserialize;

use crate::{
    state::AppResult,
    utils::{frontend_redirect, frontend_redirect_explicit},
};

#[derive(Debug, Deserialize)]
#[serde(untagged)]
pub enum Location {
    Err { err: String },
    Content { content: String },
    All { all: String },
}

#[derive(Clone, Copy)]
pub enum HXTarget {
    All,
    Content,
}

impl HXTarget {
    pub const fn as_str(&self) -> &'static str {
        match self {
            HXTarget::All => "all",
            HXTarget::Content => "content",
        }
    }

    pub const fn as_target(&self) -> &'static str {
        match self {
            HXTarget::All => "#all",
            HXTarget::Content => "#content",
        }
    }
}

#[derive(Template)]
#[template(path = "../frontend/content/index.html")]
pub struct Index {
    pub body: String,
    pub all: String,
}

#[derive(Template)]
#[template(path = "../frontend/content/homepage.html")]
struct Homepage<'a> {
    redirect_library: &'a str,
    redirect_explore: &'a str,
    redirect_settings: &'a str,
    content: &'a str,
    route: &'a str,
}

#[derive(Template)]
#[template(path = "../frontend/content/error.html")]
struct Error<'a> {
    err: &'a str,
    redirect: &'a str,
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
            Location::Err { err } => Error {
                err: &err,
                redirect: &frontend_redirect_explicit("/", HXTarget::All, Some("/")),
            }
            .render(),
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
