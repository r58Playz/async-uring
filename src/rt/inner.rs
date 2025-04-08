use std::{os::fd::OwnedFd, task::Waker};

use futures::{StreamExt, TryStreamExt};
use io_uring::cqueue;

use crate::Result;

use super::{
	UringDataHandle,
	channel::{ChannelRecv, ChannelSend},
	completion::CqueueStream,
	deps::AsyncFd,
	operation::{EventData, Operations, OpsDisabled},
	resource::{
		PendingResize, RegisterResourceSender, Resource, WorkerResource, WorkerResourceSlab,
	},
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

		let ops_disabled = OpsDisabled::new();
		let mut resources = WorkerResourceSlab::new(data)?;
		let mut pending_resource_resize: Option<PendingResize> = None;

		while let Some(evt) = combined.next().await.transpose()? {
			match evt {
				WorkerMessage::Uring { info, event } => {
					if let Some(resource) = resources.get(info.resource) {
						if let Some(op) = resource.ops.get(info.id) {
							// this drops any data that was needed for the op if it was cancelled
							op.wake(event.result());
						}
					} else {
						panic!("dropped message {info:?}");
					}

					if let Some(PendingResize {
						worker,
						complete,
						new_size,
					}) = pending_resource_resize.take_if(|_| resources.inflight_ops() == 0)
					{
						resources.resize(new_size)?;

						let ops = worker.ops.clone();

						match resources.insert(worker) {
							Ok(id) => {
								let resource = Resource::new(id, ops_disabled.clone(), ops);
								let _ = complete.send(Ok(resource));
							}
							Err(err) => {
								let _ = complete.send(Err(err));
							}
						}

						combined.get_state_mut().disable_actor = false;
						ops_disabled.set(false);
						println!("ops reenabled");
					}
				}
				WorkerMessage::RegisterResource { ops, fd, complete } => {
					let worker = WorkerResource {
						ops: ops.clone(),
						fd,
					};

					if let Some(new_size) = resources.pending_resize() {
						if dbg!(resources.inflight_ops()) != 0 {
							let _ = pending_resource_resize.insert(PendingResize {
								worker,
								complete,
								new_size,
							});
							combined.get_state_mut().disable_actor = true;
							ops_disabled.set(true);
							println!("ops disabled");

							continue;
						}
						resources.resize(new_size)?;
					}

					match resources.insert(worker) {
						Ok(id) => {
							let resource = Resource::new(id, ops_disabled.clone(), ops);
							let _ = complete.send(Ok(resource));
						}
						Err(err) => {
							let _ = complete.send(Err(err));
						}
					}
				}
				WorkerMessage::CloseResource { id, complete } => {
					let _ = resources.remove(id)?;
					complete.wake();
				}
				WorkerMessage::Stop => break,
			}
		}

		handle.destroy();

		Ok(())
	}
}
