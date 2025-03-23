use std::{
    cell::UnsafeCell,
    io,
    pin::Pin,
    sync::{
        Arc,
        atomic::{AtomicBool, AtomicU64, Ordering},
    },
    task::{Context, Poll, Waker},
};

use async_event::Event;
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

        ((top as u64) << 32) | bottom as u64
    }
}

impl From<u64> for EventData {
    fn from(value: u64) -> Self {
        let (top, bottom) = ((value >> 32) as u32, value as u32);
        Self {
            resource: top,
            id: bottom,
        }
    }
}

pub(super) struct OpsDisabled {
    disabled: AtomicBool,
    waker: Event,
}
impl OpsDisabled {
    pub fn new() -> Arc<Self> {
        Arc::new(Self {
            disabled: AtomicBool::new(false),
            waker: Event::new(),
        })
    }

    pub fn set(&self, disabled: bool) {
        self.disabled.store(disabled, Ordering::Relaxed);
        if !disabled {
            self.waker.notify_all();
        }
    }

    pub async fn wait(&self) {
        self.waker
            .wait_until(|| (!self.disabled.load(Ordering::Relaxed)).then_some(()))
            .await;
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
            2 => Self::Finished((data >> 3) as i32),
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
            OperationState::Waiting => (0, 1),
            OperationState::Finished(val) => ((val as u64) << 3, 2),
            OperationState::Cancelled(cleanup) => (Box::into_raw(cleanup).addr() as u64, 3),
        };

        ptr | ((tag as u64) & 0b111)
    }
}

pub(crate) struct Operation {
    state: AtomicU64,
    waker: UnsafeCell<Waker>,
}

unsafe impl Sync for Operation {}
unsafe impl Send for Operation {}

impl Operation {
    pub fn new() -> Self {
        Self {
            state: AtomicU64::new(OperationState::Finished(0).into()),
            waker: UnsafeCell::new(Waker::noop().clone()),
        }
    }

    pub fn register(&self, cx: &mut Context<'_>) {
        debug_assert!(matches!(self.state(), OperationState::Finished(_)));
        self.state
            .store(OperationState::Waiting.into(), Ordering::Release);

        unsafe { &mut *self.waker.get() }.clone_from(cx.waker());
    }

    pub fn wake(&self, val: i32) {
        let state: OperationState = self
            .state
            .swap(OperationState::Finished(val).into(), Ordering::Release)
            .into();
        debug_assert!(matches!(
            state,
            OperationState::Waiting | OperationState::Cancelled(_)
        ));
        let cancel = state.cancel_data();

        if cancel.as_ref().is_none_or(|x| x.wake) {
            unsafe { &mut *self.waker.get() }.wake_by_ref();
        }

        // drop anything that was needed for the op to complete safely
        drop(cancel);
    }

    pub fn state(&self) -> OperationState {
        self.state.load(Ordering::Acquire).into()
    }
}

pub(crate) struct Operations<Ops: ?Sized> {
    ops: Ops,
}

impl<Ops> Operations<Ops> {
    pub fn new(ops: Ops) -> Self {
        Self { ops }
    }
}

impl Operations<[Operation]> {
    pub unsafe fn submit<'a>(
        &'a self,
        id: u32,
        rt: &'a UringData,
        entry: squeue::Entry,
    ) -> SubmitOperation<'a> {
        SubmitOperation {
            op: &self.ops[id as usize],
            rt,
            entry: Some(entry),
        }
    }

    pub fn get(&self, id: u32) -> Option<&Operation> {
        self.ops.get(id as usize)
    }

    pub fn inflight(&self) -> usize {
        self.ops
            .iter()
            .map(|x| x.state())
            .filter(|x| matches!(x, OperationState::Waiting))
            .count()
    }
}

pub(crate) struct SubmitOperation<'a> {
    op: &'a Operation,
    rt: &'a UringData,
    entry: Option<squeue::Entry>,
}

impl<'a> Future for SubmitOperation<'a> {
    type Output = Result<i32>;

    fn poll(mut self: Pin<&mut Self>, cx: &mut Context<'_>) -> Poll<Self::Output> {
        if let Some(entry) = self.entry.take() {
            self.op.register(cx);
            // SAFETY: enforced by caller
            unsafe { self.rt.submit(&entry)? };
            Poll::Pending
        } else if let OperationState::Finished(ret) = self.op.state() {
            if ret < 0 {
                return Poll::Ready(Err(io::Error::from_raw_os_error(-ret).into()));
            } else {
                return Poll::Ready(Ok(ret));
            }
        } else {
            self.op.register(cx);
            Poll::Pending
        }
    }
}
