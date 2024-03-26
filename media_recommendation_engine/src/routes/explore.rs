use askama_axum::IntoResponse;

use crate::utils::templates::ExploreTemplate;

pub async fn explore() -> impl IntoResponse {
    ExploreTemplate
}
