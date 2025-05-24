use std::env::args;
use tokio::{
	io::{AsyncReadExt, AsyncWriteExt},
	net::TcpListener,
};

#[tokio::main]
async fn main() -> anyhow::Result<()> {
	let addr = args()
		.nth(2)
		.unwrap_or_else(|| "127.0.0.1:8080".to_string());
	let listener = TcpListener::bind(&addr).await?;
	println!("Listening on: {addr}");

	loop {
		let (mut socket, _) = listener.accept().await?;

		tokio::spawn(async move {
			let mut buf = vec![0; 16384];

			loop {
				let n = socket
					.read(&mut buf)
					.await
					.expect("failed to read data from socket");

				if n == 0 {
					return;
				}

				socket
					.write_all(&buf[0..n])
					.await
					.expect("failed to write data to socket");
			}
		});
	}
}
