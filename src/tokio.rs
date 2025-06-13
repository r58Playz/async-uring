use std::{
	os::fd::RawFd,
	task::{Context, Poll, ready},
};

use tokio::io::unix::AsyncFd;

pub type TokioAsyncFd = AsyncFd<RawFd>;
impl crate::rt::AsyncFd for TokioAsyncFd {
	fn new(fd: RawFd) -> std::io::Result<Self> {
		AsyncFd::new(fd)
	}

	fn poll_read_ready<T>(
		&self,
		cx: &mut Context,
		mut callback: impl FnMut() -> std::io::Result<T>,
	) -> Poll<std::io::Result<T>> {
		let mut guard = ready!(self.poll_read_ready(cx))?;
		match guard.try_io(|_| (callback)()) {
			Ok(val) => Poll::Ready(val),
			// WouldBlock, poll so that we register for the next ready notif
			Err(_) => crate::rt::AsyncFd::poll_read_ready(self, cx, callback),
		}
	}
}
