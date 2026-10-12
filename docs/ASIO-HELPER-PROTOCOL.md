# ASIO helper protocol (version 1)

`linktrix_asio_host.exe` (GPL-3.0-only, `crates/linktrix-asio-host`) hosts one
hardware ASIO driver in its own process. Any program can drive it with the
protocol below; Linktrix Tools starts one helper per selected ASIO device. The
shared-memory layout is defined in the MIT crate `linktrix-bus` (`bridge.rs`).

## Start

```
linktrix_asio_host <key> <driver CLSID | simulated> <frames> <primary|secondary>
```

- `key`: names the shared objects `Local\Linktrix.AsioBridge.<key>.Memory`,
  `.Request` and `.Done`. The caller creates them (`Bridge::create`) before
  starting the helper and uses a new key for every start.
- `frames`: requested buffer size. `secondary` lets the helper fall back to the
  driver's preferred size when the request is not supported.
- `simulated`: a timer-driven stand-in device (2 inputs, 2 outputs) for
  measuring the exchange without hardware.

## Control (text lines, UTF-8)

Helper → caller on stdout, caller → helper on stdin.

| Direction | Line | Meaning |
|---|---|---|
| ← | `ready <inputs> <outputs> <frames> <input latency> <output latency>` | Driver loaded at 48 kHz, buffers created. Channels are capped at 64; latencies in frames, -1 if not reported |
| ← | `error <text>` | Loading or a command failed |
| → | `start` | Start the driver. Answer: `started` or `error <text>` |
| → | `panel` | Open the driver's control panel. Answer: `panel ok` or `panel error <text>` (a modal panel answers when closed) |
| → | `quit` (or end of stdin) | Stop the driver, release it and exit |
| ← | `reset` | The driver requested a reset, resync or latency change, or changed rate |

## Audio exchange (shared memory)

Samples are signed 24-bit values in `i32`, interleaved with a stride of 64
channels (`[frame * 64 + channel]`). Each driver callback is one block with a
sequence number `seq` (1, 2, …), using slot `seq % 2` of the input and output
arrays:

1. Helper: writes the driver's inputs into `input[seq % 2]`, stores the
   callback's start time in `entry_ns`, publishes `request = seq` (release) and
   signals `Request`.
2. Caller: on `Request`, reads `request` (acquire), processes
   `input[seq % 2]` into `output[seq % 2]`, publishes `done = seq` (release)
   and signals `Done`.
3. Helper: waits for `done >= seq` until one buffer period after the callback
   started. In time: writes `output[seq % 2]` to the driver and stores
   `round_trip_ns`. Late: plays silence and increments `timeouts`.

`pin` (caller → helper): processor for the driver's callback thread as
`(group << 16) | number`, or `0xFFFFFFFF` for none. Times come from
QueryPerformanceCounter, so both processes can compare them.
