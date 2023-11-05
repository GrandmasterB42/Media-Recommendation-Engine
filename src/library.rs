use axum::{
    body::Body, extract::Path, http::Request, response::Html, routing::get, Extension, Router,
};
use tower::util::ServiceExt;
use tower_http::services::ServeFile;

use crate::database::{Connection, Database};

// TODO: The naming of this file does not match its responsibility, either restructure or rename

pub fn library() -> Router {
    Router::new().route(
        "/library",
        get(|db: Extension<Database>| async move {
            db.run(|conn: Connection| {
                let mut stmt = conn.prepare("SELECT id, path FROM data_files")?;
                let files = stmt
                    .query_map([], |row| {
                        let x: (u32, String) = (row.get(0)?, row.get(1)?);
                        Ok(x)
                    })?
                    .collect::<Result<Vec<_>, _>>()?;
                let mut html = String::new();
                for (id, path) in files {
                    html.push_str(&format!(
                        r#"<input type="button" hx-get="/video/{id}" hx-target=#content> {path} </div>"#,
                    ));
                }
                Ok(Html(format!("<div> {} </div>", html)))
            })
        }),
    )
    .route(
        "/video/:loc",
        get(|Path(loc): Path<u64>| async move {
            Html(format!(r#"<video src=/content/{loc} controls autoplay width="100%" height=auto> </video>"#))
        }),
    )
    .route(
        "/content/:loc",
        get(|Path(loc): Path<u64>, db: Extension<Database>, request: Request<Body>| async move {
            let path: String = db
                .run(move |conn| {
                    conn.query_row("SELECT path FROM data_files WHERE id=?1", [loc], |row| {
                        row.get(0)
                    })
                })
                .unwrap();
            let serve_file = ServeFile::new(path);
            serve_file.oneshot(request).await
        }),
    )
}
