use std::{path::Path, sync::Arc};

use axum::{
    Router,
    extract::State,
    http::{HeaderValue, header},
    middleware,
    response::Html,
    routing::{get, post},
};
use serde::Serialize;
use sqlx::{Sqlite, SqlitePool, migrate::MigrateDatabase};

pub mod error;
mod users;

const DB_PATH: &str = "./db/db.sqlite";

#[derive(Debug, Clone)]
struct AppState {
    db: SqlitePool,
    jinja: Arc<minijinja::Environment<'static>>,
}

#[derive(Serialize)]
struct Home {
    is_admin: bool,
}

#[tokio::main]
async fn main() {
    if !tokio::fs::try_exists(DB_PATH).await.unwrap() {
        tokio::fs::create_dir_all(Path::new(DB_PATH).parent().unwrap())
            .await
            .unwrap();
        Sqlite::create_database(DB_PATH).await.unwrap();
    }

    let db = SqlitePool::connect(DB_PATH).await.unwrap();
    sqlx::migrate!("./migrations").run(&db).await.unwrap();

    let mut jinja = minijinja::Environment::new();
    minijinja_embed::load_templates!(&mut jinja);

    let state = AppState {
        db: db.clone(),
        jinja: Arc::new(jinja),
    };

    tokio::spawn(async move {
        users::run_session_gc_scheduler(db).await;
    });

    // build our application with a route
    let app = router()
        .layer(middleware::from_fn_with_state(
            state.clone(),
            users::auth_middleware,
        ))
        .with_state(state);

    // run our app with hyper, listening globally on port 3000
    let addr = "0.0.0.0:4040";
    let listener = tokio::net::TcpListener::bind(addr).await.unwrap();
    println!("Starting webserver on: http://{}", addr);
    axum::serve(listener, app).await.unwrap();
}

fn router() -> Router<AppState> {
    let admin_routes = Router::new()
        .route("/users", get(users::index).post(users::create_post))
        .route(
            "/users/{id}/delete",
            get(users::delete_get).post(users::delete_post),
        )
        .route_layer(middleware::from_extractor::<users::RequireAdmin>());

    Router::new()
        // `GET /` goes to `root`
        .route("/", get(root))
        .route("/setup", get(users::setup_get).post(users::setup_post))
        .route("/login", get(users::login_get).post(users::login_post))
        .route("/logout", post(users::logout_post))
        .route(
            "/static/style.css",
            get((
                [(
                    header::CONTENT_TYPE,
                    HeaderValue::from_static(mime::TEXT_CSS_UTF_8.as_ref()),
                )],
                include_bytes!("../assets/static/style.css"),
            )),
        )
        .route(
            "/static/script.js",
            get((
                [(
                    header::CONTENT_TYPE,
                    HeaderValue::from_static(mime::APPLICATION_JAVASCRIPT_UTF_8.as_ref()),
                )],
                include_bytes!("../assets/static/script.js"),
            )),
        )
        .merge(admin_routes)
}

async fn root(State(state): State<AppState>, current_user: users::CurrentUser) -> Html<String> {
    let template = state
        .jinja
        .get_template("home.html")
        .expect("template is loaded");
    let rendered = template
        .render(&Home {
            is_admin: current_user.is_admin,
        })
        .unwrap();
    Html(rendered)
}
