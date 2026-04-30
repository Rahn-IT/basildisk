use std::time::Duration;

use argon2::{
    Argon2,
    password_hash::{PasswordHash, PasswordHasher, PasswordVerifier, SaltString, rand_core::OsRng},
};
use axum::{
    extract::{self, FromRequestParts, Path, State},
    http::request::{self},
    middleware::Next,
    response::{Html, IntoResponse, Redirect, Response},
};
use axum_extra::extract::{
    Form,
    cookie::{Cookie, CookieJar, SameSite},
};
use serde::{Deserialize, Serialize};
use sqlx::SqlitePool;
use uuid::Uuid;

use crate::{AppState, error::AppError};

pub const SESSION_COOKIE_NAME: &str = "session_id";
const SESSION_DURATION_SECONDS: i64 = 60 * 60 * 24 * 30;

#[derive(Debug, Clone, serde::Serialize)]
pub struct CurrentUser {
    pub(crate) id: Uuid,
    pub(crate) name: String,
    pub(crate) is_admin: bool,
}

impl FromRequestParts<AppState> for CurrentUser {
    type Rejection = AppError;

    async fn from_request_parts(
        parts: &mut request::Parts,
        _state: &AppState,
    ) -> Result<Self, Self::Rejection> {
        parts
            .extensions
            .get::<CurrentUser>()
            .cloned()
            .ok_or_else(|| AppError::unauthorized("Authentication required."))
    }
}

pub struct RequireAdmin;

impl<S> FromRequestParts<S> for RequireAdmin
where
    S: Send + Sync,
{
    type Rejection = AppError;

    async fn from_request_parts(
        parts: &mut request::Parts,
        _state: &S,
    ) -> Result<Self, Self::Rejection> {
        let current_user = parts
            .extensions
            .get::<CurrentUser>()
            .cloned()
            .ok_or_else(|| AppError::unauthorized("Authentication required."))?;

        if current_user.is_admin {
            Ok(Self)
        } else {
            Err(AppError::forbidden(
                "Only admin users can access this endpoint.",
            ))
        }
    }
}

pub async fn auth_middleware(
    State(state): State<AppState>,
    jar: CookieJar,
    mut request: extract::Request,
    next: Next,
) -> Response {
    let path = request.uri().path().to_string();
    if path.starts_with("/static/") {
        return next.run(request).await;
    }

    let has_users = match has_users(&state.db).await {
        Ok(value) => value,
        Err(err) => return err.into_response(),
    };

    if !has_users {
        if path == "/setup" {
            return next.run(request).await;
        }
        return axum::response::Redirect::to("/setup").into_response();
    }

    if path == "/setup" {
        return axum::response::Redirect::to("/login").into_response();
    }

    if path == "/login" {
        return next.run(request).await;
    }

    let session_id = match read_session_cookie(&jar) {
        Some(id) => id,
        None => return axum::response::Redirect::to("/login").into_response(),
    };

    let current_user = match resolve_current_user_from_session(&state.db, session_id).await {
        Ok(Some(user)) => user,
        Ok(None) => return axum::response::Redirect::to("/login").into_response(),
        Err(err) => return err.into_response(),
    };

    request.extensions_mut().insert(current_user);
    next.run(request).await
}

#[derive(Debug, Clone)]
pub struct User {
    pub id: Uuid,
    pub name: String,
    pub is_admin: i64,
    pub password_hash: String,
}

impl User {
    fn as_current_user(&self) -> CurrentUser {
        CurrentUser {
            id: self.id,
            name: self.name.clone(),
            is_admin: self.is_admin != 0,
        }
    }
}

#[derive(Debug, Serialize)]
struct UserListView {
    users: Vec<UserListItem>,
    current_user_id: Uuid,
    is_admin: bool,
}

#[derive(Debug, Serialize)]
struct UserListItem {
    id: Uuid,
    name: String,
    is_admin: bool,
}

#[derive(Debug, Serialize)]
struct DeleteUserConfirmView {
    id: Uuid,
    name: String,
    role: String,
    show_users_link: bool,
}

#[derive(Debug, Serialize)]
struct LoginView {
    has_error: bool,
    error_message: Option<String>,
}

#[derive(Debug, Serialize)]
struct SetupView {
    has_error: bool,
    error_message: Option<String>,
}

#[derive(Debug, Deserialize)]
pub struct LoginForm {
    name: String,
    password: String,
}

#[derive(Debug, Deserialize)]
pub struct SetupForm {
    name: String,
    password: String,
    password_confirm: String,
}

#[derive(Debug, Deserialize)]
pub struct CreateUserForm {
    name: String,
    password: String,
    is_admin: Option<String>,
}

pub async fn has_users(db: &SqlitePool) -> Result<bool, AppError> {
    let count = sqlx::query_scalar!("SELECT COUNT(*) as \"count!: i64\" FROM users")
        .fetch_one(db)
        .await?;
    Ok(count > 0)
}

pub async fn resolve_current_user_from_session(
    db: &SqlitePool,
    session_id: Uuid,
) -> Result<Option<CurrentUser>, AppError> {
    let valid_since = unix_now().saturating_sub(SESSION_DURATION_SECONDS);
    let user = sqlx::query_as!(
        User,
        r#"
        SELECT
            users.id as "id: uuid::Uuid",
            users.name,
            users.is_admin,
            users.password_hash
        FROM user_sessions
        INNER JOIN users ON users.id = user_sessions.user_id
        WHERE user_sessions.id = $1
            AND user_sessions.created_at > $2
        LIMIT 1
        "#,
        session_id,
        valid_since
    )
    .fetch_optional(db)
    .await?;

    Ok(user.map(|value| value.as_current_user()))
}

pub fn read_session_cookie(jar: &CookieJar) -> Option<Uuid> {
    jar.get(SESSION_COOKIE_NAME)
        .and_then(|cookie| Uuid::parse_str(cookie.value()).ok())
}

fn require_admin(user: &CurrentUser) -> Result<(), AppError> {
    if user.is_admin {
        Ok(())
    } else {
        Err(AppError::forbidden(
            "Only admin users can access this page.",
        ))
    }
}

pub async fn login_get(State(state): State<AppState>) -> Result<Response, AppError> {
    if !has_users(&state.db).await? {
        return Ok(Redirect::to("/setup").into_response());
    }
    render_login(&state, false).map(IntoResponse::into_response)
}

pub async fn login_post(
    State(state): State<AppState>,
    jar: CookieJar,
    Form(form): Form<LoginForm>,
) -> Result<Response, AppError> {
    if !has_users(&state.db).await? {
        return Ok(Redirect::to("/setup").into_response());
    }

    let login_name = form.name.trim().to_string();
    let user = sqlx::query_as!(
        User,
        r#"
        SELECT
            id as "id: uuid::Uuid",
            name,
            is_admin,
            password_hash
        FROM users
        WHERE LOWER(name) = LOWER($1)
        LIMIT 1
        "#,
        login_name
    )
    .fetch_optional(&state.db)
    .await?;

    let Some(user) = user else {
        return render_login(&state, true).map(IntoResponse::into_response);
    };

    if !verify_password(&user.password_hash, &form.password) {
        return render_login(&state, true).map(IntoResponse::into_response);
    }

    let session_id = Uuid::new_v4();
    let now = unix_now();
    sqlx::query!(
        "INSERT INTO user_sessions (id, user_id, created_at) VALUES ($1, $2, $3)",
        session_id,
        user.id,
        now
    )
    .execute(&state.db)
    .await?;

    let cookie = Cookie::build((SESSION_COOKIE_NAME, session_id.to_string()))
        .path("/")
        .http_only(true)
        .same_site(SameSite::Lax)
        .build();

    Ok((jar.add(cookie), Redirect::to("/")).into_response())
}

pub async fn setup_get(State(state): State<AppState>) -> Result<Response, AppError> {
    if has_users(&state.db).await? {
        return Ok(Redirect::to("/login").into_response());
    }
    render_setup(&state, None)
}

pub async fn setup_post(
    State(state): State<AppState>,
    Form(form): Form<SetupForm>,
) -> Result<Response, AppError> {
    if has_users(&state.db).await? {
        return Ok(Redirect::to("/login").into_response());
    }

    let name = form.name.trim();
    if name.is_empty() {
        return render_setup(&state, Some("Username cannot be empty."));
    }
    if form.password.len() < 8 {
        return render_setup(&state, Some("Password must be at least 8 characters."));
    }
    if form.password != form.password_confirm {
        return render_setup(&state, Some("Passwords do not match."));
    }

    let setup_user_id = Uuid::new_v4();
    let setup_created_at = unix_now();
    let setup_password_hash = hash_password(&form.password)?;
    sqlx::query!(
        "INSERT INTO users (id, name, is_admin, created_at, password_hash) VALUES ($1, $2, $3, $4, $5)",
        setup_user_id,
        name,
        1_i64,
        setup_created_at,
        setup_password_hash
    )
    .execute(&state.db)
    .await?;

    Ok(Redirect::to("/login").into_response())
}

pub async fn logout_post(
    State(state): State<AppState>,
    jar: CookieJar,
) -> Result<(CookieJar, Redirect), AppError> {
    if let Some(session_id) = read_session_cookie(&jar) {
        let _ = sqlx::query!("DELETE FROM user_sessions WHERE id = $1", session_id)
            .execute(&state.db)
            .await;
    }

    let removal_cookie = Cookie::build((SESSION_COOKIE_NAME, "")).path("/").build();

    Ok((jar.remove(removal_cookie), Redirect::to("/login")))
}

pub async fn index(
    State(state): State<AppState>,
    current_user: CurrentUser,
) -> Result<Html<String>, AppError> {
    require_admin(&current_user)?;

    let users = sqlx::query_as!(
        User,
        r#"
        SELECT
            id as "id: uuid::Uuid",
            name,
            is_admin,
            password_hash
        FROM users
        ORDER BY name ASC
        "#
    )
    .fetch_all(&state.db)
    .await?;

    let view = UserListView {
        users: users
            .into_iter()
            .map(|user| UserListItem {
                id: user.id,
                name: user.name,
                is_admin: user.is_admin != 0,
            })
            .collect(),
        current_user_id: current_user.id,
        is_admin: true,
    };

    let template = state
        .jinja
        .get_template("users.html")
        .expect("template is loaded");
    let rendered = template.render(view)?;

    Ok(Html(rendered))
}

pub async fn create_post(
    State(state): State<AppState>,
    current_user: CurrentUser,
    Form(form): Form<CreateUserForm>,
) -> Result<Redirect, AppError> {
    require_admin(&current_user)?;

    let name = form.name.trim();
    if name.is_empty() {
        return Err(AppError::conflict("User name cannot be empty."));
    }
    if form.password.len() < 8 {
        return Err(AppError::conflict(
            "Password must be at least 8 characters.",
        ));
    }

    let exists = sqlx::query_scalar!(
        r#"
        SELECT id as "id: uuid::Uuid"
        FROM users
        WHERE LOWER(name) = LOWER($1)
        "#,
        name
    )
    .fetch_optional(&state.db)
    .await?;

    if exists.is_some() {
        return Err(AppError::conflict("A user with this name already exists."));
    }

    let created_user_id = Uuid::new_v4();
    let created_is_admin = if form.is_admin.is_some() {
        1_i64
    } else {
        0_i64
    };
    let created_at = unix_now();
    let created_password_hash = hash_password(&form.password)?;
    sqlx::query!(
        "INSERT INTO users (id, name, is_admin, created_at, password_hash) VALUES ($1, $2, $3, $4, $5)",
        created_user_id,
        name,
        created_is_admin,
        created_at,
        created_password_hash
    )
    .execute(&state.db)
    .await?;

    Ok(Redirect::to("/users"))
}

pub async fn delete_post(
    State(state): State<AppState>,
    current_user: CurrentUser,
    Path(id): Path<Uuid>,
) -> Result<Redirect, AppError> {
    require_admin(&current_user)?;

    if current_user.id == id {
        return Err(AppError::conflict(
            "You cannot delete your own active user.",
        ));
    }

    let target = sqlx::query_as!(
        User,
        r#"
        SELECT
            id as "id: uuid::Uuid",
            name,
            is_admin,
            password_hash
        FROM users
        WHERE id = $1
        LIMIT 1
        "#,
        id
    )
    .fetch_optional(&state.db)
    .await?;

    let Some(target) = target else {
        return Err(AppError::not_found_for(
            "User",
            format!("No user exists for id: {}", id),
        ));
    };

    if target.is_admin != 0 {
        let admin_count =
            sqlx::query_scalar!("SELECT COUNT(*) as \"count!: i64\" FROM users WHERE is_admin = 1")
                .fetch_one(&state.db)
                .await?;

        if admin_count <= 1 {
            return Err(AppError::conflict("At least one admin user must remain."));
        }
    }

    let mut tx = state.db.begin().await?;
    sqlx::query!("DELETE FROM user_sessions WHERE user_id = $1", id)
        .execute(&mut *tx)
        .await?;

    sqlx::query!("DELETE FROM users WHERE id = $1", id)
        .execute(&mut *tx)
        .await?;
    tx.commit().await?;

    Ok(Redirect::to("/users"))
}

pub async fn delete_get(
    State(state): State<AppState>,
    current_user: CurrentUser,
    Path(id): Path<Uuid>,
) -> Result<Html<String>, AppError> {
    require_admin(&current_user)?;

    if current_user.id == id {
        return Err(AppError::conflict(
            "You cannot delete your own active user.",
        ));
    }

    let target = sqlx::query_as!(
        User,
        r#"
        SELECT
            id as "id: uuid::Uuid",
            name,
            is_admin,
            password_hash
        FROM users
        WHERE id = $1
        LIMIT 1
        "#,
        id
    )
    .fetch_optional(&state.db)
    .await?;

    let Some(target) = target else {
        return Err(AppError::not_found_for(
            "User",
            format!("No user exists for id: {}", id),
        ));
    };

    if target.is_admin != 0 {
        let admin_count =
            sqlx::query_scalar!("SELECT COUNT(*) as \"count!: i64\" FROM users WHERE is_admin = 1")
                .fetch_one(&state.db)
                .await?;

        if admin_count <= 1 {
            return Err(AppError::conflict("At least one admin user must remain."));
        }
    }

    let template = state
        .jinja
        .get_template("user_delete_confirm.html")
        .expect("template is loaded");
    let rendered = template.render(DeleteUserConfirmView {
        id: target.id,
        name: target.name,
        role: if target.is_admin != 0 {
            "Admin".to_string()
        } else {
            "User".to_string()
        },
        show_users_link: true,
    })?;

    Ok(Html(rendered))
}

fn hash_password(password: &str) -> Result<String, AppError> {
    let salt = SaltString::generate(&mut OsRng);
    Argon2::default()
        .hash_password(password.as_bytes(), &salt)
        .map(|hash| hash.to_string())
        .map_err(|err| AppError::internal(anyhow::anyhow!(err.to_string())))
}

fn verify_password(hash: &str, password: &str) -> bool {
    let parsed = match PasswordHash::new(hash) {
        Ok(parsed) => parsed,
        Err(_) => return false,
    };

    Argon2::default()
        .verify_password(password.as_bytes(), &parsed)
        .is_ok()
}

fn unix_now() -> i64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|duration| duration.as_secs() as i64)
        .unwrap_or(0)
}

fn render_login(state: &AppState, has_error: bool) -> Result<Html<String>, AppError> {
    let template = state
        .jinja
        .get_template("login.html")
        .expect("template is loaded");
    let rendered = template.render(LoginView {
        has_error,
        error_message: if has_error {
            Some("Invalid username or password.".to_string())
        } else {
            None
        },
    })?;
    Ok(Html(rendered))
}

fn render_setup(state: &AppState, error_message: Option<&str>) -> Result<Response, AppError> {
    let template = state
        .jinja
        .get_template("setup.html")
        .expect("template is loaded");
    let rendered = template.render(SetupView {
        has_error: error_message.is_some(),
        error_message: error_message.map(str::to_string),
    })?;
    Ok(Html(rendered).into_response())
}

pub async fn run_session_gc_scheduler(db: SqlitePool) {
    let mut interval = tokio::time::interval(Duration::from_secs(60 * 60));
    interval.tick().await;

    loop {
        interval.tick().await;

        match cleanup_expired_sessions(&db).await {
            Ok(0) => {
                println!("Session GC: no expired sessions found.");
            }
            Ok(count) => {
                println!("Session GC: deleted {} expired session(s).", count);
            }
            Err(err) => {
                eprintln!("Session GC failed: {}", err);
            }
        }
    }
}

pub async fn cleanup_expired_sessions(db: &SqlitePool) -> Result<u64, sqlx::Error> {
    let valid_since = unix_now().saturating_sub(SESSION_DURATION_SECONDS);
    let result = sqlx::query!(
        "DELETE FROM user_sessions WHERE created_at <= $1",
        valid_since
    )
    .execute(db)
    .await?;
    Ok(result.rows_affected())
}
