use askama::Template;
use askama_axum::IntoResponse;

#[derive(Template)]
#[template(path = "../frontend/content/explore.html")]
struct ExploreTemplate;

pub async fn explore() -> impl IntoResponse {
    ExploreTemplate
}
