/// Web/json interface to access settings
use actix_http::Payload;
use actix_session::{CookieSession, Session};
use actix_web::{
    error, http, middleware, web, App, Error, FromRequest, HttpRequest, HttpResponse, HttpServer,
    Responder,
};

use core::ops::Deref;
use futures::prelude::*;
use lazy_static::lazy_static;
use mime;
use serde::{Deserialize, Serialize};
use serde_json::json;
use std::collections::HashMap;
use std::error::Error as StdError;
use std::sync::Mutex;

use fluent_templates::{fs::LanguageIdentifier, static_loader, FluentLoader, Loader};
use tera::{Context, Tera};
use url::form_urlencoded;

mod config;
use crate::config::ConfigItem;

mod model;
use crate::model::Model;
use crate::model::Secret;

mod views;
use crate::views::{IndexPage, Page, SettingsPage, SubmittedPage, LOCALES, TERA};

/// Language to use when user did not specify any, or translation is not available at all
static DEFAULT_LANGUAGE: &str = "en-US";

struct Langs(Vec<LanguageIdentifier>);

impl AsRef<[LanguageIdentifier]> for Langs {
    fn as_ref(&self) -> &[LanguageIdentifier] {
        &self.0
    }
}

impl From<Vec<LanguageIdentifier>> for Langs {
    fn from(inner: Vec<LanguageIdentifier>) -> Self {
        Self(inner)
    }
}

impl FromRequest for Langs {
    type Error = actix_web::Error;
    type Future = future::Ready<Result<Self, Self::Error>>;
    type Config = ();

    fn from_request(req: &HttpRequest, _payload: &mut Payload) -> Self::Future {
        let langs = req
            .headers()
            .get(http::header::ACCEPT_LANGUAGE)
            .and_then(|v| v.to_str().ok())
            .unwrap_or(DEFAULT_LANGUAGE)
            .split(',')
            .filter_map(|lang| {
                lang.split(';')
                    .nth(0)
                    .and_then(|s| s.trim().parse::<LanguageIdentifier>().ok())
            })
            .collect::<Vec<_>>();
        future::ok(langs.into())
    }
}

fn render(
    template_name: &str,
    context: &Context,
    langs: &[LanguageIdentifier],
) -> tera::Result<String> {
    fn trace_error(e: tera::Error) -> tera::Error {
        if let Some(s) = e.source() {
            eprintln!("Tera error: {} ({})", e, s);
        } else {
            eprintln!("Tera error: {} (None)", e);
        }
        e
    }

    // FIXME: Only in debug build
    let mut t = TERA.lock().unwrap();
    if cfg!(debug_assertions) {
        t.full_reload().unwrap();
    }

    let default_lang = DEFAULT_LANGUAGE.parse().unwrap();
    let requested_lang = langs.first().unwrap_or(&default_lang);

    let lang = LOCALES
        .locales()
        .find(|&l| l == requested_lang)
        .map(|l| l.clone())
        .unwrap_or(default_lang);

    t.register_function(
        "fluent",
        FluentLoader::new(LOCALES.deref()).with_default_lang(lang),
    );
    t.render(template_name, context).map_err(trace_error)
}

fn render_page<T>(data: T, langs: &[LanguageIdentifier]) -> Result<HttpResponse, Error>
where
    T: Page + Serialize,
{
    let ctx = Context::from_serialize(data).map_err(error::ErrorInternalServerError)?;
    render(T::TEMPLATE_NAME, &ctx, langs)
        .map(|b| {
            HttpResponse::Ok()
                .content_type(mime::TEXT_HTML.as_ref())
                .body(b)
        })
        .map_err(error::ErrorInternalServerError)
}

fn render_json<T>(value: &T) -> Result<HttpResponse, Error>
where
    T: Serialize,
{
    serde_json::to_string(&value)
        .map(|b| {
            HttpResponse::Ok()
                .content_type(mime::APPLICATION_JSON.as_ref())
                .body(b)
        })
        .map_err(error::ErrorInternalServerError)
}

fn redirect(location: &str) -> HttpResponse {
    HttpResponse::Found()
        .header(http::header::LOCATION, location)
        .finish()
}

#[derive(Deserialize)]
struct CodeQuery {
    c: Option<String>,
}

/// Index page that asks user for one-time code
/// or redirects directly to the settings page if code is provided in query parameters
async fn index(
    model: web::Data<ModelState>,
    session: Session,
    query: web::Query<CodeQuery>,
    langs: Langs,
) -> impl Responder {
    match query.into_inner().c {
        Some(code) => access_settings(model, session, web::Form(AccessForm { code }), langs).await,
        None => render_page(IndexPage { error: None }, langs.as_ref()),
    }
}

#[derive(Serialize, Deserialize)]
struct AccessForm {
    code: String,
}

/// Provides access to settings after code verification
async fn access_settings(
    model: web::Data<ModelState>,
    session: Session,
    form: web::Form<AccessForm>,
    langs: Langs,
) -> Result<HttpResponse, Error> {
    let secret = {
        let mut m = model.inner.lock().unwrap();
        m.auth(&form.code)
    };
    match secret {
        Ok(secret) => {
            session.set(SESSION_SECRET, secret)?;
            Ok(redirect("./settings"))
        }
        Err(message) => render_page(
            IndexPage {
                error: Some(&message),
            },
            langs.as_ref(),
        ),
    }
}

/// Get settings using existing session
async fn get_settings(
    model: web::Data<ModelState>,
    session: Session,
    langs: Langs,
) -> Result<HttpResponse, Error> {
    let secret_opt = session.get::<Secret>(SESSION_SECRET)?;
    secret_opt
        .as_ref()
        .map(|secret| {
            let config_opt = {
                let mut m = model.inner.lock().unwrap();
                let s = m.settings(&secret).map(|v| v.clone());
                s
            };
            match config_opt {
                Ok(config) => {
                    render_page(SettingsPage { config: config }, langs.as_ref())
                }
                // TODO: Flash message
                Err(_) => Ok(redirect("./")),
            }
        })
        .unwrap_or_else(|| Ok(redirect("./")))
}

/// Sends updated settings to server
async fn post_settings(
    model: web::Data<ModelState>,
    session: Session,
    body: web::Bytes,
    langs: Langs,
) -> Result<HttpResponse, Error> {
    let secret: Secret = match session.get::<Secret>(SESSION_SECRET)? {
        Some(s) => s.to_owned(),
        None => {
            return Ok(redirect("./"));
        }
    };
    let form_data = form_urlencoded::parse(&body).into_owned();
    let values = form_data.collect::<HashMap<String, String>>();
    let result = {
        let mut m = model.inner.lock().unwrap();
        m.update_settings(&secret, values)
    };
    match result {
        Ok(_) => render_page(SubmittedPage {}, langs.as_ref()),
        Err(msg) => Ok(HttpResponse::BadRequest()
            .content_type("text/html")
            .body(msg)),
    }
}

async fn new_session(
    model: web::Data<ModelState>,
    config: web::Json<Vec<ConfigItem>>,
) -> Result<HttpResponse, Error> {
    let (key, secret) = model.inner.lock().unwrap().new_client(config.into_inner());
    render_json(&json!({
        "key": key,
        "secret": secret.to_string(),
    }))
}

#[derive(Deserialize)]
struct SessionQuery {
    sid: Secret,
}

/// End point for device to cancel web interface settings session
async fn end_session(
    model: web::Data<ModelState>,
    query: web::Query<SessionQuery>,
) -> Result<HttpResponse, Error> {
    let result = {
        let mut m = model.inner.lock().unwrap();
        m.remove_client(&query.sid)
    };
    let mut response = match result {
        Err(_) => HttpResponse::NotFound(),
        Ok(_) => HttpResponse::Ok(),
    };
    Ok(response.finish())
}

#[derive(Deserialize)]
struct PollQuery {
    sid: Secret,
    revision: u32,
}

/// End point for device to poll changes made by user
async fn poll_session(
    model: web::Data<ModelState>,
    query: web::Query<PollQuery>,
) -> Result<HttpResponse, Error> {
    let fut = {
        let mut m = model.inner.lock().unwrap();
        m.values(&query.sid, query.revision)
    };
    match fut.await {
        Ok(values) => render_json(&values),
        Err(_) => Ok(HttpResponse::NotFound().finish()),
    }
}

const SESSION_SECRET: &str = "secret";

struct ModelState {
    inner: Mutex<Model>,
}

impl From<Model> for ModelState {
    fn from(m: Model) -> Self {
        Self {
            inner: Mutex::new(m),
        }
    }
}

/// Configure routes
fn app_config(cfg: &mut web::ServiceConfig) {
    cfg.service(
        web::resource("/")
            .route(web::get().to(index))
            .route(web::post().to(access_settings)),
    )
    .service(
        web::resource("/settings")
            .route(web::get().to(get_settings))
            .route(web::post().to(post_settings)),
    )
    .route("/stb/new-session", web::post().to(new_session))
    .route("/stb/del-session", web::get().to(end_session))
    .route("/stb/poll", web::get().to(poll_session));
}

#[actix_rt::main]
async fn main() -> std::io::Result<()> {
    const VERSION: &str = env!("CARGO_PKG_VERSION");

    let args = clap::App::new("web settings server")
        .version(VERSION)
        .author("technic93")
        .about("Web interface to edit settings on remote embeded devices")
        .arg(
            clap::Arg::with_name("port")
                .long("port")
                .env("APP_PORT")
                .takes_value(true)
                .default_value("8000")
                .help("The port to listen to"),
        )
        .get_matches();

    let port = {
        let s = args.value_of("port").unwrap();
        s.parse::<i32>().unwrap_or_else(|e| {
            eprintln!("Bad port argument '{}', {}.", s, e);
            std::process::exit(1);
        })
    };

    env_logger::init();
    let addr = format!("127.0.0.1:{}", port);
    println!("Starting web server at {}", addr);

    // Global shared state variable
    let state = web::Data::new(ModelState::from(Model::new()));

    HttpServer::new(move || {
        // Remember to update middleware configuration in tests
        App::new()
            .app_data(state.clone())
            .wrap(middleware::Logger::default())
            .wrap(CookieSession::signed(&[0; 32]).secure(false))
            .configure(app_config)
    })
    .bind(addr)?
    .run()
    .await
}

#[cfg(test)]
mod tests {
    use super::*;
    use actix_http::httpmessage::HttpMessage;
    use actix_web::http::{header, StatusCode};
    use actix_web::test;
    use actix_web::test::TestServer;
    use config::{ConfigString, ConfigValue};
    use serde_json::Value;
    use std::sync::Arc;

    fn build_test_server() -> TestServer {
        env_logger::init();

        let state = web::Data::new(ModelState::from(Model::new()));

        test::start(move || {
            App::new()
                .app_data(state.clone())
                .wrap(middleware::Logger::default())
                .wrap(CookieSession::signed(&[0; 32]).secure(false))
                .configure(app_config)
        })
    }

    #[actix_rt::test]
    async fn happy_workflow() {
        let srv = Arc::new(build_test_server());

        // Stb sends configuration list to server
        let mut res = srv
            .post("/stb/new-session")
            .send_json(&json!([
                {
                    "name": "a",
                    "title": "TestA",
                    "type": "string",
                    "value": "qwerty",
                },
                {
                    "name": "b",
                    "title": "TestB",
                    "type": "integer",
                    "value": 33,
                    "min": 0,
                    "max": 100,
                },
                {
                    "name": "c",
                    "title": "TestC",
                    "type": "selection",
                    "value": "foo",
                    "options": [
                        {"value": "foo", "title": "Foo!" },
                        {"value": "bar", "title": "Bar!" },
                    ]
                },
                {
                    "name": "d",
                    "title": "TestD",
                    "type": "bool",
                    "value": true,
                },
            ]))
            .await
            .unwrap();

        assert_eq!(res.status(), StatusCode::OK);
        let body = res.body().await.unwrap();
        // srv.load_body(res).await.unwrap();
        let result = serde_json::from_slice::<Value>(&body).expect("valid json");
        assert!(result["key"].is_string());
        assert!(result["secret"].is_string());

        let key = result["key"].as_str().unwrap().to_owned();
        let secret = result["secret"].as_str().unwrap().to_owned();
        eprintln!("Created new session {}", secret);

        // Stb starts polling
        let srv1 = srv.clone();
        let (tx, rx) = futures::channel::oneshot::channel::<()>();

        actix_rt::spawn(async move {
            let uri = format!("/stb/poll?sid={}&revision={}", &secret, 0);

            // First reply returns revision=0 indicated that user has logged in
            eprintln!("Poll...");
            let mut res = srv1.get(&uri).send().await.unwrap();
            assert_eq!(res.status(), StatusCode::OK);
            let body = res.body().await.unwrap();
            // let body = srv.load_body(res).await.unwrap();
            let result = serde_json::from_slice::<model::Values>(&body).expect("valid json");

            eprintln!("Got revision r{}", result.revision);
            assert_eq!(result.revision, 0);
            let a = result.values.iter().find(|v| v.name == "a").unwrap();
            assert!(
                a.value
                    == ConfigValue::String(ConfigString {
                        value: "qwerty".to_owned()
                    })
            );

            // Future replies increment revision and give new values
            eprintln!("Poll...");
            let mut res = srv1.get(&uri).send().await.unwrap();
            assert_eq!(res.status(), StatusCode::OK);
            // let body = srv.load_body(res).await.unwrap();
            let body = res.body().await.unwrap();
            let result = serde_json::from_slice::<model::Values>(&body).expect("valid json");

            eprintln!("Got revision r{}", result.revision);
            assert_eq!(result.revision, 1);
            let a = result.values.iter().find(|v| v.name == "a").unwrap();
            assert!(
                a.value
                    == ConfigValue::String(ConfigString {
                        value: "sometext".to_owned()
                    })
            );

            // After Stb got updated values it usually deletes session
            eprintln!("End session");
            let res = srv1
                .get(format!("/stb/del-session?sid={}", &secret))
                .send()
                .await
                .unwrap();
            assert_eq!(res.status(), StatusCode::OK);

            // This routine is done
            tx.send(()).unwrap();
        });

        // Authorize user
        let res = srv
            .post("/")
            .send_form(&AccessForm { code: key })
            .await
            .unwrap();
        assert_eq!(res.status(), StatusCode::FOUND);
        assert_eq!(res.headers().get(header::LOCATION).unwrap(), "./settings");

        let cookies = res.cookies().unwrap();
        let cookie = cookies
            .iter()
            .find(|c| c.name() == "actix-session")
            .unwrap();

        // Follow redirect
        let mut res = srv
            .get("/settings")
            .cookie(cookie.to_owned())
            .send()
            .await
            .unwrap();
        assert_eq!(res.status(), StatusCode::OK);
        eprintln!("Authorized and accessed settings");

        let body = res.body().await.unwrap();
        assert!(std::str::from_utf8(&body).unwrap().find("qwerty").is_some());

        // Post new values
        let res = srv
            .post("/settings")
            .cookie(cookie.to_owned())
            .send_body("a=sometext")
            .await
            .unwrap();
        assert_eq!(res.status(), StatusCode::OK);
        eprintln!("Posted new valued");

        // Wait for Stb to poll all changes
        rx.await.unwrap();
    }
}
