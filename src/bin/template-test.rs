use actix_web::{error, middleware, web, App, HttpRequest, HttpResponse, HttpServer, Responder};
use serde::Serialize;
use std::collections::HashMap;
use std::fmt::Write;
use tera::Context;
use web_settings::views::{ErrorPage, IndexPage, Page, SettingsPage, SubmittedPage, TERA};

type PagesData = HashMap<&'static str, Box<Context>>;

#[actix_rt::main]
async fn main() -> std::io::Result<()> {
    env_logger::init();

    // List of mocked pages for testing html/css code
    let pages: Vec<(&'static str, Box<Context>)> = vec![
        box_page::<IndexPage>(),
        box_page::<SettingsPage>(),
        box_page::<SubmittedPage>(),
        box_page::<ErrorPage>(),
    ];

    println!(
        "Known tera templates: {:?}",
        TERA.templates.keys().collect::<Vec<_>>()
    );
    println!(
        "Mocked pages: {:?}",
        TERA.templates.keys().collect::<Vec<_>>()
    );

    let pages_hash: PagesData = pages.into_iter().collect();
    let addr = format!("127.0.0.1:8000");
    println!("Starting web server at {}", addr);

    HttpServer::new(move || {
        App::new()
            .wrap(middleware::Logger::default())
            .data(pages_hash.clone())
            .configure(|cfg| {
                cfg.route("/", web::get().to(list_pages));
                for (&page, _) in pages_hash.iter() {
                    cfg.route(page, web::get().to(get_page));
                }
            })
    })
    .bind(addr)?
    .run()
    .await
}

async fn list_pages(pages: web::Data<PagesData>) -> impl Responder {
    let mut s = String::new();
    s.push_str("<body><ul>");
    for &p in pages.keys() {
        write!(&mut s, r#"<li><a href="./{0}">{0}</a><br></li>"#, p).unwrap();
    }
    s.push_str("</ul></body>");
    HttpResponse::Ok()
        .content_type(mime::TEXT_HTML.as_ref())
        .body(s)
}

async fn get_page(req: HttpRequest, pages: web::Data<PagesData>) -> impl Responder {
    let template = &req.path()[1..];
    let context = pages.get(template).unwrap();
    TERA.render(template, &context)
        .map(|b| {
            HttpResponse::Ok()
                .content_type(mime::TEXT_HTML.as_ref())
                .body(b)
        })
        .map_err(error::ErrorInternalServerError)
}

fn box_page<T: Page + Serialize>() -> (&'static str, Box<Context>) {
    (
        T::TEMPLATE_NAME,
        Box::new(Context::from_serialize(T::mock()).unwrap()),
    )
}
