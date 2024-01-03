use axum::{
    extract::{Query, State},
    response::{Html, IntoResponse},
};
use macros::template;
use serde::Deserialize;

use crate::{
    templating::TemplatingEngine,
    utils::{frontend_redirect, frontend_redirect_explicit},
};

#[derive(Debug, Deserialize)]
#[serde(untagged)]
pub enum Location {
    Err { err: String },
    Content { content: String },
    All { all: String },
}

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

pub async fn homepage(
    templating: State<TemplatingEngine>,
    location: Option<Query<Location>>,
) -> impl IntoResponse {
    template!(
        body_html,
        templating,
        "../frontend/content/homepage.html",
        HomepageTarget
    );

    #[rustfmt::skip]
    body_html.insert(&[
        (frontend_redirect("/library", HXTarget::Content), HomepageTarget::RedirectLibrary),
        (frontend_redirect("/explore", HXTarget::Content), HomepageTarget::RedirectExplore),
        (frontend_redirect("/settings", HXTarget::All), HomepageTarget::RedirectSettings),
        (HXTarget::Content.as_str().to_owned(), HomepageTarget::Content),
    ]);

    let body = if let Some(Query(location)) = location {
        match location {
            Location::Err { err } => {
                template!(
                    error,
                    templating,
                    "../frontend/content/error.html",
                    ErrorTarget
                );

                error.insert(&[
                    (err, ErrorTarget::Err),
                    (
                        frontend_redirect_explicit("/", &HXTarget::All, "/"),
                        ErrorTarget::Redirect,
                    ),
                ]);
                error.render()
            }
            Location::Content { content } => {
                body_html.insert(&[(content, HomepageTarget::Route)]);
                body_html.render()
            }
            Location::All { all } => {
                format!(
                    r#"<div hx-trigger="load" {redirect}> </div>"#,
                    redirect = frontend_redirect(&all, HXTarget::All)
                )
            }
        }
    } else {
        body_html.insert(&[("/library", HomepageTarget::Route)]);
        body_html.render()
    };

    template!(
        html,
        templating,
        "../frontend/content/index.html",
        IndexTarget
    );

    Html(html.render_only_with(&[
        (body, IndexTarget::Body),
        (HXTarget::All.as_str().to_owned(), IndexTarget::All),
    ]))
}
