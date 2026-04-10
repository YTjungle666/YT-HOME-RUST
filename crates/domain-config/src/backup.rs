use std::{
    env, fs, process,
    time::{SystemTime, UNIX_EPOCH},
};

use infra_db::{connect_sqlite, run_migrations};
use shared::{AppError, AppResult};

use super::SettingsService;

const IMPORT_CLEAR_ORDER: &[&str] = &[
    "changes",
    "stats",
    "clients",
    "endpoints",
    "services",
    "inbounds",
    "outbounds",
    "tokens",
    "user_sessions",
    "tls",
    "users",
    "settings",
];

const IMPORT_COPY_ORDER: &[&str] = &[
    "settings",
    "users",
    "tls",
    "user_sessions",
    "tokens",
    "outbounds",
    "inbounds",
    "services",
    "endpoints",
    "clients",
    "stats",
    "changes",
];

impl SettingsService {
    pub async fn export_database(&self, exclude: &[&str]) -> AppResult<Vec<u8>> {
        for table in exclude {
            if !matches!(*table, "stats" | "changes") {
                return Err(AppError::Validation(format!(
                    "unsupported backup exclude target {table}"
                )));
            }
        }

        let temp_path = unique_temp_path("backup", "db");
        let vacuum_sql = format!("VACUUM INTO '{}'", sqlite_string_literal(&temp_path));
        sqlx::query(&vacuum_sql).execute(&self.pool).await?;

        let temp_pool = connect_sqlite(&temp_path).await?;
        for table in exclude {
            let delete_sql = format!("DELETE FROM {table}");
            sqlx::query(&delete_sql).execute(&temp_pool).await?;
        }
        sqlx::query("VACUUM").execute(&temp_pool).await?;
        temp_pool.close().await;

        let bytes = fs::read(&temp_path)?;
        let _ = fs::remove_file(&temp_path);
        Ok(bytes)
    }

    pub async fn import_database(&self, bytes: &[u8]) -> AppResult<()> {
        if !is_sqlite_database(bytes) {
            return Err(AppError::Validation("invalid SQLite database".to_string()));
        }

        let temp_path = unique_temp_path("import", "db");
        fs::write(&temp_path, bytes)?;

        let import_pool = connect_sqlite(&temp_path).await?;
        run_migrations(&import_pool).await?;
        import_pool.close().await;

        let attach_sql =
            format!("ATTACH DATABASE '{}' AS imported", sqlite_string_literal(&temp_path));
        let mut connection = self.pool.acquire().await?;

        sqlx::query("PRAGMA foreign_keys = OFF").execute(&mut *connection).await?;
        sqlx::query(&attach_sql).execute(&mut *connection).await?;
        sqlx::query("BEGIN IMMEDIATE").execute(&mut *connection).await?;

        let import_result = async {
            for table in IMPORT_CLEAR_ORDER {
                let delete_sql = format!("DELETE FROM {table}");
                sqlx::query(&delete_sql).execute(&mut *connection).await?;
            }

            for table in IMPORT_COPY_ORDER {
                let copy_sql = format!("INSERT INTO main.{table} SELECT * FROM imported.{table}");
                sqlx::query(&copy_sql).execute(&mut *connection).await?;
            }

            sqlx::query("COMMIT").execute(&mut *connection).await?;
            AppResult::Ok(())
        }
        .await;

        if import_result.is_err() {
            let _ = sqlx::query("ROLLBACK").execute(&mut *connection).await;
        }

        let _ = sqlx::query("DETACH DATABASE imported").execute(&mut *connection).await;
        let _ = sqlx::query("PRAGMA foreign_keys = ON").execute(&mut *connection).await;
        let _ = fs::remove_file(&temp_path);

        import_result?;
        self.ensure_defaults().await?;
        Ok(())
    }
}

fn unique_temp_path(prefix: &str, extension: &str) -> String {
    let nanos = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|duration| duration.as_nanos())
        .unwrap_or_default();
    let mut path = env::temp_dir();
    path.push(format!("s-ui-{prefix}-{}-{nanos}.{extension}", process::id()));
    path.display().to_string()
}

fn sqlite_string_literal(value: &str) -> String {
    value.replace('\'', "''")
}

fn is_sqlite_database(bytes: &[u8]) -> bool {
    const SQLITE_MAGIC: &[u8] = b"SQLite format 3\0";
    bytes.starts_with(SQLITE_MAGIC)
}
