# Benchmarks
Units are 16kb packets per second.

All tests are run with `bash bench.sh client st 127.0.0.1:8080 100 16384 30`.
Run on Intel i7-1360P @ 5.0GHz.

- `async-uring + tokio ST`: 279986.71
    - `bash bench.sh async_uring_server st 127.0.0.1:8080`
- `monoio`: 277725.78
    - `bash bench.sh monoio_server 127.0.0.1:8080`
- `tokio-uring`: 264343.25
    - `bash bench.sh tokio_uring_server 127.0.0.1:8080`
- `async-uring + tokio MT`: 261538.80
    - `bash bench.sh async_uring_server mt 127.0.0.1:8080`
- `tokio MT`: 254768.04
    - `bash bench.sh tokio_server mt 127.0.0.1:8080`
- `tokio ST`: 251787.68
    - `bash bench.sh tokio_server st 127.0.0.1:8080`
