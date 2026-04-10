use serde::{Deserialize, Serialize};
use sqlx::{Error, FromRow, Row, sqlite::SqliteRow};

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SettingRow {
    pub id: i64,
    pub key: String,
    pub value: String,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct TlsRow {
    pub id: i64,
    pub name: String,
    pub server: String,
    pub client: String,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct UserRow {
    pub id: i64,
    pub username: String,
    pub password: String,
    pub last_logins: String,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct TokenRow {
    pub id: i64,
    pub desc: String,
    pub token: String,
    pub expiry: i64,
    pub user_id: i64,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SessionRow {
    pub id: i64,
    pub session_id: String,
    pub user_id: i64,
    pub expires_at: i64,
    pub created_at: i64,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ClientRow {
    pub id: i64,
    pub enable: bool,
    pub name: String,
    pub config: String,
    pub inbounds: String,
    pub links: String,
    pub volume: i64,
    pub expiry: i64,
    pub down: i64,
    pub up: i64,
    pub desc: String,
    pub group_name: String,
    pub delay_start: bool,
    pub auto_reset: bool,
    pub reset_days: i64,
    pub next_reset: i64,
    pub total_up: i64,
    pub total_down: i64,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct StatsRow {
    pub id: i64,
    pub date_time: i64,
    pub resource: String,
    pub tag: String,
    pub direction: bool,
    pub traffic: i64,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ChangeRow {
    pub id: i64,
    pub date_time: i64,
    pub actor: String,
    pub key: String,
    pub action: String,
    pub obj: String,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct InboundRow {
    pub id: i64,
    pub kind: String,
    pub tag: String,
    pub allow_lan_access: bool,
    pub tls_id: i64,
    pub addrs: String,
    pub out_json: String,
    pub options: String,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct OutboundRow {
    pub id: i64,
    pub kind: String,
    pub tag: String,
    pub options: String,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ServiceRow {
    pub id: i64,
    pub kind: String,
    pub tag: String,
    pub tls_id: i64,
    pub options: String,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct EndpointRow {
    pub id: i64,
    pub kind: String,
    pub tag: String,
    pub options: String,
    pub ext: String,
}

macro_rules! impl_sqlite_from_row {
    ($ty:ty { $($field:ident),+ $(,)? }) => {
        impl<'r> FromRow<'r, SqliteRow> for $ty {
            fn from_row(row: &'r SqliteRow) -> Result<Self, Error> {
                Ok(Self {
                    $(
                        $field: row.try_get(stringify!($field))?,
                    )+
                })
            }
        }
    };
}

impl_sqlite_from_row!(SettingRow { id, key, value });
impl_sqlite_from_row!(TlsRow { id, name, server, client });
impl_sqlite_from_row!(UserRow { id, username, password, last_logins });
impl_sqlite_from_row!(TokenRow { id, desc, token, expiry, user_id });
impl_sqlite_from_row!(SessionRow { id, session_id, user_id, expires_at, created_at });
impl_sqlite_from_row!(ClientRow {
    id,
    enable,
    name,
    config,
    inbounds,
    links,
    volume,
    expiry,
    down,
    up,
    desc,
    group_name,
    delay_start,
    auto_reset,
    reset_days,
    next_reset,
    total_up,
    total_down
});
impl_sqlite_from_row!(StatsRow { id, date_time, resource, tag, direction, traffic });
impl_sqlite_from_row!(ChangeRow { id, date_time, actor, key, action, obj });
impl_sqlite_from_row!(InboundRow {
    id,
    kind,
    tag,
    allow_lan_access,
    tls_id,
    addrs,
    out_json,
    options
});
impl_sqlite_from_row!(OutboundRow { id, kind, tag, options });
impl_sqlite_from_row!(ServiceRow { id, kind, tag, tls_id, options });
impl_sqlite_from_row!(EndpointRow { id, kind, tag, options, ext });
