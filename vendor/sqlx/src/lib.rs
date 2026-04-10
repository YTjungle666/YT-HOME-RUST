pub use sqlx_core::Either;
pub use sqlx_core::acquire::Acquire;
pub use sqlx_core::arguments::{Arguments, IntoArguments};
pub use sqlx_core::column::{Column, ColumnIndex};
pub use sqlx_core::connection::{ConnectOptions, Connection};
pub use sqlx_core::database::{self, Database};
pub use sqlx_core::describe::Describe;
#[doc(inline)]
pub use sqlx_core::error::{self, Error, Result};
pub use sqlx_core::executor::{Execute, Executor};
pub use sqlx_core::from_row::FromRow;
#[cfg(feature = "migrate")]
pub use sqlx_core::migrate;
pub use sqlx_core::pool::{self, Pool};
pub use sqlx_core::query::{query, query_with};
pub use sqlx_core::query_as::{query_as, query_as_with};
pub use sqlx_core::query_builder::{self, QueryBuilder};
pub use sqlx_core::query_scalar::{query_scalar, query_scalar_with};
pub use sqlx_core::raw_sql::{RawSql, raw_sql};
pub use sqlx_core::row::Row;
pub use sqlx_core::statement::Statement;
pub use sqlx_core::transaction::{Transaction, TransactionManager};
pub use sqlx_core::type_info::TypeInfo;
pub use sqlx_core::types::Type;
pub use sqlx_core::value::{Value, ValueRef};

#[cfg(feature = "_sqlite")]
pub use sqlx_sqlite::{
    self as sqlite, Sqlite, SqliteConnection, SqliteExecutor, SqlitePool, SqliteTransaction,
};

pub mod types {
    pub use sqlx_core::types::*;
}

pub mod encode {
    pub use sqlx_core::encode::{Encode, IsNull};
}

pub use self::encode::Encode;

pub mod decode {
    pub use sqlx_core::decode::Decode;
}

pub use self::decode::Decode;

pub mod query {
    pub use sqlx_core::query::Query;
    pub use sqlx_core::query_as::QueryAs;
    pub use sqlx_core::query_scalar::QueryScalar;
}

pub mod prelude {
    pub use super::Acquire;
    pub use super::ConnectOptions;
    pub use super::Connection;
    pub use super::Decode;
    pub use super::Encode;
    pub use super::Executor;
    pub use super::FromRow;
    pub use super::IntoArguments;
    pub use super::Row;
    pub use super::Statement;
    pub use super::Type;
}
