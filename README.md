# async-uring
io-uring on top of any async runtime using `AsyncRead` and `AsyncWrite`

> [!WARNING]
> This library is probably still very unsound and not cancellation safe. I'm working on fixing that though

## Why tokio rw traits?
The `FuturesAsyncReadCompatExt` trait's compatibility layer re-initializes the buffer on every poll_read, making all reads output zeroes. This problem doesn't happen when going the other way

## benches
see `bench` folder.
