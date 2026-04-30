use std::fmt;

use axum::{
    http::{HeaderValue, StatusCode, header},
    response::{IntoResponse, Response},
};
use serde::Serialize;

#[derive(Debug)]
pub struct AppError {
    status: StatusCode,
    message: String,
    not_found_title: Option<String>,
}

impl AppError {
    pub(crate) fn internal<E>(err: E) -> Self
    where
        E: Into<anyhow::Error>,
    {
        Self {
            status: StatusCode::INTERNAL_SERVER_ERROR,
            message: err.into().to_string(),
            not_found_title: None,
        }
    }

    pub fn not_found_for(title: impl Into<String>, message: impl Into<String>) -> Self {
        Self {
            status: StatusCode::NOT_FOUND,
            message: message.into(),
            not_found_title: Some(title.into()),
        }
    }

    pub fn conflict(message: impl Into<String>) -> Self {
        Self {
            status: StatusCode::CONFLICT,
            message: message.into(),
            not_found_title: None,
        }
    }

    pub fn forbidden(message: impl Into<String>) -> Self {
        Self {
            status: StatusCode::FORBIDDEN,
            message: message.into(),
            not_found_title: None,
        }
    }

    pub fn unauthorized(message: impl Into<String>) -> Self {
        Self {
            status: StatusCode::UNAUTHORIZED,
            message: message.into(),
            not_found_title: None,
        }
    }
}

impl<E> From<E> for AppError
where
    E: Into<anyhow::Error>,
{
    fn from(err: E) -> Self {
        Self::internal(err)
    }
}

impl fmt::Display for AppError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "{}", self.message)
    }
}

impl IntoResponse for AppError {
    fn into_response(self) -> Response {
        let message = self.message;

        if self.status == StatusCode::NOT_FOUND
            || self.status == StatusCode::CONFLICT
            || self.status == StatusCode::FORBIDDEN
            || self.status == StatusCode::UNAUTHORIZED
        {
            let (title, button_label, button_href): (String, &str, &str) =
                if self.status == StatusCode::NOT_FOUND {
                    (
                        format!(
                            "{} Not Found",
                            self.not_found_title.as_deref().unwrap_or("Site")
                        ),
                        "Back Home",
                        "/",
                    )
                } else if self.status == StatusCode::FORBIDDEN {
                    ("Forbidden".to_string(), "Back Home", "/")
                } else if self.status == StatusCode::UNAUTHORIZED {
                    ("Unauthorized".to_string(), "Login", "/login")
                } else {
                    ("Cannot Save Changes".to_string(), "Back Home", "/")
                };

            let mut jinja = minijinja::Environment::new();
            minijinja_embed::load_templates!(&mut jinja);
            let rendered = jinja
                .get_template("error.html")
                .expect("template is loaded")
                .render(ErrorView {
                    title,
                    message: message.clone(),
                    button_label: button_label.to_string(),
                    button_href: button_href.to_string(),
                });

            if let Ok(html) = rendered {
                return (
                    self.status,
                    [(
                        header::CONTENT_TYPE,
                        HeaderValue::from_static(mime::TEXT_HTML_UTF_8.as_ref()),
                    )],
                    html,
                )
                    .into_response();
            }
        }

        (
            StatusCode::INTERNAL_SERVER_ERROR,
            format!("Something went wrong: {}", message),
        )
            .into_response()
    }
}

#[derive(Debug, Serialize)]
struct ErrorView {
    title: String,
    message: String,
    button_label: String,
    button_href: String,
}
