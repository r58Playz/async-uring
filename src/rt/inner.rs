use std::{
	os::fd::{IntoRawFd, OwnedFd},
	sync::{Arc, atomic::AtomicBool},
};

use futures::{StreamExt, TryStreamExt};
use io_uring::cqueue;

use crate::{Result, rt::operation::OperationPollState};

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
	CloseResource(Resource),
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
		let mut closing: Vec<Resource> = Vec::new();

		macro_rules! close {
			($resource:expr) => {
				let resource = $resource;
				if let Some(mut val) = resources.remove(resource)
					&& val.closing()
					&& let Some(fd) = val.fd.take()
				{
					// uring already closed the fd
					let _ = fd.into_raw_fd();
				}
			};
		}

		while let Some(evt) = combined.next().await.transpose()? {
			match evt {
				WorkerMessage::Uring { info, event } => {
					if let Some(resource) = closing.iter_mut().find(|x| x.id == info.resource) {
						// TODO this path still isn't proper and could lead to mem leaks if closing
						// during a cancellation. i should probably just make another stream for
						// this and poll the regular path. 
						if let Some(state) = resource.ops.poll_state(info.id) {
							debug_assert!(matches!(state, OperationPollState::Submitting));
							*state = OperationPollState::Idle;

							if !resource
								.ops
								.poll_states()
								.any(|x| matches!(x, OperationPollState::Submitting))
							{
								// all ops finished
								let id = resource.id;

								close!(id);
							}
						} else {
							panic!("dropped message while closing {info:?}");
						}
					} else if let Some(resource) = resources.get(info.resource) {
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
					let closing = Arc::new(AtomicBool::new(false));
					let worker = WorkerResource {
						ops: ops.clone(),
						fd,
						closing: closing.clone(),
					};

					match resources.insert(worker) {
						Ok(id) => {
							let resource = Resource::new(id, ops, closing);
							let _ = complete.send(Ok(resource));
						}
						Err(err) => {
							let _ = complete.send(Err(err));
						}
					}
				}
				WorkerMessage::CloseResource(mut resource) => {
					if resource
						.ops
						.poll_states()
						.any(|x| matches!(x, OperationPollState::Submitting))
					{
						closing.push(resource);
					} else {
						close!(resource.id);
					}
				}
				WorkerMessage::Stop => break,
			}
		}

		handle.destroy();

		Ok(())
	}
}
