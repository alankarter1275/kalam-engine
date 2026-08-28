# iOS

The iOS ladder lives in `docs/FFI.md`, *The iOS spike*. What is here:

| | what it is |
|---|---|
| `spike/` | Rungs 2-4: the C ABI hand-driven from Swift, on a simulator |
| `spike/rung5/` | Rung 5: a minimal app — picker, bookmark, cold resolve |

`spike/run.sh` builds `libchapbook_ffi.a` for the simulator and device
targets, compiles `main.swift` against the checked-in `chapbook.h`
through a module map — no copy of the header, no cbindgen, no generated
binding — runs the simulator binary in a booted simulator, and
link-checks the device slice it cannot run. Findings go back into
`docs/FFI.md`, which is the spike's real output.
