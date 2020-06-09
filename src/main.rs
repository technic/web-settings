/// Web/json interface to access settings
/// based on [Gotham](https://gotham.rs/) web framework
use futures::future::FutureExt;
use futures::prelude::*;
use gotham::handler::IntoHandlerFuture;
use gotham::handler::{HandlerError, HandlerFuture, IntoHandlerError, IntoResponse};
use gotham::helpers::http::response::create_empty_response;
use gotham::hyper::{body, Body, Response, StatusCode};
use gotham::middleware::session::{NewSessionMiddleware, SessionData};
use gotham::middleware::state::StateMiddleware;
use gotham::pipeline::single::single_pipeline;
use gotham::router::Router;
use gotham::state::{FromState, State};
use gotham_derive::{StateData, StaticResponseExtender};
use lazy_static::lazy_static;
use mime;
use serde::{Deserialize, Serialize};
use serde_json::json;
use std::borrow::Cow;
use std::collections::HashMap;
use std::ops::Deref;
use std::pin::Pin;
use std::sync::Arc;
use std::sync::Mutex;

use tera::{Context, Tera};
use url::form_urlencoded;

mod config;
use crate::config::ConfigItem;

mod model;
use crate::model::Model;
use crate::model::Secret;

// Assuming the Rust file is at the same level as the templates folder
// we can get a Tera instance that way:
lazy_static! {
    // Debug only
    pub static ref TERA: Mutex<Tera> = Mutex::new(Tera::new("templates/**/*.html").unwrap());
    // TODO: Relese
    // pub static ref TERA: Tera = Tera::new("templates/**/*.html").unwrap();
}

fn render(template_name: &str, context: &Context) -> tera::Result<String> {
    // FIXME: Only in debug build
    let mut t = TERA.lock().unwrap();
    t.full_reload().unwrap();
    t.render(template_name, context)
    // .map(|body| (mime::TEXT_HTML, body))
}

fn render_html(
    template_name: &str,
    context: &Context,
) -> Result<(mime::Mime, String), HandlerError> {
    render(template_name, context)
        .map(|body| (mime::TEXT_HTML, body))
        .map_err(server_error)
}

fn render_json<T>(value: &T) -> Result<(mime::Mime, String), HandlerError>
where
    T: Serialize,
{
    serde_json::to_string(&value)
        .map(|body| (mime::APPLICATION_JSON, body))
        .map_err(server_error)
}

fn bad_request<E>(e: E) -> HandlerError
where
    E: std::error::Error + Send + 'static,
{
    e.into_handler_error().with_status(StatusCode::BAD_REQUEST)
}

fn server_error<E>(e: E) -> HandlerError
where
    E: std::error::Error + Send + 'static,
{
    e.into_handler_error()
}

async fn extract_json<T>(state: &mut State) -> Result<T, HandlerError>
where
    T: serde::de::DeserializeOwned,
{
    body::to_bytes(Body::take_from(state))
        .await
        .map_err(bad_request)
        .and_then(|body| serde_json::from_slice::<T>(&body).map_err(bad_request))
}

pub fn redirect<L: Into<Cow<'static, str>>>(state: &State, location: L) -> Response<Body> {
    use hyper::header::LOCATION;
    let mut res = create_empty_response(state, StatusCode::FOUND);
    res.headers_mut()
        .insert(LOCATION, location.into().to_string().parse().unwrap());
    res
}

trait WithCode<B> {
    fn with_status(self, code: StatusCode) -> (StatusCode, mime::Mime, B);
}

impl<B> WithCode<B> for (mime::Mime, B)
where
    B: Into<Body>,
{
    fn with_status(self, code: StatusCode) -> (StatusCode, mime::Mime, B) {
        let (mime_type, body) = self;
        (code, mime_type, body)
    }
}

type HandlerResult = Result<(State, Response<Body>), (State, HandlerError)>;

fn to_handler_result<R>(state: State, result: Result<R, HandlerError>) -> HandlerResult
where
    R: IntoResponse,
{
    match result {
        Ok(r) => {
            let response = r.into_response(&state);
            Ok((state, response))
        }
        Err(e) => Err((state, e)),
    }
}

// macro_rules! try_block {
//     { $($token:tt)* } => {{
//         let l = || {
//             $($token)*
//         };
//         l()
//     }}
// }

/// Index page that asks user for one-time code
pub fn index(state: State) -> (State, Result<Response<Body>, HandlerError>) {
    let context = Context::new();
    let res = render_html("pages/index.html", &context).map(|body| body.into_response(&state));
    (state, res)
}

/// Provides access to settigns after code verification
pub fn access_settings(mut state: State) -> Pin<Box<HandlerFuture>> {
    let body_fut = body::to_bytes(Body::take_from(&mut state)).map_err(bad_request);
    let fut = body_fut.map(|b| {
        let result = b.and_then(|body_content| {
            let mut form_data = form_urlencoded::parse(&body_content).into_owned();
            let code = match form_data
                .find_map(|(key, value)| if key == "code" { Some(value) } else { None })
            {
                Some(code) => code,
                // TODO: We may want to return Err here
                None => return Ok(create_empty_response(&state, StatusCode::BAD_REQUEST)),
            };
            let secret = {
                let mut m = ModelState::borrow_from(&state).inner.lock().unwrap();
                m.auth(&code)
            };
            match secret {
                Ok(secret) => {
                    use std::ops::DerefMut;
                    let visit_data = SessionData::<Option<Secret>>::borrow_mut_from(&mut state);
                    *visit_data.deref_mut() = Some(secret);
                    return Ok(redirect(&state, "./settings"));
                }
                Err(message) => {
                    let mut ctx = Context::new();
                    ctx.insert("error", message);
                    return Ok(render_html("pages/index.html", &ctx)?.into_response(&state));
                }
            }
        });
        to_handler_result(state, result)
    });
    fut.boxed()
}

/// Get settings using existing session
pub fn get_settings(state: State) -> (State, Result<Response<Body>, HandlerError>) {
    let secret_opt = SessionData::<Option<Secret>>::borrow_from(&state).deref();
    let res = secret_opt
        .as_ref()
        .map(|secret| {
            let mut m = ModelState::borrow_from(&state).inner.lock().unwrap();
            match m.settings(&secret).clone() {
                Ok(config) => {
                    let ctx = Context::from_value(json!({ "config": config }))
                        .map_err(|e| e.into_handler_error())?;
                    let html = render_html("pages/settings.html", &ctx)?;
                    Ok(html.into_response(&state))
                }
                // TODO: Flash message
                Err(_) => Ok(redirect(&state, "./")),
            }
        })
        .unwrap_or_else(|| Ok(redirect(&state, "./")));
    (state, res)
}

/// Sends settings to device
pub fn post_settings(mut state: State) -> Pin<Box<HandlerFuture>> {
    let secret: Secret = match SessionData::<Option<Secret>>::borrow_from(&state).deref() {
        Some(s) => s.to_owned(),
        None => {
            let res = redirect(&state, "./");
            return (state, res).into_handler_future();
        }
    };
    async move {
        let response = body::to_bytes(Body::take_from(&mut state))
            .await
            .map_err(bad_request)
            .and_then(|body_content| {
                let form_data = form_urlencoded::parse(&body_content).into_owned();
                let values = form_data.collect::<HashMap<String, String>>();
                let result = {
                    let mut m = ModelState::borrow_from(&state).inner.lock().unwrap();
                    m.update_settings(&secret, values)
                };
                let res = match result {
                    Ok(_) => render_html("pages/submitted.html", &Context::new())?
                        .with_status(StatusCode::OK),
                    Err(_) => (StatusCode::BAD_REQUEST, mime::TEXT_HTML, String::new()),
                };
                Ok(res.into_response(&state))
            });
        to_handler_result(state, response)
    }
    .boxed()
}

/// End point for device to create new session
pub fn new_session(mut state: State) -> Pin<Box<HandlerFuture>> {
    async move {
        let response = extract_json::<Vec<ConfigItem>>(&mut state)
            .await
            .and_then(|config| {
                let mut m = ModelState::borrow_from(&state).inner.lock().unwrap();
                let (key, secret) = m.new_client(config);
                render_json(&json!({
                    "key": key,
                    "secret": secret.to_string(),
                }))
            });
        to_handler_result(state, response)
    }
    .boxed()
}

#[derive(Deserialize, StateData, StaticResponseExtender)]
struct SessionQuery {
    sid: String,
}

/// End point for device to cancel web interface settings session
pub fn end_session(state: State) -> (State, Response<Body>) {
    let result = {
        let q = SessionQuery::borrow_from(&state);
        let mut m = ModelState::borrow_from(&state).inner.lock().unwrap();
        m.remove_client(&q.sid)
    };
    let response = match result {
        Err(_) => create_empty_response(&state, StatusCode::NOT_FOUND),
        Ok(_) => create_empty_response(&state, StatusCode::OK),
    };
    (state, response)
}

#[derive(Deserialize, StateData, StaticResponseExtender)]
struct PollQuery {
    sid: String,
    revision: u32,
}

/// End point for device to poll changes made by user
pub fn poll_session(state: State) -> Pin<Box<HandlerFuture>> {
    let fut = {
        let q = PollQuery::borrow_from(&state);
        let mut m = ModelState::borrow_from(&state).inner.lock().unwrap();
        m.values(&q.sid, q.revision)
    };
    async move {
        let response = match fut.await {
            Ok(values) => render_json(&values).map(|r| r.with_status(StatusCode::OK)),
            Err(_) => Ok((StatusCode::NOT_FOUND, mime::APPLICATION_JSON, String::new())),
        };
        to_handler_result(state, response)
    }
    .boxed()
}

#[derive(Clone, StateData)]
struct ModelState {
    inner: Arc<Mutex<Model>>,
}

impl From<Model> for ModelState {
    fn from(m: Model) -> Self {
        Self {
            inner: Arc::new(Mutex::new(m)),
        }
    }
}

fn router() -> Router {
    use gotham::pipeline::*;
    use gotham::router::builder::*;

    let state_middleware = StateMiddleware::new(ModelState::from(Model::new()));
    let session_middleware = NewSessionMiddleware::default().with_session_type::<Option<Secret>>();

    let pipeline = new_pipeline()
        .add(state_middleware)
        .add(session_middleware)
        .build();
    let (chain, pipelines) = single_pipeline(pipeline);

    build_router(chain, pipelines, |route| {
        // web
        route.get("/").to(index);
        route.post("/").to(access_settings);
        route.get("/settings").to(get_settings);
        route.post("/settings").to(post_settings);

        // stb
        route.post("/stb/new-session").to(new_session);
        route
            .get("/stb/del-session")
            .with_query_string_extractor::<SessionQuery>()
            .to(end_session);
        route
            .get("/stb/poll")
            .with_query_string_extractor::<PollQuery>()
            .to(poll_session);
    })
}

fn main() {
    env_logger::init();
    let addr = "127.0.0.1:7878";
    println!("Listening for requests at http://{}", addr);
    gotham::start(addr, router());
}

#[cfg(test)]
mod tests {
    use super::*;
    use cookie::Cookie;
    use gotham::hyper::header::{COOKIE, SET_COOKIE};
    use gotham::hyper::{Body, StatusCode};
    use gotham::test::{TestResponse, TestServer};
    use hyper::header;
    use serde_json::Value;
    use std::str;

    /// Short named alias
    fn to_vec(json: Value) -> Vec<u8> {
        serde_json::to_vec(&json).expect("serialized value")
    }

    // fn get_cookies(response: &TestResponse) -> Vec<Cookie> {
    //     response
    //         .headers()
    //         .get_all(SET_COOKIE)
    //         .iter()
    //         .flat_map(|hv| hv.to_str().unwrap().parse::<Cookie>().unwrap())
    //         .collect::<Vec<_>>()
    // }

    #[test]
    fn happy() {
        // println!(
        //     "{}",
        //     serde_json::to_string(&ConfigItem {
        //         name: "cfg".to_string(),
        //         title: "ABC".to_string(),
        //         value: ConfigValue::String("qwerty".into())
        //     })
        //     .unwrap()
        // );

        let test_server = TestServer::new(router()).unwrap();
        let client = test_server.client();

        let response = client
            .post(
                "http://localhost/stb/new-session",
                // Exaple json request
                to_vec(json!([
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
                ])),
                mime::APPLICATION_JSON,
            )
            .perform()
            .unwrap();

        assert_eq!(response.status(), StatusCode::OK);

        let body = response.read_body().unwrap();
        let result: Value = serde_json::from_slice(&body).expect("valid json");
        dbg!(result.to_string());
        assert!(result["key"].is_string());
        assert!(result["secret"].is_string());

        let response = client
            .post(
                "http://localhost/",
                format!("code={}", result["key"].as_str().unwrap()),
                mime::APPLICATION_WWW_FORM_URLENCODED,
            )
            .perform()
            .unwrap();
        assert_eq!(response.status(), StatusCode::FOUND);
        assert_eq!(
            response.headers().get(header::LOCATION).unwrap(),
            "./settings"
        );

        let cookie = response.headers().get(SET_COOKIE).unwrap();

        let response = client
            .get(dbg!("http://localhost/settings"))
            .with_header(COOKIE, cookie.to_owned())
            // .with_header(COOKIE, cookies.get(0).unwrap().parse().unwrap())
            .perform()
            .unwrap();
        dbg!(&response);
        assert_eq!(response.status(), StatusCode::OK);

        let s1 = test_server.clone();
        let secret = result["secret"].as_str().unwrap().to_owned();
        let t = std::thread::spawn(move || {
            let response = s1
                .client()
                .get(format!(
                    "http://localhost/stb/poll?sid={}&revision={}",
                    &secret, 0
                ))
                .perform()
                .unwrap();
            assert_eq!(response.status(), StatusCode::OK);
            let body = response.read_body().unwrap();

            use config::{ConfigString, ConfigValue};
            use model::Values;
            let result = serde_json::from_slice::<Values>(&body).expect("valid json");
            assert_eq!(result.revision, 1);
            let a = result.values.iter().find(|v| v.name == "a").unwrap();
            assert!(
                a.value
                    == ConfigValue::String(ConfigString {
                        value: "sometext".to_owned()
                    })
            );
        });

        let response = client
            .post(
                "http://localhost/settings",
                "a=sometext",
                mime::APPLICATION_WWW_FORM_URLENCODED,
            )
            .with_header(COOKIE, cookie.to_owned())
            .perform()
            .unwrap();
        assert_eq!(response.status(), StatusCode::OK);

        t.join().unwrap();

        let response = client
            .get(format!(
                "http://localhost/stb/del-session?sid={}",
                result["secret"].as_str().unwrap()
            ))
            .perform()
            .unwrap();
        assert_eq!(response.status(), StatusCode::OK);
    }
}

// use gotham::handler::Handler;

// #[derive(Copy, Clone)]
// struct SimpleHandler<F: Copy> {
//     f: F,
// }

// impl<F: Copy> SimpleHandler<F> {
//     fn new(f: F) -> Self {
//         Self { f }
//     }
// }

// impl<R, F> Handler for SimpleHandler<F>
// where
//     F: FnOnce(&State) -> R + Send + Copy,
//     R: IntoResponse,
// {
//     fn handle(self, state: State) -> Pin<Box<HandlerFuture>> {
//         let response = (self.f)(&state).into_response(&state);
//         (state, response).into_handler_future()
//     }
// }
