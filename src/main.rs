use askama_axum::Template;
use axum::{
    extract::{Query, State},
    http::{header, HeaderMap, StatusCode, Uri},
    response::{IntoResponse, Redirect, Result},
    routing::get,
    Router,
};
use base64::prelude::*;
use dotenv_codegen::dotenv;
use rand::{distributions::Alphanumeric, thread_rng, Rng};
use serde::{Deserialize, Serialize};
use serde_json::json;
use std::{
    collections::{HashMap, HashSet},
    sync::{Arc, Mutex},
};
use tracing_subscriber::prelude::*;

mod cookie_manager;

type AppState = State<Arc<Mutex<AppStateInner>>>;

#[derive(Debug, Default)]
struct AppStateInner {
    code_states: HashSet<String>,
    sessions: HashMap<String, SpotifyToken>,
}

fn random_alphanum(len: usize) -> String {
    thread_rng()
        .sample_iter(Alphanumeric)
        .map(char::from)
        .take(len)
        .collect()
}

struct AppError(anyhow::Error);

impl IntoResponse for AppError {
    fn into_response(self) -> axum::response::Response {
        (StatusCode::INTERNAL_SERVER_ERROR, self.0.to_string()).into_response()
    }
}

impl<E> From<E> for AppError
where
    E: Into<anyhow::Error>,
{
    fn from(value: E) -> Self {
        Self(value.into())
    }
}

#[derive(Template)]
#[template(path = "index.html")]
struct MainTemplate {}

async fn contacts() -> impl IntoResponse {
    MainTemplate {}
}

async fn send_spotify_code_request(State(s): AppState) -> Result<impl IntoResponse, AppError> {
    let state = random_alphanum(16);
    let qs = serde_qs::to_string(&json!({
        "response_type": "code",
        "client_id": dotenv!("CLIENT_ID"),
        "scope": "streaming user-read-email user-read-private",
        "redirect_uri": "http://localhost:3000/auth/callback",
        "state": state,
    }))?;
    tracing::debug!("qs: {qs:#?}");
    let uri = Uri::builder()
        .scheme("https")
        .authority("accounts.spotify.com")
        .path_and_query(format!("/authorize/?{qs}"))
        .build()?;
    tracing::debug!("uri: {uri}");
    s.lock().unwrap().code_states.insert(state);
    Ok(Redirect::to(&uri.to_string()))
}

#[derive(Serialize, Deserialize, Debug)]
struct SpotifyAuthResponse {
    code: String,
    state: String,
}

async fn send_spotify_token_request(
    Query(q): Query<SpotifyAuthResponse>,
    State(s): AppState,
) -> Result<impl IntoResponse, AppError> {
    if s.lock().unwrap().code_states.take(&q.state).is_none() {
        tracing::warn!(
            "Attempting to find state string {} in state collection {:#?}, but it was not found",
            q.state,
            s.lock().unwrap(),
        );
        return Ok((StatusCode::UNAUTHORIZED, "Unauthorized").into_response());
    }

    let client = reqwest::Client::new();

    let request = client
        .post("https://accounts.spotify.com/api/token")
        .form(&json!({
            "code": q.code,
            "redirect_uri": "http://localhost:3000/auth/callback",
            "grant_type": "authorization_code"
        }))
        .header(
            "Authorization",
            format!(
                "Basic {}",
                BASE64_STANDARD.encode(format!(
                    "{}:{}",
                    dotenv!("CLIENT_ID"),
                    dotenv!("CLIENT_SECRET")
                )),
            ),
        );

    let response = request.send().await?;
    let token: SpotifyToken = response.json().await?;
    let max_age = token.expires_in;
    let mut session_id = random_alphanum(32);
    loop {
        let is_duplicate = s.lock().unwrap().sessions.contains_key(&session_id);
        if is_duplicate {
            session_id = random_alphanum(32);
        } else {
            break;
        }
    }
    s.lock().unwrap().sessions.insert(session_id.clone(), token);

    Ok((
        [(
            header::SET_COOKIE,
            format!("session_id={session_id}; Max-Age={max_age}"),
        )],
        Redirect::to("/"),
    )
        .into_response())
}

fn get_session(cookies: &str) -> Option<&str> {
    cookies
        .split(';')
        .map(str::trim)
        .filter_map(|s| s.split_once('='))
        .find_map(|(key, val)| (key == "session_id").then_some(val))
}

async fn test_session(State(s): AppState, headers: HeaderMap) -> impl IntoResponse {
    let Some(cookies) = headers.get("Cookie") else {
        return "false";
    };
    let Some(session_id) = get_session(cookies.to_str().unwrap()) else {
        return "false";
    };

    if s.lock().unwrap().sessions.contains_key(session_id) {
        "true"
    } else {
        "false"
    }
}

async fn get_token(headers: HeaderMap) -> impl IntoResponse {
    if let Some(session_id) = headers
        .get("Cookie")
        .and_then(|cookies| cookies.to_str().ok())
        .and_then(get_session)
    {
        session_id.to_owned().into_response()
    } else {
        StatusCode::NOT_FOUND.into_response()
    }
}

#[derive(Serialize, Deserialize, Debug)]
struct SpotifyToken {
    access_token: String,
    refresh_token: String,
    expires_in: u64,
    token_type: String,
}

#[tokio::main]
async fn main() -> anyhow::Result<()> {
    let app_state = Arc::new(Mutex::new(AppStateInner::default()));
    tracing_subscriber::registry()
        .with(
            tracing_subscriber::EnvFilter::try_from_default_env().unwrap_or_else(|_| {
                // axum logs rejections from built-in extractors with the `axum::rejection`
                // target, at `TRACE` level. `axum::rejection=trace` enables showing those events
                "blid_test=debug,tower_http=debug,axum::rejection=trace".into()
            }),
        )
        .with(tracing_subscriber::fmt::layer().pretty())
        .init();

    let spotify_auth_routes = Router::new()
        .route("/", get(send_spotify_code_request))
        .route("/callback", get(send_spotify_token_request))
        .route("/test-session", get(test_session))
        .with_state(app_state);

    let app = Router::new()
        .route("/", get(contacts))
        .nest("/auth", spotify_auth_routes)
        .layer(tower_http::trace::TraceLayer::new_for_http());

    let listener = tokio::net::TcpListener::bind("0.0.0.0:3000").await?;
    axum::serve(listener, app).await?;

    Ok(())
}
