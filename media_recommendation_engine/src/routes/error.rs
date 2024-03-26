use askama::Template;
use askama_axum::IntoResponse;
use axum::extract::Query;
use serde::Deserialize;

use crate::{
    state::AppResult,
    utils::{
        frontend_redirect_explicit,
        templates::{Error, Index},
        HXTarget,
    },
};

#[derive(Deserialize)]
pub struct Err {
    err: String,
}

pub async fn error(Query(err): Query<Err>) -> AppResult<impl IntoResponse> {
    let body = Error {
        err: &err.err,
        redirect: &frontend_redirect_explicit("/", HXTarget::All, Some("/")),
    }
    .render()?;

    Ok(Index {
        body,
        all: HXTarget::All.as_str().to_owned(),
    })
}
