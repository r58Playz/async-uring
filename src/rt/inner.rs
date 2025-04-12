use std::{os::fd::OwnedFd, task::Waker};

use futures::{StreamExt, TryStreamExt};
use io_uring::cqueue;

use crate::Result;

use super::{
	UringDataHandle,
	channel::{ChannelRecv, ChannelSend},
	completion::CqueueStream,
	deps::AsyncFd,
	operation::{EventData, Operations},
	resource::{RegisterResourceSender, Resource, WorkerResource, WorkerResourceSlab},
	select::{PollNext, select_with_strategy},
};

#[derive(Default)]
struct WorkerStreamState {
	disable_actor: bool,
	next: PollNext,
}

pub(crate) enum WorkerMessage {
	Uring {
		info: EventData,
		event: cqueue::Entry,
	},
	RegisterResource {
		fd: Option<OwnedFd>,
		ops: Operations,
		complete: RegisterResourceSender,
	},
	CloseResource {
		id: u32,
		complete: Waker,
	},
	Stop,
}

pub(crate) struct UringRuntimeWorker {
	rt: ChannelRecv<WorkerMessage>,
}
pub(crate) type RuntimeWorkerChannel = ChannelSend<WorkerMessage>;

impl UringRuntimeWorker {
	pub fn new() -> (Self, RuntimeWorkerChannel) {
		let rx = ChannelRecv::new();
		let tx = rx.sender();
		(Self { rt: rx }, tx)
	}

	pub async fn work<Fd: AsyncFd>(self, handle: UringDataHandle) -> Result<()> {
		let data = handle.load().unwrap();

		let uring_events = CqueueStream::<Fd>::new(data)?;
		let mut combined = select_with_strategy(
			uring_events.map_ok(|x| WorkerMessage::Uring {
				info: EventData::from(x.user_data()),
				event: x,
			}),
			self.rt.map(Ok),
			|x: &mut WorkerStreamState| {
				if x.disable_actor {
					PollNext::Left
				} else {
					x.next.toggle()
				}
			},
		);

		let mut resources = WorkerResourceSlab::new();

		while let Some(evt) = combined.next().await.transpose()? {
			match evt {
				WorkerMessage::Uring { info, event } => {
					if let Some(resource) = resources.get(info.resource) {
						if let Some(op) = resource.ops.get(info.id) {
							// this drops any data that was needed for the op if it was cancelled
							op.wake(event.result());
						} else {
							panic!("dropped message {info:?}");
						}
					} else {
						panic!("dropped message {info:?}");
					}
				}
				WorkerMessage::RegisterResource { ops, fd, complete } => {
					let worker = WorkerResource {
						ops: ops.clone(),
						fd,
					};

					match resources.insert(worker) {
						Ok(id) => {
							let resource = Resource::new(id, ops);
							let _ = complete.send(Ok(resource));
						}
						Err(err) => {
							let _ = complete.send(Err(err));
						}
					}
				}
				WorkerMessage::CloseResource { id, complete } => {
					let _ = resources.remove(id);
					complete.wake();
				}
				WorkerMessage::Stop => break,
			}
		}

		handle.destroy();

		Ok(())
	}
}
