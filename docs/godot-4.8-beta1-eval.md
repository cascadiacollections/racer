# Godot 4.8 beta 1 evaluation

2026-10-19, on the eval branch `eval/godot-4.8-beta1` only. The owner decided not to switch
engines: production stays on 4.7.2 (engine and stripped template together), and nothing here
is meant to merge.

The 4.8 beta 1 editor and templates are the official builds from godotengine/godot-builds, tag
`4.8-beta1` (`4.8.beta1.official.4d10419ca`). Their SHA512 matches `SHA512-SUMS.txt`. They are
installed next to 4.7.2.

The branch carries these eval-only changes:

- The Android package is `com.cascadiacollections.racer.eval48`, in both the preset and the
  Justfile's `package`.
- `GODOT_BUILD_VERSION` overrides `android/.build_version`. The existing `GODOT_TEMPLATES`
  points at `export_templates/4.8.beta1`.

## Sizes (release export, arm64-v8a, Gradle; Pixel 10 57220DLCR001SH)

| Build | APK | libgodot_android.so | Installed (`du -sk` of the app dir) |
|---|---:|---:|---:|
| 4.7.2 official | 76,678,255 | 71,114,944 | 74,963 KB |
| 4.7.2 stripped (lock `ae96ebe146c2ac61`, shipping) | 28,476,527 | 22,912,848 | 27,839 KB |
| 4.8 beta 1 official | 71,091,045 | 65,524,792 | 69,499 KB |

## Frame time (release + `profiling`, DEMO, 16 two-second windows, USB power)

| Engine | raster mean | process mean (worst p95) | frame mean (worst p95) | fps |
|---|---:|---:|---:|---:|
| 4.7.2 stripped | 0.812 ms | 2.174 ms (3.82) | 8.33 ms (8.33) | 120.0 |
| 4.8 beta 1 official | 0.786 ms | 2.165 ms (3.59) | 8.33 ms (8.33) | 120.0 |

The two engines are at parity: both are vsync-bound at 120 Hz. The raw lines are in
`docs/eval48/`.

## Findings

1. **gdext**: the `api-4-7` build (gdext 0.5.5) loads on 4.8, on desktop (headless) and on the
   device. The log reads `API v4.7.stable.official, runtime v4.8.beta1.official, safeguards
   strict`, with no gdext warnings.
   - gdext 0.5.5 has no `api-4-8` feature. An `api-custom` build against the 4.8 binary
     compiles with no warnings and runs on 4.8.
   - 4.7.2 refuses that build ("compiled against newer Godot version"), as expected. Not
     committed.
2. **Touch on the Pixel (4.8)**: all of these work:
   - START from the title;
   - holding the left pad (the car moves left);
   - sliding from the left pad to the right pad (the pad swap);
   - holding BRAKE (160 to 58 km/h).

   Multi-touch (a pad held with DRIFT) was not exercised: `adb input` sends only one pointer.
3. **Saves**: `user://` resolves to the same `app_userdata/racer` directory on both engines.
   - A cold start on each engine writes a byte-identical `tracks/` store.
   - Ghosts, layout slots and draft, `daily.txt` and run files are plain Rust `std::fs` I/O on
     `globalize_path` paths, so they do not depend on the engine. They were not round-tripped
     one by one: there is no headless autoplay.
   - On the device, the eval package has its own data.
4. **godot-device-power AAR** (built against `org.godotengine:godot:4.7.2.stable`) in the 4.8
   Gradle export: `DevicePowerAndroid` registers, the log shows `backend=android_plugin`, and
   the frame rate vote of 120 returns ok (rates 60/120). This matches 4.7.2.
5. **Editor-managed Android/Java SDK**:
   - 4.8 seeds `export/android/java_sdk_path` from `JAVA_HOME` only when the key is missing.
     Otherwise it defaults to an editor-managed `<data>/godot/java-sdk-17`.
   - An existing `editor_settings-4.8.tres` with an empty value fails export: "Invalid Java SDK
     path".
   - The Justfile exports `JAVA_HOME`, which only helps with fresh editor settings.
   - The Gradle template moves AGP 8.6.1 to 8.13.2 and Gradle 8.11.1 to 8.14.5, and adds the
     `dataLib`, `fusedDepsLib` and `assetPackInstallTime` modules. `prepare-android` and the
     engine swap still work on its layout.
6. **Stripped engine**: not built for 4.8. `tools/godot_build_profile.json` would fail
   `check_profile` on 4.8: `module_regex_enabled` is gone, because RegEx moved to core. 4.8
   adds the `tilemap`, `texture_streaming` and `visionos_xr` modules, all left enabled.
7. **Other**:
   - The 4.8 import and export rewrote no tracked file (`project.godot` unchanged) and logged
     no deprecations.
   - "No project icon specified" appears on both engines.
   - The 4.8 export logs a harmless `EditorSettings not instantiated ... shutdown_adb_on_exit`
     at exit.
   - On Linux desktop, 4.8 requests GameMode for the whole process by default
     (`application/run/use_game_mode`).
   - The sim, and so `just android-digest`, does not touch the engine.

## Risks

- Beta SDK-management code.
- The build profile breaks.
- Engine-wide GameMode is on by default.
- The Gradle and AGP bump.
- gdext has no `api-4-8` until a gdext release.

## Recommendation

Stay on 4.7.2. Revisit at 4.8 RC1 or stable, with:

- a gdext release that has `api-4-8`;
- the build profile updated (drop `module_regex_enabled`, consider `tilemap` and
  `texture_streaming`) and a stripped 4.8 engine measured against the 22.9 MB 4.7.2 one;
- an explicit `java_sdk_path`.
