use axum::Extension;
use r2d2::{ManageConnection, Pool, PooledConnection};
pub struct ConnectionManager;

impl ManageConnection for ConnectionManager {
    type Connection = rusqlite::Connection;
    type Error = rusqlite::Error;

    fn connect(&self) -> Result<Self::Connection, Self::Error> {
        rusqlite::Connection::open("database/database.sqlite")
    }

    fn is_valid(&self, conn: &mut Self::Connection) -> Result<(), Self::Error> {
        conn.execute("SELECT 1", ())?;
        Ok(())
    }

    fn has_broken(&self, _conn: &mut Self::Connection) -> bool {
        false
    }
}

#[derive(Clone)]
pub struct Database(Pool<ConnectionManager>);

impl Database {
    pub fn new() -> Result<Extension<Database>, r2d2::Error> {
        // Note: Use Pool::builder() for more configuration options.
        Ok(Extension(Self(Pool::new(ConnectionManager)?)))
    }

    pub fn connection(&self) -> Result<PooledConnection<ConnectionManager>, r2d2::Error> {
        self.0.get()
    }
}
