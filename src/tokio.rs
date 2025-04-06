use std::{
	os::fd::RawFd,
	task::{Context, Poll},
};

use tokio::io::unix::AsyncFd;

pub type TokioAsyncFd = AsyncFd<RawFd>;
impl crate::rt::AsyncFd for TokioAsyncFd {
	fn new(fd: RawFd) -> std::io::Result<Self> {
		AsyncFd::new(fd)
	}

	fn poll_read_ready(&self, cx: &mut Context) -> Poll<std::io::Result<()>> {
		self.poll_read_ready(cx).map_ok(|mut x| x.clear_ready())
	}
}
