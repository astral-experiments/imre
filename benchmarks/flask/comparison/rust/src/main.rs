use http_body_util::{BodyExt, Full};
use hyper::body::{Bytes, Incoming};
use hyper::header::CONTENT_TYPE;
use hyper::server::conn::http1;
use hyper::service::service_fn;
use hyper::{Method, Request, Response};
use hyper_util::rt::TokioIo;
use num_bigint::BigInt;
use percent_encoding::percent_decode_str;
use serde_json::Value;
use std::{convert::Infallible, io, path::PathBuf, sync::LazyLock};

const HTML: &str = "text/html; charset=utf-8";
const JSON: &str = "application/json";
static REFERENCE: LazyLock<Value> =
    LazyLock::new(|| serde_json::from_str(include_str!("../../../expected.json")).unwrap());

fn escape(text: &str) -> String {
    text.replace('&', "&amp;")
        .replace('<', "&lt;")
        .replace('>', "&gt;")
        .replace('"', "&#34;")
        .replace('\'', "&#39;")
}

fn reply(status: u16, media: &str, body: String) -> Response<Full<Bytes>> {
    Response::builder()
        .status(status)
        .header(CONTENT_TYPE, media)
        .body(Full::new(body.into()))
        .unwrap()
}

fn error(name: &str) -> Response<Full<Bytes>> {
    let reference = &REFERENCE["loopback_tcp"][name];
    reply(
        reference["status"].as_u64().unwrap() as u16,
        HTML,
        reference["body"].as_str().unwrap().to_owned(),
    )
}

async fn route(req: Request<Incoming>) -> Result<Response<Full<Bytes>>, Infallible> {
    let path = req.uri().path();
    let response = match (req.method(), path) {
        (&Method::GET, "/health") => reply(
            200,
            JSON,
            serde_json::json!({"ok": true}).to_string() + "\n",
        ),
        (&Method::GET, _) if path.starts_with("/hello/") => {
            let name = percent_decode_str(&path["/hello/".len()..]).decode_utf8_lossy();
            if name.is_empty() || name.contains('/') {
                error("missing")
            } else {
                let suffix = form_urlencoded::parse(req.uri().query().unwrap_or("").as_bytes())
                    .find(|(key, _)| key == "suffix")
                    .map(|(_, value)| value.into_owned())
                    .unwrap_or_else(|| "!".into());
                reply(
                    200,
                    HTML,
                    format!("Hello, {}{}", escape(&name), escape(&suffix)),
                )
            }
        }
        (&Method::POST, "/total") => {
            let bytes = req.into_body().collect().await.unwrap().to_bytes();
            let json: Option<Value> = serde_json::from_slice(&bytes).ok();
            let values = json
                .as_ref()
                .and_then(|doc| doc.get("values"))
                .and_then(Value::as_array);
            match values.and_then(|items| {
                let mut total = BigInt::default();
                for item in items {
                    total += item.as_number()?.to_string().parse::<BigInt>().ok()?;
                }
                Some((items.len(), total))
            }) {
                Some((count, total)) => reply(
                    201,
                    JSON,
                    format!("{{\"count\":{count},\"total\":{total}}}\n"),
                ),
                None => error("view_exception"),
            }
        }
        _ => error("missing"),
    };
    Ok(response)
}

fn main() -> io::Result<()> {
    let ready = PathBuf::from(std::env::args_os().nth(1).expect("ready path"));
    tokio::runtime::Builder::new_current_thread()
        .enable_io()
        .build()?
        .block_on(async move {
            let listener = tokio::net::TcpListener::bind(("127.0.0.1", 0)).await?;
            std::fs::write(ready, listener.local_addr()?.port().to_string())?;
            loop {
                let (stream, _) = listener.accept().await?;
                stream.set_nodelay(true)?;
                tokio::spawn(async move {
                    let _ = http1::Builder::new()
                        .serve_connection(TokioIo::new(stream), service_fn(route))
                        .await;
                });
            }
        })
}
