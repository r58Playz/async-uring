# async-uring
io-uring on top of any async runtime using `AsyncRead` and `AsyncWrite`

## benches
Units: 16kb packets per second

All tests run with 100 sockets averaged over 30 seconds using `tcp_load_test` example

- `monoio`: 148274.63
- `tokio-uring`: 122882.75
- `async-uring + tokio ST`: 86253.93
- `async-uring + tokio MT`: 81750.45
- `tokio MT`: 36173.95
- `tokio ST`: 6167.50
- `uSockets`: DNF (it segfaulted)
