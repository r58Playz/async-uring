use std::{
	os::fd::{AsRawFd, OwnedFd, RawFd},
	pin::Pin,
	task::{Context, Poll, Waker},
};

use futures::{channel::oneshot, ready};
use io_uring::{opcode, types::Fd};
use tokio::io::{AsyncRead, AsyncWrite};

use crate::{
	Error, Result,
	rt::{
		UringDataHandle,
		inner::{RuntimeWorkerChannel, WorkerMessage},
		operation::{Operations, poll_op_impl},
		resource::Resource,
	},
};

pub struct TcpStream {
	rt: UringDataHandle,
	resource: Resource,
	sender: RuntimeWorkerChannel,

	fd: RawFd,

	closing: bool,
}

impl TcpStream {
	const READ_OP_ID: u32 = 0;
	const WRITE_OP_ID: u32 = 1;
	const CLOSE_OP_ID: u32 = 2;

	pub(crate) async fn new(
		std: std::net::TcpStream,
		rt: UringDataHandle,
		sender: RuntimeWorkerChannel,
	) -> Result<Self> {
		std.set_nonblocking(true)?;
		let fd = OwnedFd::from(std);
		let raw = fd.as_raw_fd();

		let (tx, rx) = oneshot::channel();

		let ops = Operations::new_from_size();

		sender.send(WorkerMessage::RegisterResource {
			ops,
			fd: Some(fd),
			complete: tx,
		})?;

		let resource = rx.await.map_err(|_| Error::NoRuntime)??;

		Ok(Self {
			rt,
			resource,
			sender,
			fd: raw,

			closing: false,
		})
	}
}

impl AsyncRead for TcpStream {
	fn poll_read(
		mut self: Pin<&mut Self>,
		cx: &mut Context<'_>,
		buf: &mut tokio::io::ReadBuf<'_>,
	) -> Poll<std::io::Result<()>> {
		let this = &mut *self;
		poll_op_impl!(Self::READ_OP_ID, this, cx, {
			Some(Ok(val)) => |val| {
				// SAFETY: kernel just initialized these bytes in the read op
				unsafe { buf.assume_init(val as usize) };
				buf.advance(val as usize);
				Poll::Ready(Ok(()))
			},
			None => || {
				// SAFETY: we send it straight to the kernel and it doesn't de-initialize anything
				let uninit = unsafe { buf.unfilled_mut() };
				Ok(opcode::Recv::new(
					Fd(this.fd),
					uninit.as_mut_ptr().cast::<u8>(),
					uninit.len().try_into().map_err(|_| Error::BufferTooLarge)?,
				))
			}
		})
		.map_err(std::io::Error::other)
	}
}

impl AsyncWrite for TcpStream {
	fn poll_write(
		mut self: Pin<&mut Self>,
		cx: &mut Context<'_>,
		buf: &[u8],
	) -> Poll<std::io::Result<usize>> {
		let this = &mut *self;
		poll_op_impl!(Self::WRITE_OP_ID, this, cx, {
			Some(Ok(val)) => |val| Poll::Ready(Ok(val as usize)),
			None => || Ok(opcode::Send::new(Fd(this.fd), buf.as_ptr(), buf.len().try_into().map_err(|_| Error::BufferTooLarge)?))
		})
		.map_err(std::io::Error::other)
	}

	fn poll_flush(self: Pin<&mut Self>, _: &mut Context<'_>) -> Poll<std::io::Result<()>> {
		// flush is noop
		Poll::Ready(Ok(()))
	}

	fn poll_shutdown(mut self: Pin<&mut Self>, cx: &mut Context<'_>) -> Poll<std::io::Result<()>> {
		self.closing = true;
		let this = &mut *self;
		poll_op_impl!(Self::CLOSE_OP_ID, this, cx, {
			Some(Ok(val)) => |_| Poll::Ready(Ok(())),
			None => || Ok(opcode::Close::new(Fd(this.fd)))
		})
		.map_err(std::io::Error::other)
	}
}

impl Drop for TcpStream {
	fn drop(&mut self) {
		let _ = self.sender.send(WorkerMessage::CloseResource {
			id: self.resource.id,
			complete: Waker::noop().clone(),
		});
	}
}
