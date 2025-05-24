BIN="$1"
shift

if [[ "$TURBO" = "0" ]]; then
	echo 1 | sudo tee /sys/devices/system/cpu/intel_pstate/no_turbo
fi

RUSTFLAGS='-C target-cpu=native -C force-frame-pointers' cargo ${COMMAND:-r} -r --bin $BIN $*
