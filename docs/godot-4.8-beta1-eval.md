# Godot 4.8 beta 1 evaluation (WIP)

Eval branch only. Production stays on 4.7.2 (owner decision: no engine switch).

| Build | APK bytes | libgodot_android.so | Installed (du, KB) |
|---|---|---|---|
| 4.7.2 official release | 76,678,255 | 71,114,944 | 74,963 |
| 4.7.2 stripped (lock ae96ebe1) | 28,476,527 | 22,912,848 | 27,839 |
| 4.8 beta1 official release | 71,091,045 | 65,524,792 | 69,499 |

Profiling (release + profiling, DEMO, ~32 s, Pixel 10 on USB power):
- 4.7.2 stripped: raster 0.812 ms, process 2.174 ms (worst p95 3.82), frame 8.33 ms, 120.0 fps
- 4.8 beta1 official: raster 0.786 ms, process 2.165 ms (worst p95 3.59), frame 8.33 ms, 120.0 fps
