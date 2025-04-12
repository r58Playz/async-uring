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
		operation::{EventData, Operations},
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
		let Some(rt) = this.rt.load() else {
			return Poll::Ready(Err(std::io::Error::other(Error::NoRuntime)));
		};

		let id = this.resource.id;
		match ready!(this.resource.ops.poll_submit::<{ Self::READ_OP_ID }>(cx)) {
			Some(Ok(val)) => {
				unsafe { buf.assume_init(val as usize) };
				buf.advance(val as usize);
				Poll::Ready(Ok(()))
			}
			Some(Err(err)) => Poll::Ready(Err(std::io::Error::other(err))),
			None => {
				if this.closing {
					Poll::Ready(Err(std::io::Error::other(Error::ResourceClosing)))
				} else {
					let uninit = unsafe { buf.unfilled_mut() };
					let entry = opcode::Recv::new(
						Fd(this.fd),
						uninit.as_mut_ptr().cast::<u8>(),
						uninit.len() as u32,
					)
					.build()
					.user_data(
						EventData {
							resource: id,
							id: Self::READ_OP_ID,
						}
						.into(),
					);

					if let Err(err) = unsafe {
						this.resource
							.ops
							.start_submit::<{ Self::READ_OP_ID }>(rt, &entry, cx)
					} {
						Poll::Ready(Err(std::io::Error::other(err)))
					} else {
						Poll::Pending
					}
				}
			}
		}
	}
}

impl AsyncWrite for TcpStream {
	fn poll_write(
		mut self: Pin<&mut Self>,
		cx: &mut Context<'_>,
		buf: &[u8],
	) -> Poll<std::io::Result<usize>> {
		let this = &mut *self;
		let Some(rt) = this.rt.load() else {
			return Poll::Ready(Err(std::io::Error::other(Error::NoRuntime)));
		};

		let id = this.resource.id;
		match ready!(this.resource.ops.poll_submit::<{ Self::WRITE_OP_ID }>(cx)) {
			Some(Ok(val)) => Poll::Ready(Ok(val as usize)),
			Some(Err(err)) => Poll::Ready(Err(std::io::Error::other(err))),
			None => {
				if this.closing {
					Poll::Ready(Err(std::io::Error::other(Error::ResourceClosing)))
				} else {
					let entry = opcode::Send::new(Fd(this.fd), buf.as_ptr(), buf.len() as u32)
						.build()
						.user_data(
							EventData {
								resource: id,
								id: Self::WRITE_OP_ID,
							}
							.into(),
						);

					if let Err(err) = unsafe {
						this.resource
							.ops
							.start_submit::<{ Self::WRITE_OP_ID }>(rt, &entry, cx)
					} {
						Poll::Ready(Err(std::io::Error::other(err)))
					} else {
						Poll::Pending
					}
				}
			}
		}
	}

	fn poll_flush(self: Pin<&mut Self>, _: &mut Context<'_>) -> Poll<std::io::Result<()>> {
		// flush is noop
		Poll::Ready(Ok(()))
	}

	fn poll_shutdown(mut self: Pin<&mut Self>, cx: &mut Context<'_>) -> Poll<std::io::Result<()>> {
		self.closing = true;
		let this = &mut *self;
		let Some(rt) = this.rt.load() else {
			return Poll::Ready(Err(std::io::Error::other(Error::NoRuntime)));
		};

		let id = this.resource.id;
		match ready!(this.resource.ops.poll_submit::<{ Self::CLOSE_OP_ID }>(cx)) {
			Some(Ok(_)) => Poll::Ready(Ok(())),
			Some(Err(err)) => Poll::Ready(Err(std::io::Error::other(err))),
			None => {
				let entry = opcode::Close::new(Fd(this.fd)).build().user_data(
					EventData {
						resource: id,
						id: Self::CLOSE_OP_ID,
					}
					.into(),
				);

				if let Err(err) = unsafe {
					this.resource
						.ops
						.start_submit::<{ Self::CLOSE_OP_ID }>(rt, &entry, cx)
				} {
					Poll::Ready(Err(std::io::Error::other(err)))
				} else {
					Poll::Pending
				}
			}
		}
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
