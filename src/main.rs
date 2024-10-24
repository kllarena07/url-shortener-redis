use actix_files::NamedFile;
use actix_web::{get, post, web, App, HttpServer, Result, Responder, HttpResponse};
use actix_web::http::header::LOCATION;
use std::env;
use std::path::PathBuf;
use serde::Deserialize;
use dotenv::dotenv;
use std::env::var;
use fred::prelude::*;
use rand::{distributions::Alphanumeric, Rng};

struct AppState {
    redis_client: RedisClient
}

#[get("/")]
async fn index() -> Result<NamedFile> {
    let root = env::current_dir().unwrap();
    let abs_path = format!("{}/app/index.html", root.to_str().unwrap());
    let path: PathBuf = PathBuf::from(abs_path);
    Ok(NamedFile::open(path)?)
}

#[get("/{id}")]
async fn get_url(path: web::Path<String>, data: web::Data<AppState>) -> impl Responder {
    let url_id = path.into_inner();

    match data.redis_client.get::<Option<String>, String>(url_id).await {
        Ok(Some(route)) => {
            println!("HTTP 302 FOUND: Redirecting to {}", route);
            HttpResponse::Found()
                .insert_header((LOCATION, route))
                .finish()
        },
        Ok(None) => {
            println!("HTTP 404 NOT_FOUND: Redirecting to https://http.cat/404");
            HttpResponse::Found()
                .insert_header((LOCATION, "https://http.cat/404"))
                .finish()
        },
        Err(_) => {
            println!("HTTP 505 INTERNAL SERVER ERROR: Error getting url");
            HttpResponse::InternalServerError().finish()
        }
    }
}

#[derive(Deserialize)]
struct CreateRequest {
    url: String
}

fn create_shortened_url() -> String {
    let rand_string: String = rand::thread_rng()
        .sample_iter(&Alphanumeric)
        .take(5)
        .map(char::from)
        .collect();
    rand_string
}

#[post("/create")]
async fn create_url(body: web::Json<CreateRequest>, data: web::Data<AppState>) -> impl Responder { 
    let real_url = body.url.clone();
    let shortened_url = create_shortened_url();
    let expiration_seconds = 300;

    match data.redis_client.set::<(), &str, String>(&shortened_url, real_url, Some(Expiration::EX(expiration_seconds)), Some(SetOptions::NX), false).await {
        Ok(_) => {
            println!("HTTP 200 OK: Successfully created {}", &shortened_url);
            HttpResponse::Ok().body(shortened_url)
        },
        Err(_) => {
            println!("HTTP 505 INTERNAL SERVER ERROR: Error creating shortened url, {}", &shortened_url);
            HttpResponse::InternalServerError().finish()
        }
    }
}

#[shuttle_runtime::main]
async fn main() -> std::io::Result<()> {
    dotenv().ok();

    let username = var("REDIS_USERNAME").expect("REDIS_USERNAME must be set.");
    let password = var("REDIS_PASSWORD").expect("REDIS_PASSWORD must be set.");
    let host = var("REDIS_HOST").expect("REDIS_HOST must be set.");
    let port = var("REDIS_PORT").expect("REDIS_PORT must be set.");
    let redis_url = format!("redis://{}:{}@{}:{}", username, password, host, port);

    let config = RedisConfig::from_url(&redis_url).unwrap();
    let client = Builder::from_config(config).build().unwrap();

    client.init().await.map_err(|err| {
        std::io::Error::new(std::io::ErrorKind::Other, format!("Redis error: {}", err))
    })?;

    println!("Listening on 127.0.0.1:3000");

    HttpServer::new(move || {
        App::new()
            .app_data(web::Data::new(AppState { redis_client: client.clone() }))
            .service(index)
            .service(get_url)
            .service(create_url)
    })
    .bind(("127.0.0.1", 3000))?
    .run()
    .await
}
