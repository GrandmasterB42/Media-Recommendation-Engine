use std::ops::Deref;

use axum::{http, response::IntoResponse, Extension};
use r2d2::{ManageConnection, Pool, PooledConnection};
use tracing::info;

use crate::utils::HandleErr;

// TODO: Consider moving this into a submodule

pub struct ConnectionManager;

impl ManageConnection for ConnectionManager {
    type Connection = rusqlite::Connection;
    type Error = rusqlite::Error;

    fn connect(&self) -> Result<Self::Connection, Self::Error> {
        rusqlite::Connection::open("database/database.sqlite")
    }

    fn is_valid(&self, _conn: &mut Self::Connection) -> Result<(), Self::Error> {
        //conn.execute("SELECT 1", ())?; TODO: Make this do something
        Ok(())
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

    // TODO: consider making this async because reasons, maybe not?
    // -> Rethink the interface
    pub fn run<F, T>(&self, f: F) -> DatabaseResult<T>
    where
        F: FnOnce(Connection) -> rusqlite::Result<T> + Send + 'static,
    {
        let mut conn = self.0.get()?;
        Ok(f(&mut conn)?)
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
    // TODO: Make name/title consistent
    const INIT_REQUESTS: &[&str] = &[
        "CREATE TABLE storage_locations (path)",
        "INSERT INTO storage_locations VALUES ('Y:')",
        "CREATE TABLE data_files (
            id INTEGER PRIMARY KEY AUTOINCREMENT,
            path TEXT NOT NULL
        )",
        "CREATE TABLE movies (
            id INTEGER PRIMARY KEY AUTOINCREMENT,
            videoid INTEGER REFERENCES data_files (id),
            title TEXT NOT NULL
        )",
        "CREATE TABLE series (
            id INTEGER PRIMARY KEY AUTOINCREMENT,
            title TEXT NOT NULL
        )",
        "CREATE TABLE seasons (
            id INTEGER PRIMARY KEY AUTOINCREMENT,
            seriesid INTEGER REFERENCES series (id),
            season INTEGER NULL,
            name TEXT NULL
        )",
        "CREATE TABLE episodes (
            id INTEGER PRIMARY KEY AUTOINCREMENT,
            seasonid INTEGER REFERENCES seasons (id),
            videoid INTEGER REFERENCES data_files (id),
            episode INTEGER NOT NULL,
            name TEXT NULL
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
        (http::StatusCode::INTERNAL_SERVER_ERROR).into_response()
    }
}
