use std::{
	env::args,
	net::SocketAddr,
	str::FromStr,
	sync::atomic::{AtomicUsize, Ordering},
	time::{Duration, Instant},
};

use async_uring::{Result, net::tcp::TcpStream, rt::UringRuntime, tokio::TokioAsyncFd};
use futures::{AsyncReadExt, AsyncWriteExt};
use tokio::task::{JoinSet, coop::unconstrained};

static COUNT: AtomicUsize = AtomicUsize::new(0);

#[tokio::main(flavor = "current_thread")]
async fn main() -> Result<()> {
	let (rt, fut) = UringRuntime::builder::<TokioAsyncFd>().build()?;

	tokio::spawn(unconstrained(fut));

	let addr = SocketAddr::from_str(&args().nth(1).unwrap()).unwrap();
	let cnt = usize::from_str(&args().nth(2).unwrap()).unwrap();
	let size = args()
		.nth(3)
		.map_or(16 * 1024, |x| usize::from_str(&x).unwrap());

	let mut sockets = Vec::with_capacity(cnt);
	for _ in 0..cnt {
		sockets.push(
			rt.register_tcp(tokio::net::TcpStream::connect(addr).await?.into_std()?)
				.await?,
		);
	}

	let mut set = JoinSet::new();

	println!("Starting with {size} byte packets and {cnt} sockets");

	set.spawn(async move {
		let mut last = 0;
		let mut last_time = Instant::now();
		let mut interval = tokio::time::interval(Duration::from_secs(5));

		interval.tick().await;

		loop {
			let time = interval.tick().await.into_std();

			let val = COUNT.load(Ordering::Relaxed);
			let cnt = val - last;
			last = val;

			let duration = time - last_time;
			last_time = time;

			#[allow(clippy::cast_precision_loss)]
			let amt = cnt as f64 / duration.as_secs_f64();
			println!("Req/sec: {amt:.2} ({cnt} / {duration:?})");
		}
	});

	for socket in sockets {
		set.spawn(handle(socket, size));
	}

	set.join_all().await;

	Ok(())
}

async fn handle(mut stream: TcpStream, size: usize) -> Result<()> {
	let mut buf = vec![0u8; size];

	let mut received = 0;

	loop {
		stream.write_all(&buf).await?;

		let cnt = stream.read(&mut buf).await?;

		received += cnt;

		if cnt == 0 {
			break Ok(());
		}

		received %= size;
		if received == 0 {
			COUNT.fetch_add(1, Ordering::Relaxed);
		}
	}
}
