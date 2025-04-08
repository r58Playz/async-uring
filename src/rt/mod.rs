use std::{
	marker::PhantomData,
	sync::{
		Arc, Mutex,
		atomic::{AtomicBool, Ordering},
	},
};

use inner::{RuntimeWorkerChannel, UringRuntimeWorker, WorkerMessage};
use io_uring::{IoUring, cqueue, squeue};

use crate::{net::tcp::TcpStream, nop::NopStream, Result};

mod channel;
mod completion;
mod deps;
mod select;

pub(crate) mod inner;
pub(crate) mod operation;
pub(crate) mod resource;

pub use deps::AsyncFd;

pub(crate) type Uring = IoUring<squeue::Entry, cqueue::Entry>;

#[derive(Clone)]
pub(crate) struct UringDataHandle(Arc<UringData>);

impl UringDataHandle {
	pub fn new(data: UringData) -> Self {
		Self(Arc::new(data))
	}

	pub fn load(&self) -> Option<&UringData> {
		self.0
			.alive
			.load(Ordering::Relaxed)
			.then_some(self.0.as_ref())
	}

	pub fn destroy(&self) {
		self.0.alive.store(false, Ordering::Relaxed);
	}
}

pub(crate) struct UringData {
	alive: AtomicBool,

	sq_lock: Mutex<()>,
	uring: Uring,
}
impl UringData {
	pub fn new(uring: Uring) -> Self {
		Self {
			alive: AtomicBool::new(true),
			uring,

			sq_lock: Mutex::new(()),
		}
	}

	/// SAFETY: make sure entry will stay alive
	pub unsafe fn submit(&self, entry: &squeue::Entry) -> Result<()> {
		let lock = self.sq_lock.lock().unwrap();
		// SAFETY: sq is protected by the lock
		let mut sq = unsafe { self.uring.submission_shared() };

		// SAFETY: enforced by the caller
		while unsafe { sq.push(entry) }.is_err() {
			match self.uring.submit() {
				Ok(_) => {}
				Err(x) => {
					drop(sq);
					drop(lock);
					return Err(x.into());
				}
			}
			sq.sync();
		}

		// sq syncs when dropped, so we don't need to sync ourself
		drop(sq);
		drop(lock);

		self.uring.submit()?;

		Ok(())
	}
}

pub struct UringRuntimeBuilder<Fd: AsyncFd> {
	phantom: PhantomData<Fd>,
}

impl<Fd: AsyncFd> Default for UringRuntimeBuilder<Fd> {
	fn default() -> Self {
		Self::new()
	}
}

impl<Fd: AsyncFd> UringRuntimeBuilder<Fd> {
	pub fn new() -> Self {
		Self {
			phantom: PhantomData,
		}
	}

	pub fn build(self) -> Result<(UringRuntime, impl Future<Output = Result<()>> + Send)> {
		let uring = IoUring::builder()
			.setup_submit_all()
			.setup_sqpoll(1_000)
			.build(1024)?;
		let data = UringDataHandle::new(UringData::new(uring));

		let (rt, channel) = UringRuntimeWorker::new();

		Ok((
			UringRuntime {
				data: data.clone(),
				rt: channel,
			},
			rt.work::<Fd>(data),
		))
	}
}

pub struct UringRuntime {
	data: UringDataHandle,
	rt: RuntimeWorkerChannel,
}

impl UringRuntime {
	pub fn builder<Fd: AsyncFd>() -> UringRuntimeBuilder<Fd> {
		UringRuntimeBuilder::new()
	}

	pub async fn register_tcp(&self, stream: std::net::TcpStream) -> Result<TcpStream> {
		TcpStream::new(stream, self.data.clone(), self.rt.clone()).await
	}

	pub async fn nop_stream(&self) -> Result<NopStream> {
		NopStream::new(self.data.clone(), self.rt.clone()).await
	}

	pub fn stop(&self) -> Result<()> {
		self.rt.send(WorkerMessage::Stop)
	}
}
