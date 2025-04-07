use std::{
	os::fd::OwnedFd,
	pin::Pin,
	task::{Context, Poll, Waker},
};

use futures::{AsyncRead, AsyncWrite, channel::oneshot, ready};
use io_uring::{opcode, types::Fixed};

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

		let (tx, rx) = oneshot::channel();

		let ops = Operations::new_from_size();

		sender.send(WorkerMessage::RegisterResource {
			ops,
			fd,
			complete: tx,
		})?;

		let resource = rx.await.map_err(|_| Error::NoRuntime)??;

		Ok(Self {
			rt,
			resource,
			sender,
		})
	}
}

impl AsyncRead for TcpStream {
	fn poll_read(
		mut self: Pin<&mut Self>,
		cx: &mut Context<'_>,
		buf: &mut [u8],
	) -> Poll<std::io::Result<usize>> {
		let this = &mut *self;
		let Some(rt) = this.rt.load() else {
			return Poll::Ready(Err(std::io::Error::other(Error::NoRuntime)));
		};

		let id = this.resource.id;
		let ops = ready!(this.resource.poll_ops(cx));
		let ret = match ready!(ops.poll_submit::<{ Self::READ_OP_ID }>(cx)) {
			Some(Ok(val)) => Poll::Ready(Ok(val as usize)),
			Some(Err(err)) => Poll::Ready(Err(std::io::Error::other(err))),
			None => {
				let entry = opcode::Recv::new(Fixed(id), buf.as_mut_ptr(), buf.len() as u32)
					.build()
					.user_data(
						EventData {
							resource: id,
							id: Self::READ_OP_ID,
						}
						.into(),
					);

				if let Err(err) = unsafe { ops.start_submit::<{ Self::READ_OP_ID }>(rt, &entry, cx) }
				{
					Poll::Ready(Err(std::io::Error::other(err)))
				} else {
					Poll::Pending
				}
			}
		};

		this.resource.disarm();
		ret
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
		let ops = ready!(this.resource.poll_ops(cx));
		let ret = match ready!(ops.poll_submit::<{ Self::WRITE_OP_ID }>(cx)) {
			Some(Ok(val)) => Poll::Ready(Ok(val as usize)),
			Some(Err(err)) => Poll::Ready(Err(std::io::Error::other(err))),
			None => {
				let entry = opcode::Send::new(Fixed(id), buf.as_ptr(), buf.len() as u32)
					.build()
					.user_data(
						EventData {
							resource: id,
							id: Self::WRITE_OP_ID,
						}
						.into(),
					);

				if let Err(err) =
					unsafe { ops.start_submit::<{ Self::WRITE_OP_ID }>(rt, &entry, cx) }
				{
					Poll::Ready(Err(std::io::Error::other(err)))
				} else {
					Poll::Pending
				}
			}
		};

		this.resource.disarm();
		ret
	}

	fn poll_flush(self: Pin<&mut Self>, _: &mut Context<'_>) -> Poll<std::io::Result<()>> {
		// flush is noop
		Poll::Ready(Ok(()))
	}

	fn poll_close(self: Pin<&mut Self>, cx: &mut Context<'_>) -> Poll<std::io::Result<()>> {
		todo!()
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
