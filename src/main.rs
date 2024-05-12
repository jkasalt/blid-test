use axum::{
    extract::{rejection::FormRejection, Path, Query, State},
    http::{StatusCode, Uri},
    response::{Html, IntoResponse, Redirect, Result},
    routing::{delete, get, post},
    Form, Router,
};
use base64::prelude::*;
use dotenv_codegen::dotenv;
use once_cell::sync::Lazy;
use rand::{distributions::Alphanumeric, thread_rng, Rng};
use serde::{Deserialize, Serialize};
use serde_json::json;
use std::{
    collections::{HashMap, HashSet},
    sync::{Arc, Mutex},
};
use tera::{Context, Tera};
use tracing_subscriber::prelude::*;

type AppState = State<Arc<Mutex<Vec<Contact>>>>;

#[derive(Debug, Clone, Serialize, Deserialize, Default)]
struct Contact {
    first: String,
    last: String,
    phone: u32,
}

static TEMPLATES: Lazy<Tera> = Lazy::new(|| {
    let mut tera = match Tera::new("templates/**/*.html") {
        Ok(t) => t,
        Err(e) => {
            eprintln!("Parsing errors: {e}");
            ::std::process::exit(1)
        }
    };
    tera.autoescape_on(vec![".html"]);
    tera
});

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

async fn index() -> impl IntoResponse {
    Redirect::to("/contacts")
}

async fn contacts(
    State(contacts): AppState,
    Query(params): Query<HashMap<String, String>>,
) -> Result<impl IntoResponse, AppError> {
    let q = params.get("q");
    let mut contacts = contacts.lock().unwrap().clone();
    if let Some(q) = q {
        contacts.retain(|contact| contact.first.contains(q));
    }
    let page: usize = params
        .get("page")
        .and_then(|page| page.parse().ok())
        .unwrap_or(0);

    contacts = contacts.chunks(1).nth(page).unwrap_or_default().to_vec();
    tracing::debug!("{:?}", &contacts);
    let context = Context::from_serialize(json!({
        "query": q,
        "contacts": contacts,
        "page": page
    }))?;
    Ok(Html(TEMPLATES.render("index.html", &context)?))
}

async fn contacts_view(
    State(contacts): AppState,
    Path(id): Path<usize>,
) -> Result<impl IntoResponse, AppError> {
    if let Some(contact) = contacts.lock().unwrap().get(id) {
        let context = Context::from_serialize(json!({
            "contact": contact,
            "id": id,
        }))?;
        Ok((
            StatusCode::OK,
            Html(TEMPLATES.render("show.html", &context)?),
        ))
    } else {
        Ok((
            StatusCode::NOT_FOUND,
            Html(format!("Could not find contact with id {id}")),
        ))
    }
}

async fn new_contact_form() -> Result<impl IntoResponse, AppError> {
    let context = Context::from_serialize(json!({
        "contact": Contact::default(),
    }))?;
    Ok(Html(TEMPLATES.render("new.html", &context)?))
}

async fn edit_contact_form(
    Path(id): Path<usize>,
    State(contacts): AppState,
) -> Result<impl IntoResponse, AppError> {
    if let Some(contact) = contacts.lock().unwrap().get(id) {
        let context = Context::from_serialize(json!({
            "contact": contact,
            "id": id,
        }))?;
        Ok((
            StatusCode::OK,
            Html(TEMPLATES.render("edit.html", &context)?),
        ))
    } else {
        Ok((
            StatusCode::NOT_FOUND,
            Html(format!("Could not find contact with id {id}")),
        ))
    }
}

async fn new_contact_request(
    State(contacts): AppState,
    form: Result<Form<Contact>, FormRejection>,
) -> impl IntoResponse {
    let Ok(Form(contact)) = form else {
        return Redirect::to("/contacts/new");
    };
    let Ok(mut contacts) = contacts.lock() else {
        return Redirect::to("/contacts/new");
    };

    contacts.push(contact);
    Redirect::to("/contacts")
}

async fn edit_contact_request(
    State(contacts): AppState,
    Path(id): Path<usize>,
    form: Result<Form<Contact>, FormRejection>,
) -> impl IntoResponse {
    let Ok(Form(contact)) = form else {
        return Redirect::to(format!("/contacts/{id}/edit").as_str());
    };
    let Ok(mut contacts) = contacts.lock() else {
        return Redirect::to(format!("/contacts/{id}/edit").as_str());
    };

    if let Some(contact_ref) = contacts.get_mut(id) {
        *contact_ref = contact;
    }
    Redirect::to("/contacts")
}

async fn delete_contact_request(
    State(contacts): AppState,
    Path(id): Path<usize>,
) -> impl IntoResponse {
    contacts.lock().unwrap().swap_remove(id);
    Redirect::to("/contacts")
}

async fn send_spotify_code_request(
    State(s): State<Arc<Mutex<HashSet<String>>>>,
) -> Result<impl IntoResponse, AppError> {
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
    s.lock().unwrap().insert(state);
    Ok(Redirect::to(&uri.to_string()))
}

#[derive(Serialize, Deserialize, Debug)]
struct SpotifyAuthResponse {
    code: String,
    state: String,
}

async fn send_spotify_token_request(
    Query(q): Query<SpotifyAuthResponse>,
    State(s): State<Arc<Mutex<HashSet<String>>>>,
) -> Result<impl IntoResponse, AppError> {
    if s.lock().unwrap().take(&q.state).is_none() {
        tracing::warn!(
            "Attempting to find state string {} in state collection {:#?}, but it was not found",
            q.state,
            s.lock().unwrap()
        );
        return Ok((StatusCode::UNAUTHORIZED, String::from("Unauthorized")));
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

    tracing::debug!("{request:#?}");

    let response = request.send().await?;

    tracing::debug!("{response:#?}");

    let body: serde_json::Value = response.json().await?;

    Ok((StatusCode::OK, format!("{body:#?}")))
}

#[derive(Serialize, Deserialize, Debug)]
struct SpotifyToken {
    access_token: String,
    expires_in: u64,
    token_type: String,
}

#[tokio::main]
async fn main() -> anyhow::Result<()> {
    let state = Arc::new(Mutex::new(vec![
        Contact {
            first: "Alice".to_string(),
            last: "Hoho".to_string(),
            phone: 234,
        },
        Contact {
            first: "Bob".to_string(),
            last: "The builder".to_string(),
            phone: 123_123,
        },
    ]));

    let spotify_auth_requests_state = Arc::new(Mutex::new(HashSet::new()));

    tracing_subscriber::registry()
        .with(
            tracing_subscriber::EnvFilter::try_from_default_env().unwrap_or_else(|_| {
                // axum logs rejections from built-in extractors with the `axum::rejection`
                // target, at `TRACE` level. `axum::rejection=trace` enables showing those events
                "blid_test=debug,tower_http=debug,axum::rejection=trace".into()
            }),
        )
        .with(tracing_subscriber::fmt::layer())
        .init();

    let spotify_auth_routes = Router::new()
        .route("/", get(send_spotify_code_request))
        .route("/callback", get(send_spotify_token_request))
        .with_state(spotify_auth_requests_state);

    let app = Router::new()
        .route("/", get(index))
        .route("/contacts", get(contacts))
        .route("/contacts/:id", get(contacts_view))
        .route("/contacts/:id/edit", get(edit_contact_form))
        .route("/contacts/:id/edit", post(edit_contact_request))
        .route("/contacts/:id", delete(delete_contact_request))
        .route("/contacts/new", get(new_contact_form))
        .route("/contacts/new", post(new_contact_request))
        .nest("/auth", spotify_auth_routes)
        .with_state(state)
        .layer(tower_http::trace::TraceLayer::new_for_http());

    let listener = tokio::net::TcpListener::bind("0.0.0.0:3000").await?;
    axum::serve(listener, app).await?;

    Ok(())
}
