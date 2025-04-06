use std::{os::fd::OwnedFd, sync::Arc, task::Waker};

use futures::channel::oneshot;
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

	pub(crate) async fn new(
		std: std::net::TcpStream,
		rt: UringDataHandle,
		sender: RuntimeWorkerChannel,
	) -> Result<Self> {
		std.set_nonblocking(true)?;
		let fd = OwnedFd::from(std);

		let (tx, rx) = oneshot::channel();

		let ops = Arc::new(Operations::new_from_size());

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

	pub async fn read(&mut self, buf: &mut [u8]) -> Result<usize> {
		let Some(rt) = self.rt.load() else {
			return Err(Error::NoRuntime);
		};

		let entry = opcode::Recv::new(Fixed(self.resource.id), buf.as_mut_ptr(), buf.len() as u32)
			.build()
			.user_data(
				EventData {
					resource: self.resource.id,
					id: Self::READ_OP_ID,
				}
				.into(),
			);

		let ops = self.resource.ops().await;
		let amt = unsafe { ops.submit::<{ Self::READ_OP_ID }>(rt, entry) }.await?;

		Ok(amt as usize)
	}

	pub async fn write(&mut self, buf: &[u8]) -> Result<usize> {
		let Some(rt) = self.rt.load() else {
			return Err(Error::NoRuntime);
		};

		let entry = opcode::Send::new(Fixed(self.resource.id), buf.as_ptr(), buf.len() as u32)
			.build()
			.user_data(
				EventData {
					resource: self.resource.id,
					id: Self::WRITE_OP_ID,
				}
				.into(),
			);

		let ops = self.resource.ops().await;
		let amt = unsafe { ops.submit::<{ Self::WRITE_OP_ID }>(rt, entry) }.await?;

		Ok(amt as usize)
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
