use std::{
	io,
	mem::ManuallyDrop,
	sync::{
		Arc,
		atomic::{AtomicU64, Ordering},
	},
	task::{Context, Poll},
};

use diatomic_waker::DiatomicWaker;
use io_uring::squeue;

use crate::Result;

use super::{UringData, resource::Resource};

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

#[derive(Debug)]
#[repr(align(4))]
pub(crate) struct OperationCancelData {
	pub wake: bool,
	pub buf: Vec<u8>,
}

pub(crate) enum OperationState {
	Waiting,
	Cancelled(ManuallyDrop<Box<OperationCancelData>>),
	Finished(i32),
}

impl OperationState {
	const TAG_SIZE: u64 = 0b11;

	pub fn cancel_data(self) -> Option<ManuallyDrop<Box<OperationCancelData>>> {
		match self {
			Self::Cancelled(x) => Some(x),
			Self::Waiting | Self::Finished(_) => None,
		}
	}
}

impl From<u64> for OperationState {
	fn from(value: u64) -> Self {
		let (data, tag) = (value & !Self::TAG_SIZE, value & Self::TAG_SIZE);
		match tag {
			1 => Self::Waiting,
			// data is only ever an i32
			#[expect(clippy::cast_possible_truncation)]
			2 => Self::Finished((data >> 3) as i32),
			#[expect(clippy::cast_possible_truncation)]
			// SAFETY: data is only ever an 8 byte aligned pointer
			3 => Self::Cancelled(unsafe {
				ManuallyDrop::new(Box::from_raw(
					std::ptr::null_mut::<OperationCancelData>().with_addr(data as usize),
				))
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
			OperationState::Cancelled(cleanup) => (
				Box::into_raw(ManuallyDrop::into_inner(cleanup)).addr() as u64,
				3u64,
			),
		};

		debug_assert_eq!(tag & !OperationState::TAG_SIZE, 0);
		debug_assert_eq!(ptr & OperationState::TAG_SIZE, 0);

		ptr | (tag & 0b111)
	}
}

// funny hack to workaround no const generic expressions on stable
struct AssertOperationBounds<const ID: u32, const SIZE: usize>;
impl<const ID: u32, const SIZE: usize> AssertOperationBounds<ID, SIZE> {
	const OK: () = assert!((ID as usize) < SIZE, "operation out of bounds");
}

pub(crate) struct Operation<const SIZE: usize> {
	state: AtomicU64,
	waker: DiatomicWaker,
}

impl<const SIZE: usize> Operation<SIZE> {
	pub fn new() -> Self {
		Self {
			state: AtomicU64::new(OperationState::Finished(0).into()),
			waker: DiatomicWaker::new(),
		}
	}

	#[inline(always)]
	pub fn register(
		&self,
		last_state: OperationState,
		cx: &mut Context<'_>,
	) -> std::result::Result<(), OperationState> {
		// SAFETY: the worker never registers a waker
		unsafe { self.waker.register(cx.waker()) };

		if let Err(val) = self.state.compare_exchange(
			last_state.into(),
			OperationState::Waiting.into(),
			Ordering::AcqRel,
			Ordering::Acquire,
		) {
			unsafe {
				self.waker.unregister();
			};
			return Err(val.into());
		}

		Ok(())
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
			self.waker.notify();
		}

		// drop anything that was needed for the op to complete safely
		if let Some(mut cancel) = cancel {
			unsafe { ManuallyDrop::drop(&mut cancel) };
		}
	}

	#[inline(always)]
	pub fn state(&self) -> OperationState {
		self.state.load(Ordering::Acquire).into()
	}

	pub fn cancel(&self, cancel_data: OperationCancelData) -> bool {
		let cancel_data = ManuallyDrop::new(Box::new(cancel_data));
		let our_state = OperationState::Cancelled(cancel_data).into();
		match self
			.state
			.compare_exchange(
				OperationState::Waiting.into(),
				our_state,
				Ordering::AcqRel,
				Ordering::Acquire,
			)
			.unwrap_or_else(|x| x)
			.into()
		{
			OperationState::Waiting => {
				// we were waiting, task cancelled
				true
			}
			OperationState::Finished(_) | OperationState::Cancelled(_) => {
				// we were already done with the op or were already cancelled, drop our state
				unsafe {
					ManuallyDrop::drop(&mut OperationState::from(our_state).cancel_data().unwrap())
				};
				false
			}
		}
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

		let mut state = op.state();
		while let Err(err) = op.register(state, cx) {
			state = err;
		}

		// SAFETY: enforced by caller
		unsafe { rt.submit(entry)? };
		*submission = OperationPollState::Submitting;

		Ok(())
	}

	pub fn poll_submit<const ID: u32>(&mut self, cx: &mut Context) -> Poll<Option<Result<u32>>> {
		let () = AssertOperationBounds::<ID, SIZE>::OK;
		let op = &self.ops[ID as usize];
		let submission = &mut self.submissions[ID as usize];

		macro_rules! finish {
			($ret:expr) => {
				*submission = OperationPollState::Idle;

				if $ret < 0 {
					return Poll::Ready(Some(Err(io::Error::from_raw_os_error(-$ret).into())));
				} else {
					// we already check if it's below 0
					#[expect(clippy::cast_sign_loss)]
					return Poll::Ready(Some(Ok($ret as u32)));
				}
			};
		}

		match *submission {
			OperationPollState::Idle => Poll::Ready(None),
			OperationPollState::Submitting => match op.state() {
				OperationState::Finished(ret) => {
					finish!(ret);
				}
				OperationState::Waiting => match op.register(OperationState::Waiting, cx) {
					Ok(()) | Err(OperationState::Waiting) => Poll::Pending,
					Err(OperationState::Finished(ret)) => {
						finish!(ret);
					}
					Err(OperationState::Cancelled(_)) => {
						todo!();
					}
				},
				OperationState::Cancelled(_) => {
					todo!("implement polling an operation that was cancelled");
				}
			},
		}
	}

	// this isn't possible to constify without generic_const_exprs
	pub fn try_cancel(&mut self, id: u32, data: OperationCancelData) -> bool {
		let op = &self.ops[id as usize];
		let submission = &mut self.submissions[id as usize];

		if op.cancel(data) {
			*submission = OperationPollState::Idle;
			true
		} else {
			false
		}
	}

	pub fn get(&self, id: u32) -> Option<&Operation<SIZE>> {
		self.ops.get(id as usize)
	}
}

/// SAFETY: make sure the sq entry stays alive
macro_rules! poll_op_impl {
	($id:expr, $this:expr, $cx:expr, {
		Some(Ok(val)) => $ok:expr,
		None => $new:expr
	}) => {
		(|| {
			use std::task::Poll;
			use $crate::{Error, rt::operation::EventData};

			let Some(rt) = $this.rt.load() else {
				return Poll::Ready(Err(Error::NoRuntime));
			};

			let id = $this.resource.id;
			return match ready!($this.resource.ops.poll_submit::<{ $id }>($cx)) {
				Some(Ok(val)) => ($ok)(val),
				Some(Err(err)) => Poll::Ready(Err(err)),
				None => {
					if $this.resource.closing() {
						Poll::Ready(Err(Error::ResourceClosing))
					} else {
						let val = ($new)();
						let val = match val {
							Ok(val) => val,
							Err(err) => return Poll::Ready(Err(err)),
						};

						let entry = val.build().user_data(
							EventData {
								resource: id,
								id: $id,
							}
							.into(),
						);

						if let Err(err) =
							// SAFETY: enforced by the caller
							unsafe {
								$this.resource.ops.start_submit::<{ $id }>(rt, &entry, $cx)
							} {
							Poll::Ready(Err(err))
						} else {
							Poll::Pending
						}
					}
				}
			};
		})()
	};
}
pub(crate) use poll_op_impl;

pub(crate) trait ProtectedOps {
	fn get_resource(&mut self) -> &mut Resource;
	const READ_OP_ID: u32;
	const WRITE_OP_ID: u32;
}
