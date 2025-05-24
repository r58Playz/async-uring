# Benchmarks
Units are 16kb packets per second.

All tests are run with `bash bench.sh client st 127.0.0.1:8080 100 16384 30`.

- `async-uring + tokio ST`: 167059.81
    - `bash bench.sh async_uring_server st 127.0.0.1:8080`
- `async-uring + tokio MT`: 165319.33
    - `bash bench.sh async_uring_server mt 127.0.0.1:8080`
- `monoio`: 159179.67
    - `bash bench.sh monoio_server 127.0.0.1:8080`
- `tokio-uring`: 155767.05
    - `bash bench.sh tokio_uring_server 127.0.0.1:8080`
- `tokio MT`: 111296.24
    - `bash bench.sh tokio_server mt 127.0.0.1:8080`
- `tokio ST`: 5165.71
    - `bash bench.sh tokio_server st 127.0.0.1:8080`
- [`uSockets`](https://github.com/uNetworking/uSockets/blob/master/examples/echo_server.c): DNF (it segfaulted)
