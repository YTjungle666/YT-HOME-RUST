use std::{
    env, fs,
    path::{Path, PathBuf},
    str::FromStr,
    time::Duration,
};

use shared::{AppResult, settings::APP_NAME};
use sqlx::{
    SqlitePool,
    sqlite::{SqliteConnectOptions, SqliteJournalMode, SqlitePoolOptions, SqliteSynchronous},
};

pub type Db = SqlitePool;

pub async fn connect_sqlite<P>(db_path: P) -> AppResult<Db>
where
    P: AsRef<Path>,
{
    let path = db_path.as_ref();
    if let Some(parent) = path.parent() {
        fs::create_dir_all(parent)?;
    }

    let url = format!("sqlite://{}", path.display());
    let options = SqliteConnectOptions::from_str(&url)?
        .create_if_missing(true)
        .busy_timeout(Duration::from_secs(10))
        .journal_mode(SqliteJournalMode::Wal)
        .foreign_keys(true)
        .synchronous(SqliteSynchronous::Normal);

    let pool = SqlitePoolOptions::new()
        .max_connections(25)
        .min_connections(1)
        .acquire_timeout(Duration::from_secs(10))
        .connect_with(options)
        .await?;

    Ok(pool)
}

pub async fn run_migrations(pool: &Db) -> AppResult<()> {
    let migrator = sqlx::migrate::Migrator::new(resolve_migrations_dir()?).await?;
    migrator.run(pool).await?;
    Ok(())
}

pub fn default_db_path(executable_dir: &Path) -> String {
    executable_dir.join("db").join(format!("{APP_NAME}.db")).display().to_string()
}

fn resolve_migrations_dir() -> AppResult<PathBuf> {
    if let Ok(value) = env::var("SUI_MIGRATIONS_DIR") {
        let path = PathBuf::from(value);
        if path.is_dir() {
            return Ok(path);
        }
    }

    let executable_dir = env::current_exe().ok().and_then(|path| path.parent().map(PathBuf::from));
    let current_dir = env::current_dir().ok();
    let candidates = [
        executable_dir.as_ref().map(|dir| dir.join("migrations")),
        current_dir.as_ref().map(|dir| dir.join("migrations")),
        current_dir.as_ref().map(|dir| dir.join("crates/infra-db/migrations")),
    ];

    candidates.into_iter().flatten().find(|path| path.is_dir()).ok_or_else(|| {
        std::io::Error::new(std::io::ErrorKind::NotFound, "unable to resolve migrations dir").into()
    })
}
