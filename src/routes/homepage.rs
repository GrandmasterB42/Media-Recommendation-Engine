use axum::{
    extract::Query,
    response::{Html, IntoResponse},
};
use serde::Deserialize;

use crate::utils::{frontend_redirect, frontend_redirect_explicit};

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

pub async fn homepage(location: Option<Query<Location>>) -> impl IntoResponse {
    fn default_body(route: &str) -> String {
        format!(
            r##"
<link rel="stylesheet" href="/styles/library_heading.css">
<div class="heading_box">
    <h4 class="main_title"> Media Recommendation Engine </h4>
    <div class="center_container">
        <input checked id="radioLib" type="radio" name="section" {redirect_library}>
        <label for="radioLib"> Library </label>

        <input id="radioExp" type="radio" name="section" {redirect_explore}>
        <label for="radioExp"> Explore </label>

        <input id="radioSet" type="radio" name="section">
        <label for="radioSet" {redirect_settings}> Settings </label>
    </div>
    <div class="logout_container">
        <input id="logout" class="hidden" type="button" title="logout">
        <label id="logout-label" for="logout"> Logout </label>
    </div>
</div>
<div id="{content}" hx-trigger="load" hx-get="{route}"> </div>"##,
            redirect_library = frontend_redirect("/library", HXTarget::Content),
            redirect_explore = frontend_redirect("/explore", HXTarget::Content),
            redirect_settings = frontend_redirect("/settings", HXTarget::All),
            content = HXTarget::Content.as_str(),
        )
    }

    let body = if let Some(Query(location)) = location {
        match location {
            Location::Err { err } => format!(
                r#"
<link rel="stylesheet" href="/styles/error.css"/>
<h1 class="error_title"> Error: {err} </h1>
<h1 hx-trigger="load delay:750ms" {redirect} class="error_title">
        Seems like something went wrong, redirecting...
</h1>"#,
                redirect = frontend_redirect_explicit("/", &HXTarget::All, "/")
            ),
            Location::Content { content } => default_body(&content),
            Location::All { all } => {
                format!(
                    r#"<div hx-trigger="load" {redirect}> </div>"#,
                    redirect = frontend_redirect(&all, HXTarget::All)
                )
            }
        }
    } else {
        default_body("/library")
    };

    // The htmx-config here is a workaround for https://github.com/bigskysoftware/htmx/issues/497
    Html(format!(
        r##"
<!DOCTYPE html>
<html lang="en">

<script src="/htmx"> </script>
<script src="/htmx_ws"> </script>

<head>
    <meta charset="utf-8">
    <meta name="viewport" content="width=device-width, initial-scale=1.0">
    
    <meta name="htmx-config"
        content='{{"historyCacheSize": 0, "refreshOnHistoryMiss": false}}'>
    <link rel="stylesheet" href="/styles/default.css">
    <title> Media Recommendation Engine </title>
</head>

<body id="{all}">
    {body}
</body>

</html>"##,
        all = HXTarget::All.as_str()
    ))
}
