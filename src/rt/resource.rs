use std::{
	os::fd::OwnedFd,
	sync::{
		Arc,
		atomic::{AtomicBool, Ordering},
	},
};

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
	pub fd: Option<OwnedFd>,
	pub ops: Operations,
	pub closing: Arc<AtomicBool>,
}

impl WorkerResource {
	pub fn closing(&self) -> bool {
		self.closing.load(Ordering::Acquire)
	}
}

pub(super) type RegisterResourceSender = oneshot::Sender<Result<Resource>>;

#[derive(Clone)]
pub(crate) struct Resource {
	pub id: u32,
	pub ops: Operations,
	closing: Arc<AtomicBool>,
}

impl Resource {
	pub(super) fn new(id: u32, ops: Operations, closing: Arc<AtomicBool>) -> Self {
		Self { id, ops, closing }
	}

	pub fn dup(&self) -> Self {
		Self {
			id: self.id,
			ops: self.ops.dup(),
			closing: self.closing.clone()
		}
	}

	pub fn closing(&self) -> bool {
		self.closing.load(Ordering::Acquire)
	}

	pub fn set_closing(&self) {
		self.closing.store(true, Ordering::Release);
	}
}
