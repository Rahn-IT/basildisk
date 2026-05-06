use axum::{
    Router,
    http::{HeaderValue, header},
    routing::get,
};

use crate::AppState;

pub fn router() -> Router<AppState> {
    Router::new()
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
}
