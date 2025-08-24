use std::{
	pin::Pin,
	task::{Context, Poll, ready},
};

use futures::Stream;

use crate::rt::{inner::WorkerMessage, operation::OperationPollState, resource::Resource};

struct ClosingResource {
	resource: Resource<4>,
	polled: [bool; 4],
}
impl ClosingResource {
	fn new(resource: Resource) -> Self {
		Self {
			resource,
			polled: [false; 4],
		}
	}

	fn try_advance_polls(&mut self, cx: &mut Context<'_>) -> Poll<bool> {
		macro_rules! poll {
			($i:expr) => {
				if !self.polled[$i] {
					self.polled[$i] = true;
					ready!(self.resource.ops.poll_submit::<{ $i }>(cx));
				}
			};
		}

		poll!(0);
		poll!(1);
		poll!(2);
		poll!(3);

		self.polled = [false; 4];

		Poll::Ready(
			!self
				.resource
				.ops
				.poll_states()
				.any(|x| matches!(x, OperationPollState::Submitting)),
		)
	}
}

pub(crate) struct CleanupStream {
	resources: Vec<ClosingResource>,
}
impl CleanupStream {
	pub fn new() -> Self {
		Self { resources: Vec::new() }
	}

	pub fn push(&mut self, resource: Resource) {
		self.resources.push(ClosingResource::new(resource));
	}
}
impl Stream for CleanupStream {
	type Item = crate::Result<WorkerMessage>;

	fn poll_next(mut self: Pin<&mut Self>, cx: &mut Context<'_>) -> Poll<Option<Self::Item>> {
		let mut remove = None;
		for (i, resource) in self.resources.iter_mut().enumerate() {
			if ready!(resource.try_advance_polls(cx)) {
				remove = Some(i);
				break;
			}
		}
		if let Some(i) = remove {
			Poll::Ready(Some(Ok(WorkerMessage::FinishResource(self.resources.remove(i).resource))))
		} else {
			Poll::Pending
		}
	}
}

