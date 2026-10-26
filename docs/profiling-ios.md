# Profiling on iOS

The low-end iOS floor is the **iPhone SE (2nd generation)** (A2275): A13 Bionic, 3 GB RAM, a 4.7" 750 × 1334 @2x display with a home button and no notch, on iOS 27. If a build holds frame time there, it holds on every iPhone racer supports. The Android equivalent is the Galaxy A16 (NOTES.md, "Direct to screen and the A16 profile").

In portrait the SE is exactly 9:16, so it gets the shortest buffer: 180 × 320. It has no gesture area and no home indicator, so nothing is reserved at the bottom (`layout::bottom_inset_px`; Android's 32 dp gesture allowance applies on Android only). `layout::tests::an_iphone_se_gets_the_shortest_buffer_and_no_inset` checks the SE layout.

## One-time device setup

1. **Register the device.** Plug the SE into the Mac, open Xcode, and trust the computer on the phone. `just ios` builds with `-allowProvisioningUpdates`, so Xcode registers the UDID with team `9L45FNA7XM` on the first signed build. To register it by hand instead, copy the identifier from Xcode > Window > Devices and Simulators and add it under Certificates, Identifiers & Profiles > Devices on developer.apple.com.
2. **Turn on Developer Mode** on the phone (Settings > Privacy & Security > Developer Mode), then restart it. The option only appears once Xcode has seen the device.
3. **Name the phone for the recipes:** `export IOS_DEVICE="iPhone SE"`, or use its UDID from `xcrun devicectl list devices`.

You also need Godot 4.7.2 with its export templates, Xcode signed in to the team, and rustup (`docs/testflight.md`).

## Builds

| Build | Command | Use it for |
| --- | --- | --- |
| Debug | `just ios` | The perf overlay, and Instruments with symbols. Rust is unoptimised, so don't trust its frame times. |
| Release | `just ios release` | Benchmarks and Game Performance traces. No overlay. |

Both builds export through `ios-project`. The iOS preset is arm64-only, iPhone and iPad, with `min_ios_version` 16.0. The Rust library is built for `aarch64-apple-ios` with the same deployment target (`ios-lib`, `tools/testflight.py`), and `racer.gdextension` maps `ios.debug` / `ios.release` to `target/aarch64-apple-ios/{debug,release}`.

## Renderer

iOS runs the **Mobile** renderer (Metal), set by `rendering/renderer/rendering_method.ios` in `project.godot`. Android and desktop keep **Compatibility**. If Metal fails to start, `rendering_device/fallback_to_opengl3` drops to Compatibility automatically.

To try Compatibility without rebuilding:

```sh
just ios-launch --rendering-method gl_compatibility
```

To ship it, set the `.ios` line to `"gl_compatibility"`. Run the benchmark on both renderers before choosing; the SE has only 3 GB. The Android direct-to-screen path (`Racer::direct_to_screen`) stays Android-only.

## Perf overlay (debug builds)

A debug build shows the overlay from launch on a phone, top left over the sky:

```text
59.8 FPS  16.72 ms  p95 17.40  p99 21.02
DRAW 7  STATIC 31.5M  VRAM 14.2M
THERMAL NOMINAL  HEADROOM 1210M
```

What each value is:

- **Frame time:** wall clock between frames; mean, p95 and p99 over the last 240 frames.
- **DRAW, STATIC, VRAM:** Godot's `Performance` monitors for the frame the renderer finished last.
- **THERMAL:** `NSProcessInfo.thermalState`.
- **HEADROOM:** `os_proc_available_memory()`, how much the process can still allocate before jetsam kills it. The simulator has no limit, so this field is hidden there.

The thermal and headroom readings come from `perf::device`, a small shim that calls the Objective-C runtime directly. There is no plugin to export. `--racer-perf` shows the overlay on a desktop too. Release builds compile none of it.

## Soak benchmark

```sh
just ios release            # optimised build, installed
just ios-bench              # 300 s on the Mobile renderer
just ios-bench 300 gl_compatibility
just ios-bench-pull         # CSVs into build/bench/
```

`--racer-bench` hands the car to the demo bot on seed `20261004`. With `Game::fixed_forks` on, the bot picks each fork from the seed and the track index rather than the clock, so every benchmark races the same tracks. When a run ends, the seed starts again. The sim steps at a fixed 120 Hz, but the bot's input is held across the ticks in each frame, so its exact line, and the camera that follows it, can drift slightly with frame pacing. The tracks and scenery do not drift.

Every frame is kept in memory and written once at the end, to `Documents/bench/bench-<unix time>-done.csv`. If the app is backgrounded or closed early, the suffix is `-paused` or `-closed`. Then the app exits.

The log line `racer-bench: ...` gives the mean, p95 and p99 frame time after a 2 s warm-up, the worst thermal state, and the lowest headroom. The CSV columns are:

```text
frame,t_s,frame_ms,process_ms,rendered,draw_calls,static_mib,video_mib,thermal,available_mib
```

`process_ms` is the node's own `process`: input, tick, raster and upload. `rendered` is 0 on frames the dirty flag skipped.

For a soak:

- Unplug the phone, fix the brightness, and take it out of its case.
- Run three benchmarks back to back.
- Compare p99 and the thermal state across the three runs, not just the mean.

`DevicePower` lowers the frame cap under thermal pressure. So if the thermal state rises and the frame rate falls with it, that is the design working, not a regression.

`--features profiling` with `--racer-profile-seed` still prints the two-second raster and upload breakdown, as before.

## Instruments

Use a **release** build for timing and a **debug** build for Rust symbols.

1. Run `just ios release`. In Instruments, choose the SE as the target, then attach to *Racer* or let Instruments launch it.
2. **Game Performance** template: frame pacing (hitches over 16.7 ms), thermal state, and CPU time on the main thread versus Godot's render thread. For a repeatable trace, start a benchmark from Xcode's scheme by adding `--racer-bench --racer-bench-seconds 120` under Arguments Passed On Launch.
3. **Metal System Trace** template: GPU time per frame, encoder and command-buffer timing, and drawable waits. racer draws one textured quad a frame. If frames run long, look first at the raster (`screens::frame`) and the upload (`Image::set_data` and `ImageTexture::update`).
4. Record 30–60 s past the warm-up.

Xcode's debug gauges (Debug Navigator > FPS, Memory, Energy) are good for a quick look. They add overhead, so don't compare their numbers with release benchmarks.

## What needs the device

Only the physical SE can cover these:

- registering the device and signing
- Developer Mode
- Instruments traces
- real thermal states and memory headroom
