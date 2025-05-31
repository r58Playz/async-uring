use std::{
	mem::MaybeUninit,
	pin::Pin,
	task::{Context, Poll, ready},
};

use tokio::io::{AsyncRead, AsyncWrite, ReadBuf};

use crate::rt::operation::{OperationCancelData, ProtectedOps};
// from feature `maybe_uninit_write_slice`
fn write_copy_of_slice<'a, T>(this: &'a mut [MaybeUninit<T>], src: &[T]) -> &'a mut [T]
where
	T: Copy,
{
	// SAFETY: &[T] and &[MaybeUninit<T>] have the same layout
	let uninit_src: &[MaybeUninit<T>] = unsafe { std::mem::transmute(src) };

	this.copy_from_slice(uninit_src);

	// SAFETY: Valid elements have just been copied into `self` so it is initialized
	unsafe { &mut *(this as *mut [MaybeUninit<T>] as *mut [T]) }
}

#[expect(private_bounds)]
pub struct Copying<T: Unpin + ProtectedOps> {
	// this is only an option to allow into_inner.
	// the only time this option will be None is when into_inner explicitly sets it
	inner: Option<T>,

	read_buf: Vec<u8>,
	write_buf: Vec<u8>,
}

#[expect(private_bounds)]
impl<T: Unpin + ProtectedOps> Copying<T> {
	/// Disable copying data into buffers.
	///
	/// # Safety
	/// You must keep any buffers you provide alive for the entire duration of any operations.
	#[allow(clippy::missing_panics_doc)]
	pub unsafe fn into_inner(mut self) -> T {
		self.inner.take().unwrap()
	}
}

impl<T: Unpin + AsyncRead + ProtectedOps> AsyncRead for Copying<T> {
	fn poll_read(
		mut self: Pin<&mut Self>,
		cx: &mut Context<'_>,
		buf: &mut ReadBuf<'_>,
	) -> Poll<std::io::Result<()>> {
		let this = &mut *self;

		// SAFETY: we only copy things in
		let unfilled = unsafe { buf.unfilled_mut() };
		this.read_buf.reserve(unfilled.len());

		let mut inner_buf = ReadBuf::new(&mut this.read_buf);
		ready!(Pin::new(this.inner.as_mut().unwrap()).poll_read(cx, &mut inner_buf))?;

		write_copy_of_slice(
			&mut unfilled[0..inner_buf.filled().len()],
			inner_buf.filled(),
		);
		buf.advance(inner_buf.filled().len());

		Poll::Ready(Ok(()))
	}
}

impl<T: Unpin + AsyncWrite + ProtectedOps> AsyncWrite for Copying<T> {
	fn poll_write(
		mut self: Pin<&mut Self>,
		cx: &mut Context<'_>,
		buf: &[u8],
	) -> Poll<Result<usize, std::io::Error>> {
		let this = &mut *self;
		this.write_buf.reserve(buf.len());
		this.write_buf.copy_from_slice(buf);

		let ret = ready!(Pin::new(this.inner.as_mut().unwrap()).poll_write(cx, &this.write_buf));

		this.write_buf.clear();
		Poll::Ready(ret)
	}

	fn poll_flush(
		mut self: Pin<&mut Self>,
		cx: &mut Context<'_>,
	) -> Poll<Result<(), std::io::Error>> {
		Pin::new(self.inner.as_mut().unwrap()).poll_flush(cx)
	}

	fn poll_shutdown(
		mut self: Pin<&mut Self>,
		cx: &mut Context<'_>,
	) -> Poll<Result<(), std::io::Error>> {
		Pin::new(self.inner.as_mut().unwrap()).poll_shutdown(cx)
	}
}

macro_rules! try_cancel {
	($res:ident, $id:expr, $buf:expr) => {
		$res.ops.try_cancel(
			$id,
			OperationCancelData {
				wake: false,
				buf: std::mem::take(&mut $buf),
			},
		);
	};
}

impl<T: Unpin + ProtectedOps> Drop for Copying<T> {
	fn drop(&mut self) {
		if let Some(mut inner) = self.inner.take() {
			let res = inner.get_resource();
			try_cancel!(res, T::READ_OP_ID, self.read_buf);
			try_cancel!(res, T::WRITE_OP_ID, self.write_buf);
		}
	}
}
