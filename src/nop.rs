use std::{
	pin::Pin,
	task::{Context, Poll, Waker, ready},
};

use futures::{Stream, channel::oneshot};
use io_uring::opcode;

use crate::{
	Error, Result,
	rt::{
		UringDataHandle,
		inner::{RuntimeWorkerChannel, WorkerMessage},
		operation::{EventData, Operations},
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
		let Some(rt) = this.rt.load() else {
			return Poll::Ready(Some(Err(Error::NoRuntime)));
		};

		let id = this.resource.id;
		match ready!(this.resource.ops.poll_submit::<{ Self::NOP_OP_ID }>(cx)) {
			Some(Ok(val)) => Poll::Ready(Some(Ok(val))),
			Some(Err(err)) => Poll::Ready(Some(Err(err))),
			None => {
				let entry = opcode::Nop::new().build().user_data(
					EventData {
						resource: id,
						id: Self::NOP_OP_ID,
					}
					.into(),
				);

				if let Err(err) = unsafe {
					this.resource
						.ops
						.start_submit::<{ Self::NOP_OP_ID }>(rt, &entry, cx)
				} {
					Poll::Ready(Some(Err(err)))
				} else {
					Poll::Pending
				}
			}
		}
	}
}

impl Drop for NopStream {
	fn drop(&mut self) {
		let _ = self.sender.send(WorkerMessage::CloseResource {
			id: self.resource.id,
			complete: Waker::noop().clone(),
		});
	}
}
