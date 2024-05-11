use axum::{
    extract::{rejection::FormRejection, Path, Query, State},
    http::StatusCode,
    response::{Html, IntoResponse, Redirect, Result},
    routing::{delete, get, post},
    Form, Router,
};
use once_cell::sync::Lazy;
use reqwest::Client;
use serde::{Deserialize, Serialize};
use serde_json::json;
use std::{
    collections::HashMap,
    sync::{Arc, Mutex},
};
use tera::{Context, Tera};
use tracing::debug;

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
    debug!("{:?}", &contacts);
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

#[derive(Serialize, Deserialize, Debug)]
struct SpotifyToken {
    access_token: String,
    expires_in: u64,
    token_type: String,
}

#[tokio::main]
async fn main() -> anyhow::Result<()> {
    // let client = Client::new();
    // let res = client
    //     .post("https://accounts.spotify.com/api/token")
    //     .header("Content-Type", "aaplication/x-www-form-urlencoded")
    //     .form(&[
    //         ("grant_type", "client_credentials"),
    //         ("client_id", "client_id"),
    //         ("client_secret", "secret"),
    //     ])
    //     .send()
    //     .await?;
    // let token: SpotifyToken = res.json().await.unwrap();
    //
    // println!("{token:#?}");
    //
    // let result = client
    //     .get("https://api.spotify.com/v1/search")
    //     .query(&[("q", "classic"), ("type", "track")])
    //     .header(
    //         "Authorization",
    //         format!("{} {}", token.token_type, token.access_token),
    //     )
    //     .send()
    //     .await?
    //     .json::<serde_json::Value>()
    //     .await
    //     .unwrap();
    //
    // println!("{result:#?}");
    //
    // Ok(())

    #[cfg(debug_assert)]
    TEMPLATES.full_reload();

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

    tracing_subscriber::fmt::fmt()
        .pretty()
        .with_max_level(tracing::Level::DEBUG)
        .init();

    let app = Router::new()
        .route("/", get(index))
        .route("/contacts", get(contacts))
        .route("/contacts/:id", get(contacts_view))
        .route("/contacts/:id/edit", get(edit_contact_form))
        .route("/contacts/:id/edit", post(edit_contact_request))
        .route("/contacts/:id", delete(delete_contact_request))
        .route("/contacts/new", get(new_contact_form))
        .route("/contacts/new", post(new_contact_request))
        .with_state(state)
        .layer(tower_http::trace::TraceLayer::new_for_http());

    let listener = tokio::net::TcpListener::bind("0.0.0.0:3000").await?;
    axum::serve(listener, app).await?;

    Ok(())
}
