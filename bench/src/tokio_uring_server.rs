use std::{env::args, net::SocketAddr};

use tokio_uring::net::TcpListener;

fn main() {
	let socket_addr = args().nth(1).unwrap();
	let socket_addr: SocketAddr = socket_addr.parse().unwrap();

	tokio_uring::start(async {
		let listener = TcpListener::bind(socket_addr).unwrap();

		println!("Listening on {}", listener.local_addr().unwrap());

		loop {
			let (stream, socket_addr) = listener.accept().await.unwrap();
			tokio_uring::spawn(async move {
				// implement ping-pong loop

				use tokio_uring::buf::BoundedBuf; // for slice()

				println!("{socket_addr} connected");
				let mut buf = vec![0u8; 16384];
				loop {
					let (result, nbuf) = stream.read(buf).await;
					buf = nbuf;
					let read = result.unwrap();
					if read == 0 {
						break;
					}

					let (res, slice) = stream.write_all(buf.slice(..read)).await;
					res.unwrap();
					buf = slice.into_inner();
				}
			});
		}
	});
}
