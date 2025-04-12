use std::os::fd::OwnedFd;

use futures::channel::oneshot;
use slab::Slab;

use crate::{Error, Result};

use super::operation::Operations;

pub(super) struct WorkerResourceSlab {
	slab: Slab<WorkerResource>,
}

impl WorkerResourceSlab {
	pub fn new() -> Self {
		Self { slab: Slab::new() }
	}

	pub fn insert(&mut self, resource: WorkerResource) -> Result<u32> {
		self.slab
			.insert(resource)
			.try_into()
			.map_err(|_| Error::TooManyResources)
	}

	pub fn get(&self, id: u32) -> Option<&WorkerResource> {
		self.slab.get(id as usize)
	}

	pub fn remove(&mut self, id: u32) -> Option<WorkerResource> {
		self.slab.try_remove(id as usize)
	}
}

pub(super) struct WorkerResource {
	#[expect(dead_code)]
	pub fd: Option<OwnedFd>,
	pub ops: Operations,
}

pub(super) type RegisterResourceSender = oneshot::Sender<Result<Resource>>;

pub(crate) struct Resource {
	pub id: u32,
	pub ops: Operations,
}

impl Resource {
	pub(super) fn new(id: u32, ops: Operations) -> Self {
		Self { id, ops }
	}
}
