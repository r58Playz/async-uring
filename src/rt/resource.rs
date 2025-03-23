use std::{
    os::fd::{AsRawFd, OwnedFd},
    sync::Arc,
};

use futures::channel::oneshot;
use slab::Slab;

use crate::Result;

use super::{
    UringData,
    operation::{Operation, Operations, OpsDisabled},
};

pub(super) struct WorkerResourceSlab<'a> {
    rt: &'a UringData,

    slab: Slab<WorkerResource>,
}

impl<'a> WorkerResourceSlab<'a> {
    pub fn new(rt: &'a UringData) -> Result<Self> {
        rt.uring.submitter().register_files_sparse(16)?;
        Ok(Self {
            rt,
            slab: Slab::with_capacity(16),
        })
    }

    pub fn pending_resize(&mut self) -> Option<u32> {
        (self.slab.len() == self.slab.capacity()).then(|| {
            self.slab.reserve(1);
            self.slab.capacity() as u32
        })
    }

    pub fn resize(&mut self, capacity: u32) -> Result<()> {
        println!("resizing kernel uring entries to {capacity:?}");

        // clear fd table
        self.rt.uring.submitter().unregister_files()?;

        // resize table
        self.rt.uring.submitter().register_files_sparse(capacity)?;

        // add back fds
        let fds: Vec<_> = self.slab.iter().map(|x| x.1.fd.as_raw_fd()).collect();
        self.rt.uring.submitter().register_files_update(0, &fds)?;

        Ok(())
    }

    pub fn insert(&mut self, resource: WorkerResource) -> Result<u32> {
        let fd = resource.fd.as_raw_fd();
        let key = self.slab.insert(resource) as u32;

        self.rt
            .uring
            .submitter()
            .register_files_update(key, &[fd])?;

        Ok(key)
    }

    pub fn get(&self, id: u32) -> Option<&WorkerResource> {
        self.slab.get(id as usize)
    }

    pub fn remove(&mut self, id: u32) -> Result<Option<WorkerResource>> {
        let Some(resource) = self.slab.try_remove(id as usize) else {
            return Ok(None);
        };

        self.rt.uring.submitter().register_files_update(id, &[-1])?;

        Ok(Some(resource))
    }

    pub fn inflight_ops(&self) -> usize {
        self.slab.iter().map(|x| x.1.ops.inflight()).sum()
    }
}

pub(super) struct WorkerResource {
    pub fd: OwnedFd,
    pub ops: Arc<Operations<[Operation]>>,
}

pub(super) type RegisterResourceSender = oneshot::Sender<Result<Resource>>;
pub(super) struct PendingResize {
    pub worker: WorkerResource,
    pub complete: RegisterResourceSender,
    pub new_size: u32,
}

pub(crate) struct Resource {
    ops_disabled: Arc<OpsDisabled>,

    ops: Arc<Operations<[Operation]>>,

    pub id: u32,
}

impl Resource {
    pub(super) fn new(
        id: u32,
        ops_disabled: Arc<OpsDisabled>,
        ops: Arc<Operations<[Operation]>>,
    ) -> Self {
        Self {
            ops_disabled,
            ops,
            id,
        }
    }

    pub async fn ops(&self) -> &Operations<[Operation]> {
        self.ops_disabled.wait().await;
        &self.ops
    }
}
