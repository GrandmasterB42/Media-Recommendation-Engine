use std::ops::Deref;

use r2d2::{ManageConnection, Pool};
use tokio::runtime::{Handle, Runtime};
use tokio_rusqlite::Connection;
use tracing::info;

use crate::{
    state::{AppError, AppResult},
    utils::HandleErr,
};

pub struct ConnectionManager;

impl ManageConnection for ConnectionManager {
    type Connection = tokio_rusqlite::Connection;
    type Error = AppError;

    fn connect(&self) -> Result<Self::Connection, Self::Error> {
        async fn get_connection() -> Result<tokio_rusqlite::Connection, tokio_rusqlite::Error> {
            let conn = tokio_rusqlite::Connection::open("database/database.sqlite").await?;
            conn.call(|conn| {
                conn.pragma_update(None, "journal_mode", "WAL")?;
                conn.pragma_update(None, "synchronous", "NORMAL")?;
                conn.pragma_update(None, "foreign_keys", "ON")?;
                Ok(())
            })
            .await?;
            Ok(conn)
        }

        let Ok(conn) = Runtime::new()
            .expect("failed to create tokio runtime")
            .block_on(get_connection())
        else {
            return Err(AppError::Custom(
                "failed to connect to database".to_string(),
            ));
        };

        // NOTE: Read the Docs before changing something about these pragmas
        Ok(conn)
    }

    fn is_valid(&self, conn: &mut Self::Connection) -> Result<(), Self::Error> {
        Ok(tokio::task::block_in_place(|| {
            Handle::current().block_on(async {
                conn.call(|conn| Ok(conn.query_row("SELECT 1", [], |_r| Ok(()))))
                    .await
            })
        })??)
    }

    fn has_broken(&self, _conn: &mut Self::Connection) -> bool {
        false
    }
}

#[derive(Clone)]
pub struct Database(Pool<ConnectionManager>);

impl Database {
    pub async fn new() -> AppResult<Self> {
        // Note: Use Pool::builder() for more configuration options.
        let pool = Pool::new(ConnectionManager)?;
        let connection = pool.get()?;
        // TODO: db_init failing is bad, something should probably happen here
        db_init(&connection)
            .await
            .log_err_with_msg("Failed to initialize database");
        Ok(Self(pool))
    }
}

impl Deref for Database {
    type Target = Pool<ConnectionManager>;

    fn deref(&self) -> &Self::Target {
        &self.0
    }
}

async fn db_init(conn: &Connection) -> AppResult<()> {
    conn.call(|conn| {
        {
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

            const INIT_REQUEST: &str = include_str!("../../database/sql/init.sql");
            conn.execute_batch(INIT_REQUEST)?;
        }
        Ok(())
    })
    .await
    .map_err(Into::into)
}

type Mapfn<T> = for<'a, 'b> fn(&'a rusqlite::Row<'b>) -> Result<T, rusqlite::Error>;

pub trait QueryRowIntoStmtExt<T> {
    fn query_row_into<P: rusqlite::Params>(&mut self, params: P) -> Result<T, rusqlite::Error>;
    fn query_map_into<P: rusqlite::Params>(
        &mut self,
        params: P,
    ) -> Result<rusqlite::MappedRows<'_, Mapfn<T>>, rusqlite::Error>;
}

impl<T> QueryRowIntoStmtExt<T> for rusqlite::Statement<'_>
where
    T: for<'a> TryFrom<&'a rusqlite::Row<'a>, Error = rusqlite::Error>,
{
    /// Executes the prepared statement and tries to convert the first row into the provided type
    fn query_row_into<P: rusqlite::Params>(&mut self, params: P) -> Result<T, rusqlite::Error> {
        self.query_row(params, |row| row.try_into())
    }

    // Executes the prepared statement and tries to convert each row into the provided type
    fn query_map_into<P: rusqlite::Params>(
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

pub trait QueryRowIntoConnExt<T> {
    fn query_row_into<P: rusqlite::Params>(
        &self,
        sql: &str,
        params: P,
    ) -> Result<T, rusqlite::Error>;
}

impl<T> QueryRowIntoConnExt<T> for rusqlite::Connection
where
    T: for<'a> TryFrom<&'a rusqlite::Row<'a>, Error = rusqlite::Error>,
{
    /// Executes the provided sql and tries to convert the first row into the provided type
    fn query_row_into<P: rusqlite::Params>(
        &self,
        sql: &str,
        params: P,
    ) -> Result<T, rusqlite::Error> {
        self.query_row(sql, params, |row| row.try_into())
    }
}

pub trait QueryRowGetStmtExt<T> {
    fn query_row_get<P: rusqlite::Params>(&mut self, params: P) -> Result<T, rusqlite::Error>;
    fn query_map_get<P: rusqlite::Params>(
        &mut self,
        params: P,
    ) -> Result<rusqlite::MappedRows<'_, Mapfn<T>>, rusqlite::Error>;
}

impl<T> QueryRowGetStmtExt<T> for rusqlite::Statement<'_>
where
    T: rusqlite::types::FromSql,
{
    /// Executes the prepared statement and gets the first column of the first row
    fn query_row_get<P: rusqlite::Params>(&mut self, params: P) -> Result<T, rusqlite::Error> {
        self.query_row(params, |row| row.get(0))
    }

    /// Executes the prepared statement and gets the first column of each row
    fn query_map_get<P: rusqlite::Params>(
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

pub trait QueryRowGetConnExt<T> {
    fn query_row_get<P: rusqlite::Params>(
        &self,
        sql: &str,
        params: P,
    ) -> Result<T, rusqlite::Error>;
}

impl<T> QueryRowGetConnExt<T> for rusqlite::Connection
where
    T: rusqlite::types::FromSql,
{
    /// Executes the provided sql and gets the first column of the first row
    fn query_row_get<P: rusqlite::Params>(
        &self,
        sql: &str,
        params: P,
    ) -> Result<T, rusqlite::Error> {
        self.query_row(sql, params, |row| row.get(0))
    }
}
