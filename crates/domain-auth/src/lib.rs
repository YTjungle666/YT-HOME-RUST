use argon2::{
    Argon2, PasswordHash, PasswordHasher, PasswordVerifier,
    password_hash::{SaltString, rand_core::OsRng},
};
use axum_extra::extract::cookie::{Cookie, SameSite};
use base64::{Engine, engine::general_purpose::URL_SAFE_NO_PAD};
use infra_db::Db;
use rand::Rng;
use serde::Serialize;
use shared::{
    AppError, AppResult,
    model::{TokenRow, UserRow},
    settings::SESSION_COOKIE,
};
use sqlx::Row;
use time::{Duration, OffsetDateTime, macros::format_description};
use tracing::warn;

pub const DEFAULT_ADMIN_USERNAME: &str = "admin";
pub const DEFAULT_ADMIN_PASSWORD: &str = "admin";

#[derive(Debug, Clone, Serialize)]
pub struct AuthenticatedUser {
    pub id: i64,
    pub username: String,
}

#[derive(Debug)]
pub struct SessionState {
    pub cookie: Cookie<'static>,
    pub user: AuthenticatedUser,
}

#[derive(Clone)]
pub struct AuthService {
    pool: Db,
}

impl AuthService {
    pub fn new(pool: Db) -> Self {
        Self { pool }
    }

    pub async fn ensure_default_admin(&self) -> AppResult<()> {
        let count: i64 =
            sqlx::query_scalar("SELECT COUNT(*) FROM users").fetch_one(&self.pool).await?;
        if count > 0 {
            return Ok(());
        }

        let hashed_password = hash_password(DEFAULT_ADMIN_PASSWORD)?;
        sqlx::query("INSERT INTO users (username, password, last_logins) VALUES (?, ?, '')")
            .bind(DEFAULT_ADMIN_USERNAME)
            .bind(hashed_password)
            .execute(&self.pool)
            .await?;

        Ok(())
    }

    pub async fn get_first_user(&self) -> AppResult<Option<UserRow>> {
        let user = sqlx::query_as::<_, UserRow>(
            "SELECT id, username, password, last_logins FROM users ORDER BY id ASC LIMIT 1",
        )
        .fetch_optional(&self.pool)
        .await?;
        Ok(user)
    }

    pub async fn set_first_user_credentials(
        &self,
        username: Option<&str>,
        password: Option<&str>,
    ) -> AppResult<()> {
        self.ensure_default_admin().await?;
        let Some(user) = self.get_first_user().await? else {
            return Err(AppError::NotFound("user not found".to_string()));
        };

        let next_username = username
            .map(str::trim)
            .filter(|value| !value.is_empty())
            .unwrap_or(user.username.as_str());
        let next_password = match password.map(str::trim).filter(|value| !value.is_empty()) {
            Some(value) => hash_password(value)?,
            None => user.password,
        };

        sqlx::query("UPDATE users SET username = ?, password = ? WHERE id = ?")
            .bind(next_username)
            .bind(next_password)
            .bind(user.id)
            .execute(&self.pool)
            .await?;
        sqlx::query("DELETE FROM user_sessions WHERE user_id = ?")
            .bind(user.id)
            .execute(&self.pool)
            .await?;
        Ok(())
    }

    pub async fn reset_first_user_to_default(&self) -> AppResult<()> {
        self.set_first_user_credentials(Some(DEFAULT_ADMIN_USERNAME), Some(DEFAULT_ADMIN_PASSWORD))
            .await
    }

    pub async fn get_public_users(&self) -> AppResult<Vec<AuthenticatedUserView>> {
        let users = sqlx::query_as::<_, UserRow>(
            "SELECT id, username, password, last_logins FROM users ORDER BY id ASC",
        )
        .fetch_all(&self.pool)
        .await?;
        Ok(users
            .into_iter()
            .map(|user| AuthenticatedUserView {
                id: user.id,
                username: user.username,
                last_login: user.last_logins,
            })
            .collect())
    }

    pub async fn login(
        &self,
        username: &str,
        password: &str,
        remote_ip: &str,
        session_max_age_minutes: i64,
        secure_cookie: bool,
    ) -> AppResult<SessionState> {
        let Some(mut user) = sqlx::query_as::<_, UserRow>(
            "SELECT id, username, password, last_logins FROM users WHERE username = ? LIMIT 1",
        )
        .bind(username)
        .fetch_optional(&self.pool)
        .await?
        else {
            return Err(AppError::Authentication(format!(
                "wrong user or password! IP:  {remote_ip}"
            )));
        };

        let (matches, needs_upgrade) = verify_password(&user.password, password)?;
        if !matches {
            return Err(AppError::Authentication(format!(
                "wrong user or password! IP:  {remote_ip}"
            )));
        }

        if needs_upgrade {
            let hashed = hash_password(password)?;
            sqlx::query("UPDATE users SET password = ? WHERE id = ?")
                .bind(&hashed)
                .bind(user.id)
                .execute(&self.pool)
                .await?;
            user.password = hashed;
        }

        self.cleanup_expired_sessions().await?;
        let session_id = random_token(32);
        let now = OffsetDateTime::now_utc().unix_timestamp();
        let expires_at = if session_max_age_minutes > 0 {
            now + session_max_age_minutes * 60
        } else {
            now + (30 * 24 * 60 * 60)
        };

        sqlx::query("DELETE FROM user_sessions WHERE user_id = ?")
            .bind(user.id)
            .execute(&self.pool)
            .await?;
        sqlx::query(
            "INSERT INTO user_sessions (session_id, user_id, expires_at, created_at) VALUES (?, ?, ?, ?)",
        )
        .bind(&session_id)
        .bind(user.id)
        .bind(expires_at)
        .bind(now)
        .execute(&self.pool)
        .await?;

        let last_login = format!(
            "{} {}",
            OffsetDateTime::now_utc()
                .format(&format_description!("[year]-[month]-[day] [hour]:[minute]:[second]"))
                .map_err(|error| AppError::Validation(error.to_string()))?,
            remote_ip
        );
        sqlx::query("UPDATE users SET last_logins = ? WHERE id = ?")
            .bind(last_login)
            .bind(user.id)
            .execute(&self.pool)
            .await?;

        Ok(SessionState {
            cookie: build_session_cookie(&session_id, session_max_age_minutes, secure_cookie),
            user: AuthenticatedUser { id: user.id, username: user.username },
        })
    }

    pub async fn authenticate_session(
        &self,
        session_id: &str,
    ) -> AppResult<Option<AuthenticatedUser>> {
        self.cleanup_expired_sessions().await?;

        let user = sqlx::query(
            r#"
            SELECT users.id, users.username
            FROM user_sessions
            INNER JOIN users ON users.id = user_sessions.user_id
            WHERE user_sessions.session_id = ? AND user_sessions.expires_at > ?
            LIMIT 1
            "#,
        )
        .bind(session_id)
        .bind(OffsetDateTime::now_utc().unix_timestamp())
        .fetch_optional(&self.pool)
        .await?;

        Ok(user.map(|row| AuthenticatedUser { id: row.get("id"), username: row.get("username") }))
    }

    pub async fn logout(&self, session_id: &str) -> AppResult<()> {
        sqlx::query("DELETE FROM user_sessions WHERE session_id = ?")
            .bind(session_id)
            .execute(&self.pool)
            .await?;
        Ok(())
    }

    pub async fn change_password(
        &self,
        id: i64,
        old_password: &str,
        new_username: &str,
        new_password: &str,
    ) -> AppResult<()> {
        if new_username.is_empty() {
            return Err(AppError::Validation("username can not be empty".to_string()));
        }
        if new_password.is_empty() {
            return Err(AppError::Validation("password can not be empty".to_string()));
        }

        let Some(user) = sqlx::query_as::<_, UserRow>(
            "SELECT id, username, password, last_logins FROM users WHERE id = ? LIMIT 1",
        )
        .bind(id)
        .fetch_optional(&self.pool)
        .await?
        else {
            return Err(AppError::NotFound("user not found".to_string()));
        };

        let (matches, _) = verify_password(&user.password, old_password)?;
        if !matches {
            return Err(AppError::Authentication("wrong user or password".to_string()));
        }

        let hashed = hash_password(new_password)?;
        sqlx::query("UPDATE users SET username = ?, password = ? WHERE id = ?")
            .bind(new_username)
            .bind(hashed)
            .bind(id)
            .execute(&self.pool)
            .await?;
        sqlx::query("DELETE FROM user_sessions WHERE user_id = ?")
            .bind(id)
            .execute(&self.pool)
            .await?;
        Ok(())
    }

    pub async fn load_tokens(&self) -> AppResult<Vec<LoadedToken>> {
        let rows = sqlx::query_as::<_, TokenRow>(
            "SELECT id, desc, token, expiry, user_id FROM tokens WHERE expiry = 0 OR expiry > ?",
        )
        .bind(OffsetDateTime::now_utc().unix_timestamp())
        .fetch_all(&self.pool)
        .await?;

        let mut result = Vec::with_capacity(rows.len());
        for row in rows {
            let username: String = sqlx::query_scalar("SELECT username FROM users WHERE id = ?")
                .bind(row.user_id)
                .fetch_one(&self.pool)
                .await?;
            result.push(LoadedToken { token: row.token, expiry: row.expiry, username });
        }
        Ok(result)
    }

    pub async fn get_user_tokens(&self, username: &str) -> AppResult<Vec<UserTokenView>> {
        let Some(user_id) =
            sqlx::query_scalar::<_, i64>("SELECT id FROM users WHERE username = ? LIMIT 1")
                .bind(username)
                .fetch_optional(&self.pool)
                .await?
        else {
            return Ok(Vec::new());
        };

        let rows = sqlx::query_as::<_, TokenRow>(
            "SELECT id, desc, token, expiry, user_id FROM tokens WHERE user_id = ? ORDER BY id ASC",
        )
        .bind(user_id)
        .fetch_all(&self.pool)
        .await?;

        Ok(rows
            .into_iter()
            .map(|row| UserTokenView {
                id: row.id,
                desc: row.desc,
                token: "****".to_string(),
                expiry: row.expiry,
                user_id: row.user_id,
            })
            .collect())
    }

    pub async fn add_token(
        &self,
        username: &str,
        expiry_days: i64,
        desc: &str,
    ) -> AppResult<String> {
        let Some(user_id) =
            sqlx::query_scalar::<_, i64>("SELECT id FROM users WHERE username = ? LIMIT 1")
                .bind(username)
                .fetch_optional(&self.pool)
                .await?
        else {
            return Err(AppError::NotFound("user not found".to_string()));
        };

        let token = random_token(32);
        let expiry = if expiry_days > 0 {
            OffsetDateTime::now_utc().unix_timestamp() + (expiry_days * 86_400)
        } else {
            0
        };

        sqlx::query("INSERT INTO tokens (desc, token, expiry, user_id) VALUES (?, ?, ?, ?)")
            .bind(desc)
            .bind(&token)
            .bind(expiry)
            .bind(user_id)
            .execute(&self.pool)
            .await?;

        Ok(token)
    }

    pub async fn delete_token(&self, id: i64) -> AppResult<()> {
        sqlx::query("DELETE FROM tokens WHERE id = ?").bind(id).execute(&self.pool).await?;
        Ok(())
    }

    async fn cleanup_expired_sessions(&self) -> AppResult<()> {
        sqlx::query("DELETE FROM user_sessions WHERE expires_at <= ?")
            .bind(OffsetDateTime::now_utc().unix_timestamp())
            .execute(&self.pool)
            .await?;
        Ok(())
    }
}

#[derive(Debug, Clone, Serialize)]
pub struct AuthenticatedUserView {
    pub id: i64,
    pub username: String,
    pub last_login: String,
}

#[derive(Debug, Clone, Serialize)]
pub struct UserTokenView {
    pub id: i64,
    pub desc: String,
    pub token: String,
    pub expiry: i64,
    pub user_id: i64,
}

#[derive(Debug, Clone, Serialize)]
pub struct LoadedToken {
    pub token: String,
    pub expiry: i64,
    pub username: String,
}

pub fn build_session_cookie(
    session_id: &str,
    session_max_age_minutes: i64,
    secure: bool,
) -> Cookie<'static> {
    let mut builder = Cookie::build((SESSION_COOKIE, session_id.to_string()))
        .path("/")
        .http_only(true)
        .same_site(SameSite::Lax)
        .secure(secure);

    if session_max_age_minutes > 0 {
        builder = builder.max_age(Duration::minutes(session_max_age_minutes));
    }

    builder.build()
}

pub fn removal_cookie(secure: bool) -> Cookie<'static> {
    Cookie::build((SESSION_COOKIE, String::new()))
        .path("/")
        .http_only(true)
        .same_site(SameSite::Lax)
        .secure(secure)
        .max_age(Duration::seconds(0))
        .build()
}

pub fn hash_password(password: &str) -> AppResult<String> {
    if password.is_empty() {
        return Err(AppError::Validation("password can not be empty".to_string()));
    }

    let salt = SaltString::generate(&mut OsRng);
    let argon2 = Argon2::default();
    let password_hash = argon2
        .hash_password(password.as_bytes(), &salt)
        .map_err(|error| AppError::Validation(error.to_string()))?;

    Ok(password_hash.to_string())
}

pub fn verify_password(stored_password: &str, plain_password: &str) -> AppResult<(bool, bool)> {
    if !is_password_hash(stored_password) {
        return Ok((stored_password == plain_password, stored_password == plain_password));
    }

    let parsed_hash = PasswordHash::new(stored_password)
        .map_err(|error| AppError::Validation(error.to_string()))?;
    let argon2 = Argon2::default();
    let verified = argon2.verify_password(plain_password.as_bytes(), &parsed_hash).is_ok();
    Ok((verified, false))
}

pub fn is_password_hash(value: &str) -> bool {
    value.starts_with("$argon2id$")
}

fn random_token(byte_len: usize) -> String {
    let mut bytes = vec![0_u8; byte_len];
    rand::rng().fill_bytes(&mut bytes);
    URL_SAFE_NO_PAD.encode(bytes)
}

pub fn is_secure_request(forwarded_proto: Option<&str>, has_tls: bool) -> bool {
    if has_tls {
        return true;
    }
    forwarded_proto.map(|proto| proto.eq_ignore_ascii_case("https")).unwrap_or(false)
}

pub fn maybe_default_password_display(stored_password: &str, username: &str) -> String {
    if is_password_hash(stored_password) && username == DEFAULT_ADMIN_USERNAME {
        if matches!(verify_password(stored_password, DEFAULT_ADMIN_PASSWORD), Ok((true, _))) {
            return DEFAULT_ADMIN_PASSWORD.to_string();
        }
        return "<hidden: stored securely>".to_string();
    }

    if is_password_hash(stored_password) {
        return "<hidden: stored securely>".to_string();
    }

    warn!("plaintext password encountered while formatting admin output");
    stored_password.to_string()
}
