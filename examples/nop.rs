use std::{env::args, str::FromStr, time::Instant};

use async_uring::{Result, rt::UringRuntime, tokio::TokioAsyncFd};
use futures::StreamExt;
use tokio::{runtime::Builder, task::coop::unconstrained};

fn main() -> Result<()> {
	let mut builder = Builder::new_current_thread();
	builder.enable_io();
	let rt = builder.build()?;
	rt.block_on(async move {
		let (rt, fut) = UringRuntime::builder::<TokioAsyncFd>().build()?;
		tokio::spawn(unconstrained(fut));

		tokio::spawn(unconstrained(async move {
			let milestone = usize::from_str(&args().nth(1).unwrap()).unwrap();

			let mut nopper = rt.nop_stream().await?;

			let mut cnt = 0;
			let mut last_milestone = Instant::now();
			while let Some(Ok(_)) = nopper.next().await {
				cnt += 1;

				if cnt % milestone == 0 {
					let now = Instant::now();
					let elapsed = now - last_milestone;
					last_milestone = now;

					println!("{milestone} nops in {elapsed:?}");
				}
			}

			Ok(())
		}))
		.await
		.unwrap()
	})
}
