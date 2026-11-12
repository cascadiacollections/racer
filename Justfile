default:
    @just --list

# rustup's cargo and rustc, called explicitly: a Homebrew or system Rust
# earlier on PATH has neither the pinned toolchain nor an Android std.
cargo := 'RUSTC="$(rustup which rustc)" "$(rustup which cargo)"'

# Build the GDExtension (debug) that the editor and `run` load.
build:
    @mkdir -p target/trackviz && touch target/trackviz/.gdignore
    {{ cargo }} build -p racer-godot

# Build, import, and play on the desktop: a portrait window, letterboxed
# when wider than it is tall.
run *args: build
    bash tools/godot_headless.sh --frame-delay 800 --import
    godot --path . {{ args }}

# Play the free shareware edition (crates/racer-godot/src/tier.rs): runs end after 2 tracks; no TRACKS/editor.
run-shareware: build
    bash tools/godot_headless.sh --frame-delay 800 --import
    godot --path . -- --racer-shareware

# Android Shareware is a release export signed with the debug key, as android-apk does (run `just android-template` once first).
# Export the shareware edition: "Linux" or "Android". Writes build/shareware/.
export-shareware platform="Linux":
    #!/usr/bin/env bash
    set -euo pipefail
    case "{{ platform }}" in
        Linux) out=racer-shareware.x86_64; {{ cargo }} build --release -p racer-godot ;;
        Android) exec just android-apk release "" "Android Shareware" ;;
        *) echo "error: no shareware preset for '{{ platform }}'" >&2; exit 2 ;;
    esac
    mkdir -p build/shareware
    bash tools/godot_headless.sh --frame-delay 800 --import
    bash tools/godot_headless.sh --export-release "{{ platform }} Shareware" "build/shareware/$out"
    # --export-release has been known to exit 0 having written nothing.
    test -e "build/shareware/$out" || { echo "error: export wrote nothing" >&2; exit 1; }
    echo "build/shareware/$out"

# Unit, property, snapshot and golden-run tests.
test:
    cargo test --workspace

fmt:
    cargo fmt --all -- --check

lint:
    cargo clippy --workspace --all-targets -- -D warnings

# Generate tracks across many run seeds, validate each with the bots and print
# the solvability gate report (DESIGN §2.4). Exit status 1 if any gate fails.
balance seeds="200":
    cargo run --release -p racer-sim --bin batch -- --seeds {{ seeds }}

# Cross-platform determinism digest (DESIGN §6): 200 noisy bot runs, each
# replayed from its log. CI compares the digest across platforms.
digest:
    cargo run --release -p racer-sim --bin determinism

# Look at one generated track: `just trackviz 7 2 1` prints a strip chart of
# run seed 7, track index 2, branch 1; add `--png out.png` for a picture.
trackviz seed="1" index="0" branch="0" *flags:
    cargo run --release -p racer-sim --bin trackviz -- --seed {{ seed }} --index {{ index }} --branch {{ branch }} {{ flags }}

# Layout tools (DESIGN §9): `just layout export --seed 7 --index 2 --branch 1`
# (a draft code and its origin), `just layout prove course.txt` (the gate
# report and the proven share code), `just layout show RL…`,
# `just layout check --verify a.txt b.txt`, `just layout survey --seeds 50`.
layout *args:
    cargo run --release -q -p racer-sim --bin layout -- {{ args }}

# List the bundled courses and prove each again to the timers it carries.
layout-courses:
    cargo run --release -q -p racer-sim --bin layout -- bundled

# Regenerate the sounds and title music in art/audio (tools/gen_audio.py,
# standard library only, byte-for-byte reproducible). Commit the result.
gen-audio:
    python3 tools/gen_audio.py

# Fail unless art/audio is exactly what tools/gen_audio.py makes (CI runs it).
check-audio:
    python3 tools/gen_audio.py --check

# Check advisories, licences, bans and sources (deny.toml). Needs cargo-deny.
deny:
    cargo deny check

# ------------------------------------------------------------------ Android
#
# Godot 4.7.2's Gradle build template, arm64 only; release exports carry
# racer's stripped engine when tools/godot_templates.lock.json pins one
# (docs/godot-templates.md). Always the SDK's adb: mixing it with a system
# adb kills the server.

android_sdk := env("ANDROID_HOME", home_directory() / "Android/Sdk")
android_ndk := env("ANDROID_NDK_HOME", `ls -d "${ANDROID_HOME:-$HOME/Android/Sdk}"/ndk/* 2>/dev/null | sort -V | tail -1`)
android_host := if os() == "macos" { "darwin-x86_64" } else { "linux-x86_64" }
android_bin := android_ndk / "toolchains/llvm/prebuilt" / android_host / "bin"
adb := android_sdk / "platform-tools/adb"
java_home := env("JAVA_HOME", home_directory() / ".local/share/mise/installs/java/temurin-21.0.12+101.0.LTS")
keystore := env("GODOT_ANDROID_KEYSTORE_DEBUG_PATH", if path_exists(home_directory() / ".android/debug.keystore") == "true" { home_directory() / ".android/debug.keystore" } else { home_directory() / ".local/share/godot/keystores/debug.keystore" })
package := "com.cascadiacollections.racer"

# Cross-compile the GDExtension for Android (arm64) and strip its debug
# info. `just android-lib release profiling` builds the timing build.
android-lib profile="debug" features="":
    rustup target add aarch64-linux-android
    {{ cargo }} build -p racer-godot --target aarch64-linux-android \
        {{ if profile == "release" { "--release" } else { "" } }} \
        {{ if features != "" { "--features " + features } else { "" } }} \
        --config 'target.aarch64-linux-android.linker="{{ android_bin }}/aarch64-linux-android24-clang"'
    "{{ android_bin }}/llvm-strip" --strip-debug target/aarch64-linux-android/{{ profile }}/libracer_godot.so

# Unpack Godot's official Android build template into android/build, which
# Gradle exports need (and which bundles the device_power Kotlin plugin), and
# put the stripped engine the lock pins into its release AAR. Without a
# usable pin it keeps the official engine and prints a loud warning;
# STRIPPED_ENGINE=off forces the official one. Re-run after a Godot upgrade;
# android/build is not tracked.
android-template:
    python3 tools/godot_template.py prepare-android

# Build racer's stripped Godot engine (Android arm64) from source into
# build/godot-templates (docs/godot-templates.md). Needs uv (it runs the pinned
# SCons) and the Android SDK; about 3 minutes on 16 cores. `lock --dir` then
# pins it locally to try on a phone (never commit that lock).
godot-template *args:
    ANDROID_HOME="{{ android_sdk }}" python3 tools/godot_template.py build {{ args }}

# Opt-in Godot 4.8-beta3 trial. Does not change the 4.7.2 pin, the lock, or
# the api-4-7 feature (docs/godot-48-trial.md).
godot-48-trial *args:
    python3 tools/godot_48_trial.py run {{ args }}

# Export the APK to build/racer.apk: a debug export, or with
# profile=release a release export signed with the same debug key. A Gradle
# build: run `just android-template` once first. preset="Android Shareware"
# exports the shareware edition to build/shareware/racer-shareware.apk.
android-apk profile="debug" features="" preset="Android": build (android-lib profile features)
    #!/usr/bin/env bash
    set -euo pipefail
    mkdir -p build && touch build/.gdignore
    export JAVA_HOME="{{ java_home }}"
    export GODOT_ANDROID_KEYSTORE_DEBUG_PATH="{{ keystore }}"
    export GODOT_ANDROID_KEYSTORE_DEBUG_USER="${GODOT_ANDROID_KEYSTORE_DEBUG_USER:-androiddebugkey}"
    export GODOT_ANDROID_KEYSTORE_DEBUG_PASSWORD="${GODOT_ANDROID_KEYSTORE_DEBUG_PASSWORD:-android}"
    export GODOT_ANDROID_KEYSTORE_RELEASE_PATH="${GODOT_ANDROID_KEYSTORE_RELEASE_PATH:-{{ keystore }}}"
    export GODOT_ANDROID_KEYSTORE_RELEASE_USER="${GODOT_ANDROID_KEYSTORE_RELEASE_USER:-androiddebugkey}"
    export GODOT_ANDROID_KEYSTORE_RELEASE_PASSWORD="${GODOT_ANDROID_KEYSTORE_RELEASE_PASSWORD:-android}"
    bash tools/godot_headless.sh --frame-delay 800 --import
    [ -f android/.build_version ] || { echo "run 'just android-template' first" >&2; exit 1; }
    # Re-select the engine on every release export, so a lock change (or
    # STRIPPED_ENGINE=off) takes effect without re-unpacking the template.
    {{ if profile == "release" { "python3 tools/godot_template.py engine" } else { "" } }}
    apk={{ if preset == "Android" { "build/racer.apk" } else { "build/shareware/racer-shareware.apk" } }}
    mkdir -p "$(dirname "$apk")"
    bash tools/godot_headless.sh --export-{{ profile }} "{{ preset }}" "$apk"
    ls -l "$apk"
    unzip -lv "$apk" 'lib/*/libgodot_android.so' | tail -n +4 | head -n 1

# Build, install and launch on the attached device.
android profile="debug" features="": (android-apk profile features)
    "{{ adb }}" install -r -d build/racer.apk
    "{{ adb }}" shell am force-stop {{ package }}
    "{{ adb }}" shell input keyevent KEYCODE_WAKEUP
    "{{ adb }}" shell monkey -p {{ package }} -c android.intent.category.LAUNCHER 1 >/dev/null 2>&1

# Play the determinism corpus on the attached phone (arm64) and fail unless
# its digest equals this machine's: the sim must agree bit for bit across
# CPUs, not just across the CI OS matrix (DESIGN §6).
android-digest:
    #!/usr/bin/env bash
    set -euo pipefail
    rustup target add aarch64-linux-android
    {{ cargo }} build --release -q -p racer-sim --bin determinism \
        --target aarch64-linux-android \
        --config 'target.aarch64-linux-android.linker="{{ android_bin }}/aarch64-linux-android24-clang"'
    host=$({{ cargo }} run --release -q -p racer-sim --bin determinism | grep -oE 'digest: 0x[0-9a-f]+' | cut -d' ' -f2)
    echo "host ($(uname -sm)) digest: $host"
    ADB="{{ adb }}" bash tools/android_digest.sh target/aarch64-linux-android/release/determinism --expect "$host"

# Copy the runs the game kept on the attached phone to target/runs and
# compare them with the human-like bot (playstats; NOTES.md "Playtest
# capture"). The files stay on the phone.
pull-runs:
    mkdir -p target/runs && touch target/runs/.gdignore
    "{{ adb }}" pull /sdcard/Android/data/{{ package }}/files/Documents/runs/. target/runs/
    {{ cargo }} run --release -q -p racer-sim --bin playstats -- target/runs/run-*.txt

# Follow the game's log on the device.
android-log:
    "{{ adb }}" logcat --pid="$("{{ adb }}" shell pidof {{ package }})"

# Save a device screenshot to target/screens/<name>.png.
screen name:
    mkdir -p target/screens && touch target/screens/.gdignore
    "{{ adb }}" exec-out screencap -p > target/screens/{{ name }}.png

# Signed archive; add --upload to distribute to internal TestFlight testers.
testflight *args:
    python3 tools/testflight.py {{ args }}

# iOS (arm64), as in Salt Line. macOS only. Needs Xcode signed in to the developer account
# (Settings > Accounts), rustup, Godot 4.7.2's export templates, and the Team ID in the iOS
# preset (application/app_store_team_id; IOS_TEAM overrides it). xcodebuild signs with
# automatic provisioning, which also registers the phone; devicectl installs and launches.
# IOS_DEVICE picks the phone (a name or UDID; `xcrun devicectl list devices`).

# The iOS preset's application/min_ios_version; the library is built for the same floor.
ios_min := "16.0"

# Build the GDExtension for iOS.
ios-lib profile="debug":
    rustup target add aarch64-apple-ios
    IPHONEOS_DEPLOYMENT_TARGET={{ ios_min }} {{ cargo }} build -p racer-godot \
        --target aarch64-apple-ios {{ if profile == "release" { "--release" } else { "" } }}

# Export the Xcode project to build/ios (open build/ios/Racer.xcodeproj to run from Xcode).
ios-project profile="debug": build (ios-lib profile)
    #!/usr/bin/env bash
    set -euo pipefail
    rm -rf build/ios && mkdir -p build/ios
    bash tools/godot_headless.sh --frame-delay 800 --import
    bash tools/godot_headless.sh --export-{{ profile }} "iOS" build/ios/Racer.ipa
    test -d build/ios/Racer.xcodeproj
    # Godot emits empty usage strings for permissions racer never asks for.
    info=build/ios/Racer/Racer-Info.plist
    for key in NSCameraUsageDescription NSMicrophoneUsageDescription NSPhotoLibraryUsageDescription; do
        plutil -remove "$key" "$info" 2>/dev/null || true
    done
    # Godot skips a missing extension library without a word; the app then starts empty.
    if ! find build/ios -name 'libracer_godot.framework' | grep -q .; then
        echo "error: the export has no libracer_godot.framework; is target/aarch64-apple-ios built?" >&2
        exit 1
    fi

# Sign, install and launch on the iPhone. `just ios release` for the optimised build.
ios profile="debug": (ios-project profile)
    #!/usr/bin/env bash
    set -euo pipefail
    team="${IOS_TEAM:-$(sed -n 's/^application\/app_store_team_id="\(.*\)"$/\1/p' export_presets.cfg)}"
    device="${IOS_DEVICE:?set IOS_DEVICE to the iPhone name or UDID}"
    # Stage outside any file-provider folder: copied frameworks pick up FinderInfo there,
    # which codesign rejects.
    stage=$(mktemp -d "${TMPDIR:-/tmp}/racer-ios.XXXXXX")
    trap 'rm -rf "$stage"' EXIT
    mkdir -p "$stage/project" "$stage/out"
    rsync -a build/ios/ "$stage/project/"
    xattr -cr "$stage"
    config={{ if profile == "release" { "Release" } else { "Debug" } }}
    xcodebuild -project "$stage/project/Racer.xcodeproj" -configuration "$config" -sdk iphoneos \
        -allowProvisioningUpdates DEVELOPMENT_TEAM="$team" CODE_SIGN_STYLE=Automatic \
        COMPILER_INDEX_STORE_ENABLE=NO SYMROOT="$stage/out" -quiet build
    xcrun devicectl device install app --device "$device" "$(ls -d "$stage/out/$config-iphoneos"/*.app | head -1)"
    xcrun devicectl device process launch --device "$device" {{ package }}

# E.g. `just ios-launch --racer-perf` or `just ios-launch --rendering-method gl_compatibility`.
# Launch the installed app on the iPhone with developer flags, streaming its log.
ios-launch *args:
    xcrun devicectl device process launch --console --terminate-existing \
        --device "${IOS_DEVICE:?set IOS_DEVICE to the iPhone name or UDID}" {{ package }} {{ args }}

# renderer is mobile or gl_compatibility; the CSV lands in the app's Documents/bench.
# Soak benchmark on the iPhone (docs/profiling-ios.md): `just ios release` first, then this.
ios-bench seconds="300" renderer="mobile":
    just ios-launch --rendering-method {{ renderer }} --racer-bench --racer-bench-seconds {{ seconds }}

# Copy the benchmark CSVs off the iPhone into build/bench.
ios-bench-pull:
    mkdir -p build/bench
    xcrun devicectl device copy from --device "${IOS_DEVICE:?set IOS_DEVICE to the iPhone name or UDID}" \
        --domain-type appDataContainer --domain-identifier {{ package }} \
        --source Documents/bench --destination build/bench
