use std::env::args;

use async_uring::{Result, net::tcp::TcpStream, rt::UringRuntime, tokio::TokioAsyncFd};
use futures::{AsyncReadExt, AsyncWriteExt};
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
		tokio::spawn(handle(stream));
	}

	Ok(())
}

async fn handle(mut stream: TcpStream) -> Result<()> {
	let mut buf = vec![0u8; 16 * 1024];

	loop {
		let cnt = stream.read(&mut buf).await?;

		if cnt == 0 {
			break Ok(());
		}

		stream.write_all(&buf[0..cnt]).await?;
	}
}
