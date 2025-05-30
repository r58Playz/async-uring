use std::env::args;

use monoio::{
	io::{AsyncReadRent, AsyncWriteRentExt},
	net::{TcpListener, TcpStream},
};

#[monoio::main(driver = "fusion")]
async fn main() {
	let listener = TcpListener::bind(args().nth(1).unwrap()).unwrap();
	println!("listening");
	loop {
		let incoming = listener.accept().await;
		match incoming {
			Ok((stream, addr)) => {
				println!("accepted a connection from {addr}");
				monoio::spawn(echo(stream));
			}
			Err(e) => {
				println!("accepted connection failed: {e}");
				return;
			}
		}
	}
}

async fn echo(mut stream: TcpStream) -> std::io::Result<()> {
	let mut buf: Vec<u8> = Vec::with_capacity(16 * 1024);
	let mut res;
	loop {
		// read
		(res, buf) = stream.read(buf).await;
		if res? == 0 {
			return Ok(());
		}

		// write all
		(res, buf) = stream.write_all(buf).await;
		res?;
	}
}
