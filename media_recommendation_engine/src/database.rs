use std::{
    fmt::{self, Formatter},
    ops::Deref,
};

use r2d2::{ManageConnection, Pool};
use tracing::info;

use crate::{
    state::{AppError, AppResult},
    utils::ConvertErr,
};

pub struct ConnectionManager;

impl ManageConnection for ConnectionManager {
    type Connection = rusqlite::Connection;
    type Error = AppError;

    fn connect(&self) -> Result<Self::Connection, Self::Error> {
        let conn = rusqlite::Connection::open("database/database.sqlite")?;

        // NOTE: Read the Docs before changing something about these pragmas
        conn.pragma_update(None, "journal_mode", "WAL")?;
        conn.pragma_update(None, "synchronous", "NORMAL")?;
        conn.pragma_update(None, "foreign_keys", "ON")?;

        Ok(conn)
    }

    fn is_valid(&self, conn: &mut Self::Connection) -> Result<(), Self::Error> {
        conn.query_row("SELECT 1", [], |_r| Ok(())).convert_err()
    }

    fn has_broken(&self, _conn: &mut Self::Connection) -> bool {
        false
    }
}

#[derive(Clone)]
pub struct Database(Pool<ConnectionManager>);

impl Database {
    pub fn new() -> AppResult<Self> {
        // Note: Use Pool::builder() for more configuration options.
        let pool = Pool::new(ConnectionManager)?;
        let connection = pool.get()?;
        db_init(&connection).expect(
            "Database initialization failed, when this happens something has gone horribly wrong",
        );
        Ok(Self(pool))
    }
}

impl Deref for Database {
    type Target = Pool<ConnectionManager>;

    fn deref(&self) -> &Self::Target {
        &self.0
    }
}

impl fmt::Debug for Database {
    fn fmt(&self, f: &mut Formatter<'_>) -> fmt::Result {
        f.debug_struct("Database").finish()
    }
}

fn db_init(conn: &rusqlite::Connection) -> AppResult<()> {
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
    - Consider adding a hash and last_modified column to data_files for tracking what needs to be recomputed/reevaluated
        -> Same hash somewhere new -> reassign playback thumbnails for example
        -> Different hash but location the same -> just recompute stuff related to the file without changing/removing references to it
        -> last modified changed -> could be the trigger for recomputing the hash depending on how expensive that is, does this have any other meaning?
    */

    const INIT_REQUEST: &str = include_str!("../../database/sql/init.sql");
    conn.execute_batch(INIT_REQUEST)?;

    Ok(())
}

type Mapfn<T> = for<'a, 'b> fn(&'a rusqlite::Row<'b>) -> Result<T, rusqlite::Error>;

pub trait QueryRowIntoStmtExt<P>
where
    P: rusqlite::Params,
{
    fn query_map_into<T: for<'a> TryFrom<&'a rusqlite::Row<'a>, Error = rusqlite::Error>>(
        &mut self,
        params: P,
    ) -> Result<rusqlite::MappedRows<'_, Mapfn<T>>, rusqlite::Error>;
}

impl<P> QueryRowIntoStmtExt<P> for rusqlite::Statement<'_>
where
    P: rusqlite::Params,
{
    // Executes the prepared statement and tries to convert each row into the provided type
    fn query_map_into<T: for<'a> TryFrom<&'a rusqlite::Row<'a>, Error = rusqlite::Error>>(
        &mut self,
        params: P,
    ) -> Result<rusqlite::MappedRows<'_, Mapfn<T>>, rusqlite::Error> {
        fn map_row<T>(row: &rusqlite::Row<'_>) -> Result<T, rusqlite::Error>
        where
            T: for<'a> TryFrom<&'a rusqlite::Row<'a>, Error = rusqlite::Error>,
        {
            row.try_into()
        }

        self.query_map(params, map_row)
    }
}

pub trait QueryRowIntoConnExt<P>
where
    P: rusqlite::Params,
{
    fn query_row_into<T: for<'a> TryFrom<&'a rusqlite::Row<'a>, Error = rusqlite::Error>>(
        &self,
        sql: &str,
        params: P,
    ) -> Result<T, rusqlite::Error>;
}

impl<P> QueryRowIntoConnExt<P> for rusqlite::Connection
where
    P: rusqlite::Params,
{
    /// Executes the provided sql and tries to convert the first row into the provided type
    fn query_row_into<T: for<'a> TryFrom<&'a rusqlite::Row<'a>, Error = rusqlite::Error>>(
        &self,
        sql: &str,
        params: P,
    ) -> Result<T, rusqlite::Error> {
        self.query_row(sql, params, |row| row.try_into())
    }
}

pub trait QueryRowGetStmtExt<P>
where
    P: rusqlite::Params,
{
    fn query_row_get<T: rusqlite::types::FromSql>(
        &mut self,
        params: P,
    ) -> Result<T, rusqlite::Error>;
    fn query_map_get<T: rusqlite::types::FromSql>(
        &mut self,
        params: P,
    ) -> Result<rusqlite::MappedRows<'_, Mapfn<T>>, rusqlite::Error>;
}

impl<P> QueryRowGetStmtExt<P> for rusqlite::Statement<'_>
where
    P: rusqlite::Params,
{
    /// Executes the prepared statement and gets the first column of the first row
    fn query_row_get<T: rusqlite::types::FromSql>(
        &mut self,
        params: P,
    ) -> Result<T, rusqlite::Error> {
        self.query_row(params, |row| row.get(0))
    }

    /// Executes the prepared statement and gets the first column of each row
    fn query_map_get<T: rusqlite::types::FromSql>(
        &mut self,
        params: P,
    ) -> Result<rusqlite::MappedRows<'_, Mapfn<T>>, rusqlite::Error> {
        fn map_row<T>(row: &rusqlite::Row<'_>) -> Result<T, rusqlite::Error>
        where
            T: rusqlite::types::FromSql,
        {
            row.get(0)
        }

        self.query_map(params, map_row)
    }
}

pub trait QueryRowGetConnExt<P>
where
    P: rusqlite::Params,
{
    fn query_row_get<T: rusqlite::types::FromSql>(
        &self,
        sql: &str,
        params: P,
    ) -> Result<T, rusqlite::Error>;
}

impl<P> QueryRowGetConnExt<P> for rusqlite::Connection
where
    P: rusqlite::Params,
{
    /// Executes the provided sql and gets the first column of the first row
    fn query_row_get<T: rusqlite::types::FromSql>(
        &self,
        sql: &str,
        params: P,
    ) -> Result<T, rusqlite::Error> {
        self.query_row(sql, params, |row| row.get(0))
    }
}
