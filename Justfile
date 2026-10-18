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

# Check advisories, licences, bans and sources (deny.toml). Needs cargo-deny.
deny:
    cargo deny check

# ------------------------------------------------------------------ Android
#
# Official Godot 4.7.2 templates, no Gradle, arm64 only. Always the SDK's
# adb: mixing it with a system adb kills the server.

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
# Gradle exports need (and which bundles the device_power Kotlin plugin).
# Re-run after a Godot upgrade; android/build is not tracked.
android-template:
    #!/usr/bin/env bash
    set -euo pipefail
    src="${GODOT_TEMPLATES:-$HOME/.local/share/godot/export_templates/4.7.2.stable}/android_source.zip"
    [ -f "$src" ] || { echo "missing $src: install Godot's 4.7.2 export templates" >&2; exit 1; }
    rm -rf android/build && mkdir -p android/build
    unzip -q "$src" -d android/build
    touch android/build/.gdignore
    echo "4.7.2.stable" > android/.build_version
    chmod +x android/build/gradlew

# Export the APK to build/racer.apk: a debug export, or with
# profile=release a release export signed with the same debug key. A Gradle
# build: run `just android-template` once first.
android-apk profile="debug" features="": build (android-lib profile features)
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
    bash tools/godot_headless.sh --export-{{ profile }} "Android" build/racer.apk
    ls -l build/racer.apk

# Build, install and launch on the attached device.
android profile="debug" features="": (android-apk profile features)
    "{{ adb }}" install -r -d build/racer.apk
    "{{ adb }}" shell am force-stop {{ package }}
    "{{ adb }}" shell input keyevent KEYCODE_WAKEUP
    "{{ adb }}" shell monkey -p {{ package }} -c android.intent.category.LAUNCHER 1 >/dev/null 2>&1

# Follow the game's log on the device.
android-log:
    "{{ adb }}" logcat --pid="$("{{ adb }}" shell pidof {{ package }})"

# Save a device screenshot to target/screens/<name>.png.
screen name:
    mkdir -p target/screens && touch target/screens/.gdignore
    "{{ adb }}" exec-out screencap -p > target/screens/{{ name }}.png
