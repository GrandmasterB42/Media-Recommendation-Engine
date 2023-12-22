use std::ops::Deref;

use axum::{http, response::IntoResponse, Extension};
use r2d2::{ManageConnection, Pool, PooledConnection};
use tracing::info;

use crate::utils::HandleErr;

pub struct ConnectionManager;

impl ManageConnection for ConnectionManager {
    type Connection = rusqlite::Connection;
    type Error = rusqlite::Error;

    fn connect(&self) -> Result<Self::Connection, Self::Error> {
        let conn = rusqlite::Connection::open("database/database.sqlite")?;
        // NOTE: Read the Docs before changing something about these pragmas
        conn.pragma_update(None, "journal_mode", "WAL")?;
        conn.pragma_update(None, "synchronous", "NORMAL")?;
        conn.pragma_update(None, "foreign_keys", "ON")?;
        Ok(conn)
    }

    fn is_valid(&self, conn: &mut Self::Connection) -> Result<(), Self::Error> {
        conn.query_row("SELECT 1", [], |_r| Ok(()))
    }

    fn has_broken(&self, _conn: &mut Self::Connection) -> bool {
        false
    }
}

#[derive(Clone)]
pub struct Database(Pool<ConnectionManager>);

impl Database {
    pub fn new() -> DatabaseResult<Extension<Self>> {
        // Note: Use Pool::builder() for more configuration options.
        let pool = Pool::new(ConnectionManager)?;
        let mut connection = pool.get()?;
        // TODO: db_init failing is bad, something should probably happen here
        db_init(&mut connection).log_err_with_msg("Failed to initialize database");
        Ok(Extension(Self(pool)))
    }
}

impl Deref for Database {
    type Target = Pool<ConnectionManager>;

    fn deref(&self) -> &Self::Target {
        &self.0
    }
}

pub type Connection<'a> = &'a mut PooledConnection<ConnectionManager>;

fn db_init(conn: Connection) -> rusqlite::Result<()> {
    {
        let mut stmt = conn.prepare("SELECT name FROM sqlite_master")?;
        let mut rows = stmt.query([])?;
        let initialized = rows.next()?.is_some();
        if initialized {
            return Ok(());
        }
    };
    info!("Setting up database for the first time");

    /*
    TODO
    - Make the storage_locations entry not hardcoded
    - Consider adding a hash and last_modified column to data_files for tracking what needs to be recomputed/reevaluated
        -> Same hash somewhere new -> reassign playback thumbnails for example
        -> Different hash but location the same -> just recompute stuff related to the file without changing/removing references to it
        -> last modified changed -> could be the trigger for recomputing the hash depending on how expensive that is, does this have any other meaning?
    */
    // NOTE: I know this isn't the best way to do this, but I'm lazy and it's easy to extend right now
    const INIT_REQUESTS: &[&str] = &[
        "CREATE TABLE storage_locations (path)",
        "INSERT INTO storage_locations VALUES ('Y:')",
        "CREATE TABLE data_files (
            id INTEGER PRIMARY KEY,
            path TEXT NOT NULL
        )",
        "CREATE TABLE multipart (
            id INTEGER NOT NULL,
            videoid INTEGER REFERENCES data_files (id),
            part INTEGER NOT NULL
        )",
        "CREATE TABLE franchise (
            id INTEGER PRIMARY KEY,
            title TEXT NOT NULL
        )",
        "CREATE TABLE movies (
            id INTEGER PRIMARY KEY,
            franchiseid INTEGER REFERENCES franchise (id),
            videoid INTEGER,
            referenceflag INTEGER NOT NULL,
            title TEXT NOT NULL
        )",
        "CREATE TABLE series (
            id INTEGER PRIMARY KEY,
            franchiseid INTEGER REFERENCES franchise (id),
            title TEXT NULL
        )",
        "CREATE TABLE seasons (
            id INTEGER PRIMARY KEY,
            seriesid INTEGER REFERENCES series (id),
            season INTEGER NULL,
            title TEXT NULL
        )",
        "CREATE TABLE episodes (
            id INTEGER PRIMARY KEY,
            seasonid INTEGER REFERENCES seasons (id),
            videoid INTEGER,
            referenceflag INTEGER NOT NULL,
            episode INTEGER NOT NULL,
            title TEXT NULL
        )",
    ];

    let tx = conn.transaction()?;
    for request in INIT_REQUESTS {
        tx.execute(request, [])?;
    }
    tx.commit()?;

    Ok(())
}

pub type DatabaseResult<T> = Result<T, DatabaseError>;

#[derive(Debug)]
pub enum DatabaseError {
    Database(rusqlite::Error),
    Pool(r2d2::Error),
}

impl From<r2d2::Error> for DatabaseError {
    fn from(e: r2d2::Error) -> Self {
        DatabaseError::Pool(e)
    }
}

impl From<rusqlite::Error> for DatabaseError {
    fn from(e: rusqlite::Error) -> Self {
        DatabaseError::Database(e)
    }
}

impl IntoResponse for DatabaseError {
    fn into_response(self) -> axum::response::Response {
        #[cfg(not(debug_assertions))]
        return (http::StatusCode::INTERNAL_SERVER_ERROR).into_response();
        #[cfg(debug_assertions)]
        return (
            http::StatusCode::INTERNAL_SERVER_ERROR,
            format!("Error: {self:?}"),
        )
            .into_response();
    }
}
