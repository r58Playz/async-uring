use std::{
	collections::VecDeque,
	pin::Pin,
	sync::{
		Arc, Mutex, Weak,
		atomic::{AtomicBool, Ordering},
	},
	task::{Context, Poll},
};

use diatomic_waker::DiatomicWaker;
use futures::Stream;

use crate::{Error, Result};

struct ChannelInner<T> {
	empty: AtomicBool,
	messages: Mutex<VecDeque<T>>,
	waker: DiatomicWaker,
}

pub(crate) struct ChannelRecv<T> {
	len: usize,
	inner: Arc<ChannelInner<T>>,
}
impl<T> ChannelRecv<T> {
	pub fn new() -> Self {
		Self {
			inner: Arc::new(ChannelInner {
				empty: AtomicBool::new(true),
				messages: Mutex::new(VecDeque::new()),
				waker: DiatomicWaker::new(),
			}),
			len: 0,
		}
	}

	pub fn sender(&self) -> ChannelSend<T> {
		ChannelSend {
			inner: Arc::downgrade(&self.inner),
		}
	}

	fn get_msg(&mut self) -> Option<T> {
		let mut guard = self.inner.messages.lock().unwrap();
		let msg = guard.pop_front();
		self.len = guard.len();
		if self.len == 0 {
			self.inner.empty.store(true, Ordering::Release);
		}
		msg
	}
}

impl<T> Stream for ChannelRecv<T> {
	type Item = T;

	fn poll_next(mut self: Pin<&mut Self>, cx: &mut Context<'_>) -> Poll<Option<Self::Item>> {
		if self.len > 0 {
			// we have some messages remaining from the last wake
			let msg = self.get_msg();
			debug_assert!(msg.is_some());
			Poll::Ready(msg)
		} else if Arc::weak_count(&self.inner) == 0 {
			// every sender dropped
			Poll::Ready(None)
		} else if self.inner.empty.load(Ordering::Acquire) {
			// senders haven't sent anything yet
			// SAFETY: this is the only task registering
			unsafe { self.inner.waker.register(cx.waker()) };
			Poll::Pending
		} else {
			// senders have sent something
			let msg = self.get_msg();
			debug_assert!(msg.is_some());
			Poll::Ready(msg)
		}
	}
}

pub(crate) struct ChannelSend<T> {
	inner: Weak<ChannelInner<T>>,
}

impl<T> ChannelSend<T> {
	pub fn send(&self, msg: T) -> Result<()> {
		let Some(inner) = self.inner.upgrade() else {
			return Err(Error::NoRuntime);
		};
		let mut guard = inner.messages.lock().unwrap();
		guard.push_back(msg);
		inner.empty.store(false, Ordering::Release);
		drop(guard);

		inner.waker.notify();

		Ok(())
	}
}

impl<T> Clone for ChannelSend<T> {
	fn clone(&self) -> Self {
		Self {
			inner: self.inner.clone(),
		}
	}
}
