use rusqlite::{types::FromSql, ToSql};

/// The things content can be, other means it has to be manually assigned or doesn't exist yet
#[derive(Clone, Copy)]
pub enum ContentType {
    Other,
    Movie,
    Episode,
    Song,
}

impl FromSql for ContentType {
    fn column_result(value: rusqlite::types::ValueRef) -> rusqlite::types::FromSqlResult<Self> {
        match value {
            rusqlite::types::ValueRef::Integer(i) => match i {
                0 => Ok(ContentType::Other),
                1 => Ok(ContentType::Movie),
                2 => Ok(ContentType::Episode),
                3 => Ok(ContentType::Song),
                _ => Err(rusqlite::types::FromSqlError::InvalidType),
            },
            _ => Err(rusqlite::types::FromSqlError::InvalidType),
        }
    }
}

impl ToSql for ContentType {
    fn to_sql(&self) -> rusqlite::Result<rusqlite::types::ToSqlOutput<'_>> {
        Ok(rusqlite::types::ToSqlOutput::Owned(
            rusqlite::types::Value::Integer(*self as i64),
        ))
    }
}

/// All things that can be inside collections
#[derive(Clone, Copy)]
pub enum TableId {
    Collection,
    Content,
}

impl FromSql for TableId {
    fn column_result(value: rusqlite::types::ValueRef) -> rusqlite::types::FromSqlResult<Self> {
        match value {
            rusqlite::types::ValueRef::Integer(i) => match i {
                0 => Ok(TableId::Collection),
                1 => Ok(TableId::Content),
                _ => Err(rusqlite::types::FromSqlError::InvalidType),
            },
            _ => Err(rusqlite::types::FromSqlError::InvalidType),
        }
    }
}

impl ToSql for TableId {
    fn to_sql(&self) -> rusqlite::Result<rusqlite::types::ToSqlOutput<'_>> {
        Ok(rusqlite::types::ToSqlOutput::Owned(
            rusqlite::types::Value::Integer(*self as i64),
        ))
    }
}

/// All different types of collections
#[derive(Clone, Copy)]
pub enum CollectionType {
    UserCollection,
    Franchise,
    Season,
    Series,
    Theme,
}

impl FromSql for CollectionType {
    fn column_result(value: rusqlite::types::ValueRef) -> rusqlite::types::FromSqlResult<Self> {
        match value {
            rusqlite::types::ValueRef::Integer(i) => match i {
                0 => Ok(CollectionType::UserCollection),
                1 => Ok(CollectionType::Franchise),
                2 => Ok(CollectionType::Season),
                3 => Ok(CollectionType::Series),
                4 => Ok(CollectionType::Theme),
                _ => Err(rusqlite::types::FromSqlError::InvalidType),
            },
            _ => Err(rusqlite::types::FromSqlError::InvalidType),
        }
    }
}

impl ToSql for CollectionType {
    fn to_sql(&self) -> rusqlite::Result<rusqlite::types::ToSqlOutput<'_>> {
        Ok(rusqlite::types::ToSqlOutput::Owned(
            rusqlite::types::Value::Integer(*self as i64),
        ))
    }
}
