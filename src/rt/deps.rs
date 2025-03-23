use std::{
    io::Result,
    os::fd::RawFd,
    task::{Context, Poll},
};

pub trait AsyncFd: Sync + Send + Sized + Unpin {
    fn new(fd: RawFd) -> Result<Self>;

    fn poll_read_ready(&self, cx: &mut Context) -> Poll<Result<()>>;
}
