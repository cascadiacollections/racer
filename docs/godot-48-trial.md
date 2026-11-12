# Godot 4.8 beta 3 trial

Godot **4.7.2** stays the editor, the CI export, and the stripped Android
engine. This path does not change `GODOT_VERSION`, `project.godot`'s feature
list, `racer.gdextension`'s `compatibility_minimum`, the `api-4-7` feature, or
`tools/godot_templates.lock.json`.

4.8 beta 3 (9 November 2026, commit `c51ff06e8836a70ea905701c081bf94124dcd6ce`)
is a preview with matching export templates. It is not a migration.

## What gdext can target

The locked crate is `godot` 0.5.5 (`gdextension-api` 0.5.1). `tools/godot_48_trial.py report`
prints the features Cargo actually resolved. As of this trial:

- Pregenerated API levels are `api-4-2` through `api-4-7`. There is no `api-4-8`.
- `api-custom` generates bindings from a Godot binary (`GDRUST_GODOT_BIN`;
  `GODOT4_BIN` is the deprecated alias). `api-custom-json` reads
  `GDRUST_GODOT_API_JSON`. When the binary's minor is newer than the prebuilt
  header (4.7 in this release), godot-bindings 0.5.5 dumps
  `gdextension_interface.json` from the binary instead of using the prebuilt one.
- gdext loads only when the runtime is at least the API it was compiled for.
  The committed `api-4-7` library is the one that can run on 4.7.2 and on 4.8.
  A library generated from the 4.8 editor would refuse to load on 4.7.2, so
  `run` only typechecks that probe in `target/godot-48-api-custom` and restores
  `Cargo.toml` afterwards.

`run` records whether that probe typechecks. The shipped build stays
`api-4-7` either way: a library generated from the 4.8 editor refuses to
load on 4.7.2. On this run the probe typechecked, and the `api-4-7`
extension loaded under the 4.8-beta2 editor (`racer: sim epoch 1`). Beta3 re-run pending.

## Run it

```sh
just godot-48-trial
# or
python3 tools/godot_48_trial.py run
python3 tools/godot_48_trial.py build-engine --dry-run
```

`run` downloads the official Linux editors and export templates for 4.8-beta3
and, as a size baseline only, 4.7.2. It checks SHA-512 against the
godot-builds `SHA512-SUMS.txt` published with each release, then checks:

- editor `--version` (includes the build and the commit, for example
  `4.8.beta3.official.c51ff06e8`)
- engine commit (that same 9-character prefix)
- template build key: `templates/version.txt`, the `GODOT_VERSION_FULL_CONFIG`
  string the editor uses as the export-templates directory name
  (`4.8.beta3`, `4.7.2.stable`). The editor version must start with it.
- packaged native libraries in the official templates, including arm64
  `libgodot_android.so`
- the Rust libraries the export would ship (`linux` debug and release, and
  `android.release.arm64` when an NDK is on `PATH` via `ANDROID_NDK_HOME` or
  `ANDROID_NDK_LATEST_HOME`)
- a Linux release export of this project on each editor, same `api-4-7` extension

Downloads go to `~/.cache/racer/godot-trial`. The report is
`build/godot-48-trial/report.txt`. Nothing is written into the 4.7.2 template
directory or the stripped-engine lock.

CI runs the same `run` only on request, in the opt-in **godot-4.8-trial**
workflow (`.github/workflows/godot-4.8-trial.yml`, `workflow_dispatch` only):

```
gh workflow run godot-4.8-trial.yml -R cascadiacollections/racer
gh workflow run godot-4.8-trial.yml -R cascadiacollections/racer -f custom_engine=true
```

Pull requests and pushes to main never run it, so it costs nothing until
someone dispatches it. Its `compare` job uploads `report.txt` as the
`godot-48-trial-report` artifact. `custom_engine=true` adds an allow-failure
`build-engine` job that uploads `godot-48-trial-engine`. In `ci.yml`,
`python3 tools/godot_48_trial.py self-test` is the blocking check: it asserts
the stable pin, that Python 3.9 does not pass the engine-build floor, and that
the trial workflow stays dispatch-only and out of `ci.yml`.

Headless, 4.8 (seen on beta2) prints `WARNING: GameMode: Failed to enable` when the
desktop GameMode daemon is not installed. 4.7.2 did not. racer does not use
that daemon. The trial records the line and still requires a zero exit, the
sim-epoch log, and no other warning or error. Both seasons did that on this run.

## Custom engine builds

`build-engine` is the only command that compiles Godot. Beta 2's `SConstruct`
calls `EnsurePythonVersion(3, 10)` (GH-122597). The command checks the
interpreter against 3.10 before it clones, then checks the requirement in the
tree it checked out. The stable `just godot-template` command is unchanged
and still builds 4.7.2.

The clone is `~/.cache/racer/godot-4.8-beta3` (not the 4.7.2 cache). SCons is
the same pin as the stable recipe, with `GODOT_VERSION_STATUS=beta3` so a
custom binary is recognized as this beta. The product is
`build/godot-48-trial/engine/`. It is not locked, attested, or published.
There is no Android SDK on a typical desk, so this command is for a machine
that already builds the stripped 4.7.2 engine.

SCons gets a trial profile written next to the clone, not
`tools/godot_build_profile.json`. That file stays the 4.7.2 profile, and the
4.7.2 lock key hashes it. The trial copy changes two things:

- **Drops `module_regex_enabled`.** RegEx moved into core in 4.8, so the
  option no longer exists.
- **Sets `rendering_device=false`.** 4.8 adds this option, on by default, and
  defines `RD_ENABLED` from it rather than from `vulkan`. With only
  `vulkan=false`, `display_server_android.cpp` calls
  `free_vulkan_global_context()`, which is declared only under
  `VULKAN_ENABLED`, and the Android template fails to compile. Salt Line hit
  this first (cascadiacollections/saltline#154).

Every option in the trial profile is checked against the fetched tree. A drop
the tree still has is refused as stale. `self-test` covers both checks.

## Device checklist

Upstream fixes to confirm on a phone, not defects already seen in racer.
4.7.2 remains the build to compare against. Use a release export signed like
the one already on the device, or uninstall first.

1. **ARM64 launch.** Install the arm64 export and cold-start it. The log
   should show the sim epoch and the engine class check, with no extension
   load error. Confirm `lib/arm64-v8a/` contains both `libgodot_android.so`
   and `libracer_godot.so` (the Rust library is the `api-4-7` build; 4.8 does
   not get its own bindings until gdext ships them).
2. **Touch, including clipped and off-screen presses.** racer's pads are
   rastered and hit-tested in Rust (`touch.rs`), not Godot `Button`s. On a
   `canvas_items` / `keep` layout, press a pad, then press the black
   letterbox and just off the visible game rect: the letterbox must not
   steer or fire a pad. GH-123008 (`BaseButton` checks the visible viewport
   rect; issue 121492) is the engine fix for Control nodes under those bars.
   This project has no `Control` buttons, so the pad check is the game's own
   hit test; the BaseButton fix still wants a look if a Godot control is
   added later.
3. **Multiply blend.** GH-124838 fixes multiply blending that was too dark in
   the Compatibility renderer. racer copies sprites 1:1 in its own raster, so
   the game frame should look the same as 4.7.2. On Android (Compatibility),
   and on iOS launched with `--rendering-method gl_compatibility`
   (`just ios-launch`), confirm a multiply-blended canvas sprite or effect is
   not darker than the same scene on 4.7.2. iOS's default renderer is Mobile
   (Metal); that path is not the one GH-124838 changed.
4. **Save and resume.** Start a real run (not DEMO), quit to the title or
   leave mid-race so the run is written, force-stop, and launch again. The
   run file under the app's `runs` directory must still be there
   (`user://runs`, which on Android is the external files dir
   `just pull-runs` reads).

## Sizes

Measured from the official template packs (SHA-512 checked) and from a Linux
release export of this tree. The Rust library is the committed `api-4-7`
build, so it is the same file on both editors. This is not the stripped
Android APK in `docs/godot-templates.md`; that engine is still 4.7.2 only,
and a 4.8 game APK was not exported here (no Android SDK).

| | 4.7.2 | 4.8-beta2 | delta |
| --- | ---: | ---: | ---: |
| `libgodot_android.so` arm64, release AAR | 71,114,944 | 64,818,536 | −6,296,408 |
| `linux_release.x86_64` template | 73,519,416 | 76,655,160 | +3,135,744 |
| `linux_release.arm64` template | 67,079,432 | 69,036,184 | +1,956,752 |
| Linux export `racer.x86_64` | 73,952,752 | 77,088,496 | +3,135,744 |
| Linux export plus `libracer_godot.so` | 77,523,048 | 80,658,792 | +3,135,744 |

The arm64 engine library in the 4.7.2 release AAR matches the 71,114,944 byte
figure already in `docs/godot-templates.md`. The 4.8-beta2 copy is smaller.
The Linux export grew by exactly the template binary's delta; `libracer_godot.so`
is 3,570,296 bytes (ELF64 x86_64) in both. The arm64 Rust library, built with
NDK r27c and not yet inside an APK, is `target/aarch64-linux-android/release/libracer_godot.so`,
3,932,216 bytes, ELF64 aarch64.

`android_release.apk` in the template pack (every ABI, not a racer export)
is 104,803,333 bytes on 4.7.2 and 102,428,389 on 4.8-beta2. `ios.zip` is
202,976,946 and 194,810,931. No Android or iOS game package was produced.
