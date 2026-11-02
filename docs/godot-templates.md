# The stripped Godot engine

racer draws every pixel in Rust (`crates/racer-godot`) and hands Godot one
texture a frame and the input events. The official export templates carry
the whole engine anyway: 3D, physics, navigation, XR, Vulkan, the text
shaping stack, every image and audio codec. On Android that is a 71 MB
`libgodot_android.so`, and racer's Gradle export stores it uncompressed (so
Android can map it straight from the APK), which is most of a 76 MB APK.

So the Android release APK ships a Godot engine built from the same source
tag without those parts. Everything else in the APK (the Java side, the
manifest, the device_power plugin, the resources) is the official Gradle
template of the same Godot version; only the arm64 `libgodot_android.so` in
the template's release `godot-lib` AAR is replaced.

| Release APK (arm64, 4.7.2, `release profiling`) | Official engine | Stripped engine |
| --- | ---: | ---: |
| APK | 76,048,830 B | 27,847,206 B (−63%) |
| `libgodot_android.so` (stored uncompressed) | 71,114,944 B | 22,912,848 B (−68%) |
| Installed on a Pixel 10 (`extractNativeLibs=false`) | 73 MB (`du`) | 27 MB (`du`) |

Because the Gradle template stores native libraries uncompressed and does
not extract them, the installed size tracks the APK. Measured October 18,
2026 from a local build (`just godot-template`, SCons 4.11.1, NDK
29.0.14206865, 5m47s on 16 threads). On the phone, a release profiling
build on the stripped engine ran DEMO at 120 fps on USB and 60 fps with
`dumpsys battery unplug`, with `Device power: backend=android_plugin`, the
engine class check passing and no engine errors in logcat.

This is Salt Line's pipeline (its `docs/godot-templates.md`), ported: the
same build profile with one more module left out, the same build recipe,
lock file, attested prerelease and startup class check.

## What is left out, and why it is safe

`tools/godot_build_profile.json` is a Godot build profile: the SCons options
it sets are applied as if passed on the command line. `tools/godot_template.py`
adds how it is compiled: `platform=android swappy=no target=template_release
production=yes lto=full optimize=size_extra debug_symbols=no`.

| Left out | Why racer does not need it |
| --- | --- |
| `disable_3d`, `disable_physics_2d/3d`, `disable_navigation_2d/3d`, `disable_xr` | The pseudo-3D road is a software raster; one `Node2D` draws one texture. `racer-sim` does its own physics. |
| `disable_advanced_gui` | No `Control` nodes; every button and HUD element is rastered in Rust. |
| `vulkan`, `d3d12`, `metal` | `rendering_method` is `gl_compatibility` everywhere (project.godot). |
| `module_text_server_*`, `module_freetype`, `module_msdfgen`, `brotli` | No Godot text is drawn (`font.rs` is a bitmap font). Godot falls back to its built-in dummy text server. |
| `module_webp`, `module_svg`, `jpg`, `tga`, `bmp`, `hdr`, `dds`, `ktx`, `tinyexr`, `basis_universal`, `astcenc`, `etcpak`, `bcdec`, `betsy`, `cvtt` | racer imports no image at all: its one texture is made at runtime (`Image::create_from_data`). PNG (the boot splash) is core. |
| `ogg`, `vorbis`, `mp3`, `theora`, `interactive_music` | No audio yet; see below. |
| `gltf`, `fbx`, `csg`, `gridmap`, `meshoptimizer`, `xatlas_unwrap`, `vhacd`, `lightmapper_rd`, `raycast`, `noise`, `visual_shader`, `navigation_*`, `godot_physics_*`, `jolt_physics`, `openxr`, `mobile_vr`, `webxr`, `camera` | 3D, physics, XR and editor-side tooling. |
| `enet`, `multiplayer`, `webrtc`, `websocket`, `upnp`, `jsonrpc`, `mbedtls`, `regex`, `zip`, `minizip` | No networking, no TLS, no regex, no zip packs. Core's own hashing still builds its few mbedtls files. |
| `objectdb_profiler`, `mono`, `accesskit`, `glslang` | Debugging, C#, screen readers, Vulkan shader compilation. |

Kept: GDScript (`godot/platform_services.gd` and the device_power addon run
on every platform), and everything in core and the 2D scene tree.

**Differences from Salt Line's profile.** One: racer also leaves out
`module_webp`. Salt Line keeps it because its sprites are imported lossless,
and Godot's lossless texture import stores WebP inside the `.ctex` (unless
`rendering/textures/lossless_compression/force_png` is set). racer imports no
textures, so nothing decodes WebP. The first imported image (a title card, a
sprite sheet) needs `module_webp` back, or `force_png` on.

**Audio.** racer plays nothing yet. The audio server, `AudioStreamPlayer` and
`AudioStreamWAV` are core and stay whatever the profile says (a build profile
can only drop them with `disabled_classes`, which this one leaves empty). So
the first sound effect works on this engine as long as it is a WAV, as every
Salt Line sound is; OGG Vorbis or MP3 music needs `module_ogg` +
`module_vorbis` or `module_mp3` back in the profile. Keeping the profile
identical to Salt Line's here is deliberate: the two games then need the
same engine, apart from WebP.

**Android plugin v2 (device_power).** The Kotlin plugin is Java side and
untouched (the AAR's `classes.jar` is the official one). Its native face,
`JNISingleton`, and the fallback `JavaClassWrapper` live in
`platform/android/api`, which no profile option removes; Salt Line ships the
same addon on the same profile and reports `backend=android_plugin`. Its
`EditorExportPlugin` is editor-only and never in a template.

Also kept, deliberately: `deprecated`. SConstruct reads that option before it
applies a build profile, so a profile cannot turn it off.

Godot applies a profile's options without checking their names: an option a
future Godot renames would be silently ignored. `godot_template.py build`
checks every option in the profile against the source tree it is about to
build and refuses an unknown one.

### gdext and missing classes

`crates/racer-godot/Cargo.toml` enables gdext's `lazy-function-tables`: an
engine method is looked up on its first call instead of every method of every
class at load, which would panic on the first 3D class this engine lacks. The
costs: the table is not thread-safe, so no engine call may be made off the
main thread (`builder.rs`'s worker makes none), and a call into a removed
class would panic only when made.

That last one is covered at startup instead. `src/engine_classes.rs` lists
every engine class the game uses, and the library checks them all with
`ClassDB.class_exists` when the Scene stage initialises, failing at once with
`this Godot engine build lacks classes racer uses: ...` in `adb logcat`. A
unit test reads every `godot::classes::` path in the crate and fails if one
names a class that is not on the list. `JNISingleton` and `JavaClassWrapper`
are not on it: platform APIs register after extensions initialise, so the
`Device power: backend=android_plugin` log line is their check.

## Adding something back

1. Add the class to `REQUIRED` in `crates/racer-godot/src/engine_classes.rs`
   (the unit test insists, for anything imported from `godot::classes`).
2. Find what provides it: a module under `modules/` in the Godot source, or
   one of the `disable_*` switches (the class's source directory tells you).
3. Delete that line from `tools/godot_build_profile.json`. The profile is
   canonicalised before hashing, so formatting does not matter, but any real
   change gives a new key.
4. `just godot-template`, `python3 tools/godot_template.py lock --dir`, then
   `just android release` to try it on a phone. Restore the lock afterwards
   (`git checkout tools/godot_templates.lock.json`): a local lock is refused
   by CI.
5. Merge; the workflow publishes the build for the new key; pin it (below).

## Building it

```sh
just godot-template                          # build/godot-templates/libgodot.android.template_release.arm64.so
python3 tools/godot_template.py lock --dir   # pin that local build (do not commit)
just android-template                        # official Gradle template + the pinned engine
just android release                         # release APK on it, installed
```

`godot_template.py build` clones the pinned tag into
`~/.cache/racer/godot-4.7.2-stable` (checking the commit it resolves to and
refusing a tree with changed tracked files) and runs the pinned SCons there
through uv (`uvx --from scons==4.11.1 scons`). It needs `ANDROID_HOME`;
Godot wants NDK 29.0.14206865 (`platform/android/detect.py`) and installs it
with `sdkmanager` when missing. The engine must come from the same compiler
as the official one, since both load the template's `libc++_shared.so`;
`engine` refuses one that does not.

## Choosing the engine for an export

`just android-template` unpacks `android_source.zip` and then runs
`godot_template.py engine`, which `just android-apk release` also runs
before every release export:

- It first restores the official engine into
  `android/build/libs/release/godot-lib.template_release.aar`, so switching
  back never keeps a stale stripped library.
- If `tools/godot_templates.lock.json` pins an engine for the current key, it
  takes `build/godot-templates/<engine>` when its sha256 matches the lock,
  else downloads the pinned release with `gh` and checks the sha256; any
  mismatch or download failure is an error.
- If the lock is for another key (the profile or recipe changed and nobody
  has pinned the new build) or pins nothing, it keeps the official engine and
  prints a banner warning. `--require` makes that an error.
- `STRIPPED_ENGINE=off` forces the official engine, with the same warning.

Debug exports always use the official debug engine (`godot-lib.template_debug`);
only release templates are built.

## How CI uses it

1. **Key.** `godot_template.py key` hashes the Godot tag and commit, the
   pinned SCons version, the build profile, the SCons flags and the source of
   the script's build functions. Workflow files are deliberately left out:
   Salt Line's key included its workflow, so a routine action-pin bump moved
   the key, CI fell back to the official engine and a 71 MB engine shipped
   without anyone noticing. Here the workflow only calls
   `godot_template.py build`; what it builds is decided by the hashed inputs.
2. **Build and publish.** `.github/workflows/godot-templates.yml` runs when
   one of those inputs changes on `main`, or by hand. If no
   `godot-templates-<version>-<key>` release exists yet, it builds Android
   arm64 in a job with a read-only token. Only on `main`, a separate job (the
   only one with write, OIDC and attestation permissions, and no build code)
   attests the file's provenance and publishes it as that prerelease. A
   published key is never rebuilt or overwritten.
3. **Pin it.** `python3 tools/godot_template.py lock --release` downloads the
   published file, runs `gh attestation verify` on it (signer
   `cascadiacollections/racer/.github/workflows/godot-templates.yml`, source
   ref `refs/heads/main`), and writes the lock. Try it on a phone
   (`just android-template`, `just android release`), then open a pull
   request with the lock file.
4. **Check.** The `ci.yml` hygiene job runs `godot_template.py check`: a lock
   that does not match the current key, or pins nothing, is a warning
   annotation on every run; a lock written from a local build fails.

5. **Export.** The `ci.yml` `android` job runs `godot_template.py
   prepare-android` after installing the templates, with `--require` on
   pushes to main (the builds `dev release` publishes), and honours a
   `vars.STRIPPED_ENGINE` off switch as Salt Line's `ci.yml` does.

## Bumping Godot

1. Change `GODOT_VERSION` and `GODOT_COMMIT` in `tools/godot_template.py`
   (`git ls-remote https://github.com/godotengine/godot
   refs/tags/<version>-stable`) along with the editor and templates.
2. New modules default to enabled; check the new version's `modules/` list
   and `SConstruct` options for anything worth adding to the profile.
3. Build and test locally, merge, let the workflow publish, pin it. Until
   then release exports fall back to the official engine, with the warning.

Trying the 4.8 beta 2 editor without moving `GODOT_VERSION` is
[docs/godot-48-trial.md](godot-48-trial.md). That path does not rewrite this
pin or `tools/godot_templates.lock.json`.
