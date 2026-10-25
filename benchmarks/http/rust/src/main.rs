use http_body_util::Full;
use hyper::body::{Bytes, Incoming};
use hyper::header::{CONTENT_LENGTH, CONTENT_TYPE, HeaderValue};
use hyper::server::conn::http1;
use hyper::service::service_fn;
use hyper::{Method, Request, Response, StatusCode};
use hyper_util::rt::TokioIo;
use std::{convert::Infallible, net::SocketAddr};

async fn hello(req: Request<Incoming>) -> Result<Response<Full<Bytes>>, Infallible> {
    let ok = req.method() == Method::GET && req.uri().path() == "/hello";
    let (status, body, len) = if ok {
        (StatusCode::OK, &b"hello\n"[..], "6")
    } else {
        (StatusCode::NOT_FOUND, &b"not found\n"[..], "10")
    };
    let mut resp = Response::new(Full::new(Bytes::from_static(body)));
    *resp.status_mut() = status;
    resp.headers_mut()
        .insert(CONTENT_TYPE, HeaderValue::from_static("text/plain"));
    resp.headers_mut()
        .insert(CONTENT_LENGTH, HeaderValue::from_static(len));
    Ok(resp)
}

fn main() -> std::io::Result<()> {
    let port: u16 = std::env::args()
        .nth(1)
        .expect("port")
        .parse()
        .expect("numeric port");
    tokio::runtime::Builder::new_current_thread()
        .enable_io()
        .build()?
        .block_on(async move {
            let socket = tokio::net::TcpSocket::new_v4()?;
            socket.set_reuseaddr(true)?;
            socket.bind(SocketAddr::from(([127, 0, 0, 1], port)))?;
            let listener = socket.listen(128)?;
            loop {
                let (stream, _) = listener.accept().await?;
                stream.set_nodelay(true)?;
                tokio::spawn(async move {
                    let _ = http1::Builder::new()
                        .auto_date_header(false)
                        .serve_connection(TokioIo::new(stream), service_fn(hello))
                        .await;
                });
            }
        })
}
