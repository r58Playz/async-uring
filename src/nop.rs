use std::{
	pin::Pin,
	task::{Context, Poll, ready},
};

use futures::{Stream, channel::oneshot};
use io_uring::opcode;

use crate::{
	Error, Result,
	rt::{
		UringDataHandle,
		inner::{RuntimeWorkerChannel, WorkerMessage},
		operation::{Operations, poll_op_impl},
		resource::Resource,
	},
};

pub struct NopStream {
	rt: UringDataHandle,
	resource: Resource,
	sender: RuntimeWorkerChannel,
}

impl NopStream {
	const NOP_OP_ID: u32 = 0;

	pub(crate) async fn new(rt: UringDataHandle, sender: RuntimeWorkerChannel) -> Result<Self> {
		let (tx, rx) = oneshot::channel();

		let ops = Operations::new_from_size();

		sender.send(WorkerMessage::RegisterResource {
			ops,
			fd: None,
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

impl Stream for NopStream {
	type Item = Result<u32>;

	fn poll_next(mut self: Pin<&mut Self>, cx: &mut Context<'_>) -> Poll<Option<Self::Item>> {
		let this = &mut *self;
		poll_op_impl!(Self::NOP_OP_ID, this, cx, false, {
			Some(Ok(val)) => |val| Poll::Ready(Ok(val)),
			None => || Ok(opcode::Nop::new())
		})
		.map(Some)
	}
}

impl Drop for NopStream {
	fn drop(&mut self) {
		let _ = self
			.sender
			.send(WorkerMessage::CloseResource(self.resource.dup()));
	}
}
