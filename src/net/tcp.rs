use std::{
	os::fd::{AsRawFd, OwnedFd, RawFd},
	pin::Pin,
	task::{Context, Poll},
};

use futures::{channel::oneshot, ready};
use io_uring::{opcode, types::Fd};
use tokio::io::{AsyncRead, AsyncWrite};

use crate::{
	Error, Result,
	rt::{
		UringDataHandle,
		inner::{RuntimeWorkerChannel, WorkerMessage},
		operation::{Operations, ProtectedOps, poll_op_impl},
		resource::Resource,
	},
};

const READ_OP_ID: u32 = 0;
const WRITE_OP_ID: u32 = 1;
const CLOSE_OP_ID: u32 = 2;

macro_rules! poll_read {
    ($self:ident, $cx:ident, $buf:ident) => {
		let this = &mut *$self;
		return poll_op_impl!(READ_OP_ID, this, $cx, false, {
			Some(Ok(val)) => |val| {
				// SAFETY: kernel just initialized these bytes in the read op
				unsafe { $buf.assume_init(val as usize) };
				$buf.advance(val as usize);
				Poll::Ready(Ok(()))
			},
			None => || {
				// SAFETY: we send it straight to the kernel and it doesn't de-initialize anything
				let uninit = unsafe { $buf.unfilled_mut() };
				Ok(opcode::Recv::new(
					Fd(this.fd),
					uninit.as_mut_ptr().cast::<u8>(),
					uninit.len().try_into().map_err(|_| Error::BufferTooLarge)?,
				))
			}
		})
		.map_err(std::io::Error::other);
    };
}

macro_rules! poll_write {
    ($self:ident, $cx:ident, $buf:ident) => {
		let this = &mut *$self;
		return poll_op_impl!(WRITE_OP_ID, this, $cx, false, {
			Some(Ok(val)) => |val| Poll::Ready(Ok(val as usize)),
			None => || Ok(opcode::Send::new(Fd(this.fd), $buf.as_ptr(), $buf.len().try_into().map_err(|_| Error::BufferTooLarge)?))
		})
		.map_err(std::io::Error::other);
    };
}

macro_rules! poll_shutdown {
    ($self:ident, $cx: ident) => {
		$self.resource.set_closing();
		let this = &mut *$self;
		return poll_op_impl!(CLOSE_OP_ID, this, $cx, true, {
			Some(Ok(val)) => |_| Poll::Ready(Ok(())),
			None => || Ok(opcode::Close::new(Fd(this.fd)))
		})
		.map_err(std::io::Error::other)
    };
}

pub struct ReadHalf {
	rt: UringDataHandle,
	resource: Resource,
	sender: RuntimeWorkerChannel,

	fd: RawFd,
}
pub struct WriteHalf {
	rt: UringDataHandle,
	resource: Resource,
	sender: RuntimeWorkerChannel,

	fd: RawFd,
}
pub struct TcpStream {
	rt: UringDataHandle,
	resource: Resource,
	sender: RuntimeWorkerChannel,

	fd: RawFd,

	destructuring: bool,
}

impl ProtectedOps for ReadHalf {
	const READ_OP_ID: u32 = READ_OP_ID;
	const WRITE_OP_ID: u32 = WRITE_OP_ID;

	fn get_resource(&mut self) -> &mut Resource {
		&mut self.resource
	}
}
impl ProtectedOps for WriteHalf {
	const READ_OP_ID: u32 = READ_OP_ID;
	const WRITE_OP_ID: u32 = WRITE_OP_ID;

	fn get_resource(&mut self) -> &mut Resource {
		&mut self.resource
	}
}
impl ProtectedOps for TcpStream {
	const READ_OP_ID: u32 = READ_OP_ID;
	const WRITE_OP_ID: u32 = WRITE_OP_ID;

	fn get_resource(&mut self) -> &mut Resource {
		&mut self.resource
	}
}

impl TcpStream {
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

			destructuring: false,
		})
	}

	pub fn into_split(mut self) -> (ReadHalf, WriteHalf) {
		self.destructuring = true;
		(
			ReadHalf {
				resource: self.resource.clone(),
				rt: self.rt.clone(),
				sender: self.sender.clone(),
				fd: self.fd,
			},
			WriteHalf {
				resource: self.resource.clone(),
				rt: self.rt.clone(),
				sender: self.sender.clone(),
				fd: self.fd,
			},
		)
	}
}

impl AsyncRead for TcpStream {
	fn poll_read(
		mut self: Pin<&mut Self>,
		cx: &mut Context<'_>,
		buf: &mut tokio::io::ReadBuf<'_>,
	) -> Poll<std::io::Result<()>> {
		poll_read!(self, cx, buf);
	}
}
impl AsyncRead for ReadHalf {
	fn poll_read(
		mut self: Pin<&mut Self>,
		cx: &mut Context<'_>,
		buf: &mut tokio::io::ReadBuf<'_>,
	) -> Poll<std::io::Result<()>> {
		poll_read!(self, cx, buf);
	}
}

impl AsyncWrite for TcpStream {
	fn poll_write(
		mut self: Pin<&mut Self>,
		cx: &mut Context<'_>,
		buf: &[u8],
	) -> Poll<std::io::Result<usize>> {
		poll_write!(self, cx, buf);
	}

	fn poll_flush(self: Pin<&mut Self>, _: &mut Context<'_>) -> Poll<std::io::Result<()>> {
		// flush is noop
		Poll::Ready(Ok(()))
	}

	fn poll_shutdown(mut self: Pin<&mut Self>, cx: &mut Context<'_>) -> Poll<std::io::Result<()>> {
		poll_shutdown!(self, cx);
	}
}
impl AsyncWrite for WriteHalf {
	fn poll_write(
		mut self: Pin<&mut Self>,
		cx: &mut Context<'_>,
		buf: &[u8],
	) -> Poll<std::io::Result<usize>> {
		poll_write!(self, cx, buf);
	}

	fn poll_flush(self: Pin<&mut Self>, _: &mut Context<'_>) -> Poll<std::io::Result<()>> {
		// flush is noop
		Poll::Ready(Ok(()))
	}

	fn poll_shutdown(mut self: Pin<&mut Self>, cx: &mut Context<'_>) -> Poll<std::io::Result<()>> {
		poll_shutdown!(self, cx);
	}
}

impl Drop for ReadHalf {
	fn drop(&mut self) {
		let _ = self
			.sender
			.send(WorkerMessage::CloseResource(self.resource.dup()));
	}
}
impl Drop for WriteHalf {
	fn drop(&mut self) {
		let _ = self
			.sender
			.send(WorkerMessage::CloseResource(self.resource.dup()));
	}
}
impl Drop for TcpStream {
	fn drop(&mut self) {
		if !self.destructuring {
			let _ = self
				.sender
				.send(WorkerMessage::CloseResource(self.resource.dup()));
		}
	}
}
