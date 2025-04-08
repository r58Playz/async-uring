CPU="$1"
shift
EXAMPLE="$1"
shift

echo 1 | sudo tee /sys/devices/system/cpu/intel_pstate/no_turbo
taskset -c "$CPU" bash -c "RUSTFLAGS='-C target-cpu=native -C force-frame-pointers' cargo ${COMMAND:-r} -r --example $EXAMPLE $*"
