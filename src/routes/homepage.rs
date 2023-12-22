use axum::{
    extract::Query,
    response::{Html, IntoResponse},
};
use serde::Deserialize;

#[derive(Debug, Deserialize)]
#[serde(untagged)]
pub enum Location {
    Err { err: String },
    Content { content: String },
}

pub async fn homepage(location: Option<Query<Location>>) -> impl IntoResponse {
    fn default_body(route: &str) -> String {
        format!(
            r##"<div style="text-align: center; display: flex;">
    <h4
        style="color: #999; position: absolute; left: 0.5%; text-align:left; width: 20%; vertical-align: middle; margin: 0;">
        Media Recommendation Engine</h4>
    <div style="display: inline-block; margin: 20px;">
        <input checked id="radioLib" type="radio" name="section" hx-get="/library" hx-swap="innerHTML"
            hx-target="#content">
        <label for="radioLib"> Library </label>

        <input id="radioExp" type="radio" name="section" hx-get="/explore" hx-swap="innerHTML" hx-target="#content">
        <label for="radioExp"> Explore </label>

        <input id="radioSet" type="radio" name="section">
        <label for="radioSet" hx-get="/settings" hx-target=#all> Settings </label>
    </div>
    <div style="display: inline-block; position:absolute; right: 0.5%; margin: 20px 0;">
        <input id="logout" type="button" title="logout" style="display: none;">
        <label id="logout-label" for="logout"> Logout </label>
    </div>
</div>
<div id="content" hx-trigger="load" hx-swap="innerHTML" hx-get="{route}"> </div>"##
        )
    }

    let body =  {
        if let Some(Query(location)) = location {
            match location {
                Location::Err{err} => {
                    Some(format!(r#"
<h1 style="text-align: center; margin-top: 5%;"> Error: {err} </h1>
<h1 hx-trigger="load delay:750ms" hx-get="/" hx-target=#all hx-swap="innerHTML" hx-push-url="/" style="text-align: center; margin-top: 5%;">
        Seems like something went wrong, redirecting...
</h1>"#)
                    )},
                Location::Content{content} => Some(default_body(&content)),
            }
        } else {None}
    }.unwrap_or(default_body("/library"));

    Html(format!(
        r##"
        <!DOCTYPE html>
<html lang="en">

<script src="/htmx"> </script>

<head>
    <meta charset="utf-8">
    <meta name="viewport" content="width=device-width, initial-scale=1.0">
    <link rel="stylesheet" href="/styles/default.css">
    <title> Media Recommendation Engine </title>
</head>

<body id="all">
    {body}
</body>

</html>"##
    ))
}
