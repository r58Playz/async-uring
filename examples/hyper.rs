use std::{convert::Infallible, env::args};

use async_uring::{Result, rt::UringRuntime, tokio::TokioAsyncFd};
use bytes::Bytes;
use http_body_util::Full;
use hyper::{Request, Response, server::conn::http1, service::service_fn};
use hyper_util::rt::{TokioIo, TokioTimer};
use tokio::{net::TcpListener, task::coop::unconstrained};

#[tokio::main(flavor = "current_thread")]
async fn main() -> Result<()> {
	let (rt, fut) = UringRuntime::builder::<TokioAsyncFd>().build()?;

	tokio::spawn(unconstrained(fut));

	let listener = TcpListener::bind(args().nth(1).unwrap()).await?;

	println!("listening");

	while let Ok((stream, addr)) = listener.accept().await {
		let stream = rt.register_tcp(stream.into_std()?).await?;
		println!("accepted {addr:?}");
		let stream = TokioIo::new(stream);
		tokio::spawn(async move {
			let ret = http1::Builder::new()
				.timer(TokioTimer::new())
				.serve_connection(stream, service_fn(handle))
				.await;
			println!("served {addr:?} {ret:?}");
		});
	}

	Ok(())
}

async fn handle(
	_: Request<impl hyper::body::Body>,
) -> std::result::Result<Response<Full<Bytes>>, Infallible> {
	Ok(Response::new(Full::new(Bytes::from("Hello World!"))))
}
