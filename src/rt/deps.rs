use std::{
	io::Result,
	os::fd::RawFd,
	task::{Context, Poll},
};

pub trait AsyncFd: Sync + Send + Sized + Unpin {
	fn new(fd: RawFd) -> Result<Self>;

	fn poll_read_ready<T>(
		&self,
		cx: &mut Context,
		callback: impl FnMut() -> Result<T>,
	) -> Poll<Result<T>>;
}
