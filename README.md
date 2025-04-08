# async-uring
io-uring on top of any async runtime using `AsyncRead` and `AsyncWrite`

> [!WARNING]
> This library is probably still very unsound and not cancellation safe. I'm working on fixing that though

## benches
Units are 16kb packets per second.
All tests run with 100 sockets averaged over 30 seconds using `tcp_load_test` example in singlethread mode.
Tests in other runtimes have been modified to change the buffer size to 16kb to make it fair.

- `async-uring + tokio ST`: 168862.22
- `async-uring + tokio MT`: 164889.61
- `monoio`: 159179.67
- `tokio-uring`: 155767.05
- `tokio MT`: 111296.24
- `tokio ST`: 5165.71
- `uSockets`: DNF (it segfaulted)
