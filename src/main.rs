#![allow(clippy::disallowed_names)]
#![allow(clippy::let_underscore_future)]

use dotenv::dotenv;
use fred::prelude::*;
use std::env::var;

use std::convert::Infallible;
use std::net::SocketAddr;

use http_body_util::{BodyExt, Full};
use hyper::body::Bytes;
use hyper::server::conn::http1;
use hyper::service::service_fn;
use hyper::{Request, Response};
use hyper_util::rt::TokioIo;
use tokio::net::TcpListener;

use serde::{Deserialize, Serialize};
use serde_json;

use rand::{distributions::Alphanumeric, Rng};

#[derive(Serialize, Deserialize, Debug)]
struct UrlData {
    real_url: String,
}

fn create_url_id() -> String {
    let rand_string: String = rand::thread_rng()
        .sample_iter(&Alphanumeric)
        .take(5)
        .map(char::from)
        .collect();

    rand_string
}

fn get_redis_client() -> Result<RedisClient, RedisError> {
    let username = var("REDIS_USERNAME").expect("REDIS_USERNAME must be set.");
    let password = var("REDIS_PASSWORD").expect("REDIS_PASSWORD must be set.");
    let host = var("REDIS_HOST").expect("REDIS_HOST must be set.");
    let port = var("REDIS_PORT").expect("REDIS_PORT must be set.");

    let redis_url = format!("redis://{}:{}@{}:{}", username, password, host, port);

    let config = RedisConfig::from_url(&redis_url)?;

    let client = Builder::from_config(config).build()?;

    Ok(client)
}

fn create_res(response: String) -> Response<Full<Bytes>> {
    Response::new(Full::new(Bytes::from(response)))
}

async fn process(req: Request<hyper::body::Incoming>) -> Result<Response<Full<Bytes>>, Infallible> {
    println!("Incoming request: {:?}", req);

    let client = match get_redis_client() {
        Ok(client) => client,
        Err(err) => {
            eprintln!("{}", err);
            return Ok(create_res(String::from("Internal Server Error 500.")));
        }
    };

    match client.init().await {
        Ok(_) => {}
        Err(err) => {
            eprintln!("{}", err);
            return Ok(create_res(String::from("Internal Server Error 500.")));
        }
    };

    let method = req.method();
    let uri_path = req.uri().path();

    let response: String = match method {
        &hyper::Method::GET => {
            let shortened_url = &uri_path[1..];

            println!("Obtaining real URL from {}.", shortened_url);

            match client.get::<Option<String>, &str>(shortened_url).await {
                Ok(Some(url)) => url,
                Ok(None) => String::from("Shortened URL not found."),
                Err(_) => String::from("Failed to obtain real URL."),
            }
        }
        &hyper::Method::POST => match uri_path {
            "/" => {
                let body_bytes = match req.collect().await {
                    Ok(data) => data.to_bytes(),
                    Err(_) => return Ok(create_res(String::from("Internal Server Error 500."))),
                };

                let body_string =
                    String::from_utf8(body_bytes.to_vec()).expect("Internal Server Error 500.");

                let real_url: String = match serde_json::from_str::<UrlData>(&body_string) {
                    Ok(data) => data.real_url,
                    Err(_) => return Ok(create_res(String::from("Invalid JSON data."))),
                };

                let url_id = create_url_id();
                println!("Creating shortened URL from {}.", real_url);

                match client
                    .set::<(), &str, String>(
                        &url_id,
                        real_url,
                        Some(Expiration::KEEPTTL),
                        Some(SetOptions::NX),
                        false,
                    )
                    .await
                {
                    Ok(_) => format!("Successfully created shortened URL. ID is {}", url_id),
                    Err(_) => String::from("Failed to create shortened URL."),
                }
            }
            _ => String::from("Endpoint method access doesn't exist."),
        },
        _ => String::from("Method not implemented."),
    };

    match client.quit().await {
        Ok(_) => {}
        Err(err) => {
            eprintln!("{}", err);
            return Ok(create_res(String::from("Internal Server Error 500.")));
        }
    };

    Ok(create_res(response))
}

async fn shutdown_signal() {
    tokio::signal::ctrl_c()
        .await
        .expect("failed to install CTRL+C signal handler");
}

#[tokio::main]
async fn main() -> Result<(), Box<dyn std::error::Error + Send + Sync>> {
    dotenv().ok();

    let addr = SocketAddr::from(([127, 0, 0, 1], 3000));
    let listener = TcpListener::bind(addr).await?;
    println!("Listening on 127.0.0.1:3000");

    let graceful = hyper_util::server::graceful::GracefulShutdown::new();
    let mut signal = std::pin::pin!(shutdown_signal());
    let http = http1::Builder::new();

    loop {
        tokio::select! {
            Ok((stream, _addr)) = listener.accept() => {
                let io = TokioIo::new(stream);
                let conn = http.serve_connection(io, service_fn(process));
                let fut = graceful.watch(conn);
                tokio::spawn(async move {
                    if let Err(e) = fut.await {
                        eprintln!("Error serving connection: {:?}", e);
                    }
                });
            },

            _ = &mut signal => {
                eprintln!("graceful shutdown signal received");
                break;
            }
        }
    }

    tokio::select! {
        _ = graceful.shutdown() => {
            eprintln!("all connections gracefully closed");
        },
        _ = tokio::time::sleep(std::time::Duration::from_secs(10)) => {
            eprintln!("timed out wait for all connections to close");
        }
    }

    Ok(())
}
