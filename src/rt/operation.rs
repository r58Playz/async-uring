use std::{
	cell::UnsafeCell,
	io,
	sync::{
		Arc,
		atomic::{AtomicBool, AtomicU64, Ordering},
	},
	task::{Context, Poll, Waker},
};

use event_listener::{Event, EventListener};
use futures::FutureExt;
use io_uring::squeue;

use crate::Result;

use super::UringData;

#[derive(Debug)]
pub(crate) struct EventData {
	pub resource: u32,
	pub id: u32,
}

impl From<EventData> for u64 {
	fn from(value: EventData) -> Self {
		let EventData {
			resource: top,
			id: bottom,
		} = value;

		(u64::from(top) << 32) | u64::from(bottom)
	}
}

impl From<u64> for EventData {
	fn from(value: u64) -> Self {
		// this is 2 u32s packed into 1 u64
		#[expect(clippy::cast_possible_truncation)]
		let (top, bottom) = ((value >> 32) as u32, value as u32);

		Self {
			resource: top,
			id: bottom,
		}
	}
}

struct OpsDisabledData {
	disabled: AtomicBool,
	waker: Event,
}
impl OpsDisabledData {
	pub fn new() -> Arc<Self> {
		Arc::new(Self {
			disabled: AtomicBool::new(false),
			waker: Event::new(),
		})
	}

	pub fn set(&self, disabled: bool) {
		self.disabled.store(disabled, Ordering::Relaxed);
		if !disabled {
			self.waker.notify_additional(usize::MAX);
		}
	}
}

enum OpsDisabledState {
	Disarmed,
	Arming(EventListener),
	Armed,
}

impl OpsDisabledState {
	#[inline(always)]
	fn disarm(&mut self) {
		debug_assert!(matches!(self, Self::Armed));
		*self = Self::Disarmed;
	}

	#[inline(always)]
	fn poll_arm(&mut self, data: &OpsDisabledData, cx: &mut Context) -> Poll<()> {
		macro_rules! try_arm {
			() => {
				if data.disabled.load(Ordering::Relaxed) {
					Poll::Pending
				} else {
					*self = Self::Armed;
					Poll::Ready(())
				}
			};
		}

		match self {
			Self::Armed => Poll::Ready(()),
			Self::Disarmed => {
				if data.disabled.load(Ordering::Relaxed) {
					*self = Self::Arming(data.waker.listen());
					try_arm!()
				} else {
					*self = Self::Armed;
					Poll::Ready(())
				}
			}
			Self::Arming(ev) => match ev.poll_unpin(cx) {
				Poll::Ready(()) => {
					if data.disabled.load(Ordering::Relaxed) {
						*self = Self::Arming(data.waker.listen());
						try_arm!()
					} else {
						*self = Self::Armed;
						Poll::Ready(())
					}
				}
				Poll::Pending => Poll::Pending,
			},
		}
	}
}

pub(crate) struct OpsDisabled {
	handle: Arc<OpsDisabledData>,
	state: OpsDisabledState,
}

impl Clone for OpsDisabled {
	fn clone(&self) -> Self {
		Self {
			handle: self.handle.clone(),
			state: OpsDisabledState::Disarmed,
		}
	}
}

impl OpsDisabled {
	pub fn new() -> Self {
		Self {
			handle: OpsDisabledData::new(),
			state: OpsDisabledState::Disarmed,
		}
	}

	pub fn set(&self, disabled: bool) {
		self.handle.set(disabled);
	}

	#[inline(always)]
	pub fn poll_arm(&mut self, cx: &mut Context) -> Poll<()> {
		self.state.poll_arm(&self.handle, cx)
	}

	#[inline(always)]
	pub fn disarm(&mut self) {
		self.state.disarm();
	}
}

#[derive(Debug)]
#[repr(align(8))]
pub(crate) struct OperationCancelData {
	wake: bool,
}

#[derive(Debug)]
pub(crate) enum OperationState {
	// only the op task can access waker here
	Waiting,
	// only happens if an op gets dropped while waiting. only op task can access waker
	Cancelled(Box<OperationCancelData>),
	// only the worker task can access waker here
	Finished(i32),
}

impl OperationState {
	pub fn cancel_data(self) -> Option<Box<OperationCancelData>> {
		match self {
			Self::Cancelled(x) => Some(x),
			Self::Waiting | Self::Finished(_) => None,
		}
	}
}

impl From<u64> for OperationState {
	fn from(value: u64) -> Self {
		let (data, tag) = (value & !0b111, value & 0b111);
		match tag {
			1 => Self::Waiting,
			// data is only ever an i32
			#[expect(clippy::cast_possible_truncation)]
			2 => Self::Finished((data >> 3) as i32),
			#[expect(clippy::cast_possible_truncation)]
			// SAFETY: data is only ever an 8 byte aligned pointer
			3 => Self::Cancelled(unsafe {
				Box::from_raw(std::ptr::null_mut::<OperationCancelData>().with_addr(data as usize))
			}),
			_ => unreachable!("{value:08X}"),
		}
	}
}
impl From<OperationState> for u64 {
	fn from(value: OperationState) -> Self {
		let (ptr, tag) = match value {
			OperationState::Waiting => (0, 1u64),
			// casted value is immediately casted back to i32
			#[expect(clippy::cast_sign_loss)]
			OperationState::Finished(val) => ((val as u64) << 3, 2u64),
			OperationState::Cancelled(cleanup) => (Box::into_raw(cleanup).addr() as u64, 3u64),
		};

		debug_assert_eq!(tag & !0b111, 0);

		ptr | (tag & 0b111)
	}
}

// funny hack to workaround no const generic expressions on stable
struct AssertOperationBounds<const ID: u32, const SIZE: usize>;
impl<const ID: u32, const SIZE: usize> AssertOperationBounds<ID, SIZE> {
	const OK: () = assert!((ID as usize) < SIZE, "operation out of bounds");
}

struct OperationWakerList<const SIZE: usize> {
	wakers: [Waker; SIZE],
}
impl<const SIZE: usize> OperationWakerList<SIZE> {
	pub fn new() -> Self {
		Self {
			wakers: std::array::from_fn(|_| Waker::noop().clone()),
		}
	}

	pub fn register<const ID: u32>(&mut self, cx: &mut Context) {
		let () = AssertOperationBounds::<ID, SIZE>::OK;
		self.wakers[ID as usize].clone_from(cx.waker());
	}
	pub fn wake(&mut self) {
		for waker in &mut self.wakers {
			std::mem::replace(waker, Waker::noop().clone()).wake();
		}
	}
}

pub(crate) struct Operation<const SIZE: usize> {
	state: AtomicU64,
	waker: UnsafeCell<OperationWakerList<SIZE>>,
}

// SAFETY: OperationState acts as a mutex and waker is send/sync
unsafe impl<const SIZE: usize> Sync for Operation<SIZE> {}
// SAFETY: OperationState acts as a mutex and waker is send/sync
unsafe impl<const SIZE: usize> Send for Operation<SIZE> {}

impl<const SIZE: usize> Operation<SIZE> {
	pub fn new() -> Self {
		Self {
			state: AtomicU64::new(OperationState::Finished(0).into()),
			waker: UnsafeCell::new(OperationWakerList::new()),
		}
	}

	#[inline(always)]
	pub fn register<const ID: u32>(&self, cx: &mut Context<'_>) {
		debug_assert!(matches!(
			self.state(),
			OperationState::Finished(_) | OperationState::Waiting
		));
		self.state
			.store(OperationState::Waiting.into(), Ordering::Release);

		// SAFETY: we are waiting
		unsafe { &mut *self.waker.get() }.register::<ID>(cx);
	}

	#[inline(always)]
	pub fn wake(&self, val: i32) {
		let state: OperationState = self
			.state
			.swap(OperationState::Finished(val).into(), Ordering::AcqRel)
			.into();
		debug_assert!(matches!(
			state,
			OperationState::Waiting | OperationState::Cancelled(_)
		));
		let cancel = state.cancel_data();

		if cancel.as_ref().is_none_or(|x| x.wake) {
			// SAFETY: we are finished
			unsafe { &mut *self.waker.get() }.wake();
		}

		// drop anything that was needed for the op to complete safely
		drop(cancel);
	}

	#[inline(always)]
	pub fn state(&self) -> OperationState {
		self.state.load(Ordering::Acquire).into()
	}
}

#[derive(Copy, Clone)]
enum OperationPollState {
	Idle,
	Submitting,
}

pub(crate) struct Operations<const SIZE: usize = 4> {
	ops: Arc<[Operation<SIZE>; SIZE]>,
	submissions: [OperationPollState; SIZE],
}

impl<const SIZE: usize> Clone for Operations<SIZE> {
	fn clone(&self) -> Self {
		Self {
			ops: self.ops.clone(),
			submissions: [OperationPollState::Idle; SIZE],
		}
	}
}

impl<const SIZE: usize> Operations<SIZE> {
	pub fn new(ops: [Operation<SIZE>; SIZE]) -> Self {
		Self {
			ops: Arc::new(ops),
			submissions: [OperationPollState::Idle; SIZE],
		}
	}

	pub fn new_from_size() -> Self {
		Operations::new(std::array::from_fn::<_, SIZE, _>(|_| Operation::new()))
	}

	/// SAFETY: make sure entry will stay alive
	pub unsafe fn start_submit<const ID: u32>(
		&mut self,
		rt: &UringData,
		entry: &squeue::Entry,
		cx: &mut Context,
	) -> Result<()> {
		let () = AssertOperationBounds::<ID, SIZE>::OK;
		let op = &self.ops[ID as usize];
		let submission = &mut self.submissions[ID as usize];

		op.register::<ID>(cx);
		// SAFETY: enforced by caller
		unsafe { rt.submit(entry)? };
		*submission = OperationPollState::Submitting;

		Ok(())
	}

	pub fn poll_submit<const ID: u32>(&mut self, cx: &mut Context) -> Poll<Option<Result<u32>>> {
		let () = AssertOperationBounds::<ID, SIZE>::OK;
		let op = &self.ops[ID as usize];
		let submission = &mut self.submissions[ID as usize];

		match *submission {
			OperationPollState::Idle => Poll::Ready(None),
			OperationPollState::Submitting => {
				if let OperationState::Finished(ret) = op.state() {
					*submission = OperationPollState::Idle;

					if ret < 0 {
						Poll::Ready(Some(Err(io::Error::from_raw_os_error(-ret).into())))
					} else {
						// we already check if it's below 0
						#[expect(clippy::cast_sign_loss)]
						Poll::Ready(Some(Ok(ret as u32)))
					}
				} else {
					op.register::<ID>(cx);
					Poll::Pending
				}
			}
		}
	}

	pub fn poll_wait_for<const ID: u32>(&mut self, cx: &mut Context) -> Poll<()> {
		let () = AssertOperationBounds::<ID, SIZE>::OK;
		let op = &self.ops[ID as usize];
		let submission = &mut self.submissions[ID as usize];
		match *submission {
			OperationPollState::Idle => Poll::Ready(()),
			OperationPollState::Submitting => {
				if let OperationState::Finished(_) = op.state() {
					Poll::Ready(())
				} else {
					op.register::<ID>(cx);
					Poll::Pending
				}
			}
		}
	}

	pub fn get(&self, id: u32) -> Option<&Operation<SIZE>> {
		self.ops.get(id as usize)
	}

	pub fn inflight(&self) -> usize {
		self.ops
			.iter()
			.map(Operation::state)
			.filter(|x| matches!(x, OperationState::Waiting))
			.count()
	}
}
