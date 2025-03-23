use std::{
    os::fd::AsRawFd,
    pin::Pin,
    task::{Context, Poll, ready},
};

use futures::Stream;
use io_uring::{CompletionQueue, cqueue};

use super::{AsyncFd, UringData};

pub struct CqueueStream<'a, Fd: AsyncFd> {
    fd: Fd,
    cqueue: CompletionQueue<'a>,
}
unsafe impl<'a, Fd: AsyncFd> Sync for CqueueStream<'a, Fd> {}
unsafe impl<'a, Fd: AsyncFd> Send for CqueueStream<'a, Fd> {}

impl<'a, Fd: AsyncFd> CqueueStream<'a, Fd> {
    pub fn new(rt: &'a UringData) -> crate::Result<Self> {
        let fd = Fd::new(rt.uring.as_raw_fd())?;

        let cqueue = unsafe { rt.uring.completion_shared() };

        Ok(Self { fd, cqueue })
    }
}

impl<'a, Fd: AsyncFd> Stream for CqueueStream<'a, Fd> {
    type Item = crate::Result<cqueue::Entry>;

    fn poll_next(mut self: Pin<&mut Self>, cx: &mut Context<'_>) -> Poll<Option<Self::Item>> {
        if let Some(entry) = self.cqueue.next() {
            Poll::Ready(Some(Ok(entry)))
        } else {
            ready!(self.fd.poll_read_ready(cx))?;
            self.cqueue.sync();

            self.poll_next(cx)
        }
    }
}
