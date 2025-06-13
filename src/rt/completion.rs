use std::{
	os::fd::AsRawFd,
	pin::Pin,
	task::{Context, Poll},
};

use futures::Stream;
use io_uring::{CompletionQueue, cqueue};

use super::{AsyncFd, UringData};

pub struct CqueueStream<'a, Fd: AsyncFd> {
	fd: Fd,
	cqueue: CompletionQueue<'a>,
}
// SAFETY: we are the only ones using the cqueue
unsafe impl<Fd: AsyncFd> Sync for CqueueStream<'_, Fd> {}
// SAFETY: we are the only ones using the cqueue
unsafe impl<Fd: AsyncFd> Send for CqueueStream<'_, Fd> {}

impl<'a, Fd: AsyncFd> CqueueStream<'a, Fd> {
	pub fn new(rt: &'a UringData) -> crate::Result<Self> {
		let fd = Fd::new(rt.uring.as_raw_fd())?;

		// SAFETY: we are the only ones using the cqueue
		let cqueue = unsafe { rt.uring.completion_shared() };

		Ok(Self { fd, cqueue })
	}
}

impl<Fd: AsyncFd> Stream for CqueueStream<'_, Fd> {
	type Item = crate::Result<cqueue::Entry>;

	fn poll_next(mut self: Pin<&mut Self>, cx: &mut Context<'_>) -> Poll<Option<Self::Item>> {
		let this = &mut *self;
		this.fd
			.poll_read_ready(cx, || {
				this.cqueue.sync();
				this.cqueue
					.next()
					.ok_or(std::io::ErrorKind::WouldBlock.into())
			})
			.map_err(Into::into)
			.map(Some)
	}
}
