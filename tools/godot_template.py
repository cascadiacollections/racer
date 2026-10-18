#!/usr/bin/env python3
"""racer's own Godot Android engine: Godot built from source without the
parts of the engine this game never uses (docs/godot-templates.md). Ported
from Salt Line's tools/godot_template.py. Standard library only, Python 3.9+.

    godot_template.py key
        Print the key: a hash of everything that decides what the build
        produces (Godot tag and commit, the SCons version, the build profile,
        the SCons flags, and the source of this file's build functions). The
        release holding a build is named godot-templates-<version>-<key>.
        CI workflow files are deliberately not part of it: bumping an action
        pin must not orphan the pinned engine (docs/godot-templates.md).

    godot_template.py build [--arch A] [--src DIR] [--out DIR] [-j N]
        Fetch the pinned Godot source (a shallow clone of the tag, checked
        against the pinned commit and refused if its tracked files are
        changed) and build the Android release engine into --out (default
        build/godot-templates). Runs the pinned SCons through uv; needs
        ANDROID_HOME (Godot installs the NDK it wants with sdkmanager).

    godot_template.py check
        Report whether tools/godot_templates.lock.json pins an engine for the
        current key. Drift and an unpinned lock are warnings (exports fall
        back to the official engine); a lock written from a local build
        (`lock --dir`) is an error, since nobody else can fetch it.

    godot_template.py prepare-android [--require]
        Unpack the official Gradle build template (android_source.zip) into
        android/build, then `engine`.

    godot_template.py engine [--require]
        Put the engine the lock pins into android/build's release godot-lib
        AAR (Gradle exports ignore custom_template/release, so the library is
        swapped inside the AAR the Gradle project links), after checking its
        sha256 against the lock and that the official engine's compiler built
        it (both share the template's libc++_shared.so). The engine comes
        from build/godot-templates when a file there matches the lock, else
        from the pinned release (gh CLI). When the lock does not match the
        current key, pins nothing, or STRIPPED_ENGINE=off is set, the AAR
        gets the official engine back and a loud warning is printed;
        --require makes that an error. Always restores the official entry
        first, so switching back never keeps a stale stripped library.

    godot_template.py lock [--dir DIR | --release]
        Rewrite the lock for the current key from the build in DIR (default
        build/godot-templates; marked "source": "local", for trying a build
        on a phone, never for a pull request), or with --release from the
        file the godot-templates workflow published for this key, after
        `gh attestation verify` proves that workflow built it on main.
"""

from __future__ import annotations

import argparse
import hashlib
import inspect
import json
import os
import re
import shutil
import subprocess
import sys
import tempfile
import zipfile
from pathlib import Path
from typing import NoReturn, Optional

ROOT = Path(__file__).resolve().parent.parent
PROFILE = ROOT / "tools" / "godot_build_profile.json"
LOCK = ROOT / "tools" / "godot_templates.lock.json"
DEFAULT_OUT = ROOT / "build" / "godot-templates"
ANDROID_BUILD = ROOT / "android" / "build"
REPO = "cascadiacollections/racer"

GODOT_VERSION = "4.7.2"
GODOT_TAG = f"{GODOT_VERSION}-stable"
# The commit the tag points at. A moved tag fails the build instead of
# quietly building something else.
GODOT_COMMIT = "ed1daf0bf001b61586d9930840f2f1394092c079"
GODOT_GIT = "https://github.com/godotengine/godot.git"
# SCons, pinned: a different SCons can build different bytes. Run through uv.
SCONS_VERSION = "4.11.1"
SCONS = ["uvx", "--from", f"scons=={SCONS_VERSION}", "scons"]
# Who may have built a release this lock trusts (lock --release).
SIGNER_WORKFLOW = f"{REPO}/.github/workflows/godot-templates.yml"
SIGNER_REF = "refs/heads/main"

# How it is compiled; the build profile holds what is left out. `lto` and
# `production` must be on the command line: SConstruct sets them after it
# reads the profile. Swappy paces Vulkan frames; this build has no Vulkan.
FLAGS = [
    "platform=android",
    "swappy=no",
    "target=template_release",
    "production=yes",
    "lto=full",
    "optimize=size_extra",
    "debug_symbols=no",
]
ARCH = "arm64"
# export_presets.cfg, Android: arm64-v8a only.
ANDROID_ABI = "arm64-v8a"
ENGINE = f"libgodot.android.template_release.{ARCH}.so"


def fail(msg: str) -> NoReturn:
    print(f"error: {msg}", file=sys.stderr)
    sys.exit(1)


def warn(msg: str) -> None:
    # GitHub turns this into an annotation; locally it is a banner, since
    # this is the line that says a 71 MB engine is about to ship.
    if os.environ.get("GITHUB_ACTIONS"):
        print(f"::warning::{msg}", file=sys.stderr)
    else:
        bar = "!" * 72
        print(f"\n{bar}\nWARNING: {msg}\n{bar}\n", file=sys.stderr)


def sha256(path: Path) -> str:
    h = hashlib.sha256()
    with path.open("rb") as f:
        for chunk in iter(lambda: f.read(1 << 20), b""):
            h.update(chunk)
    return h.hexdigest()


def scons_args(arch: str) -> list[str]:
    return [*FLAGS, f"arch={arch}"]


def key() -> str:
    h = hashlib.sha256()
    h.update(f"{GODOT_TAG}\n{GODOT_COMMIT}\nscons {SCONS_VERSION}\n".encode())
    # The code that builds, so a change to it moves the key.
    for fn in RECIPE:
        h.update(inspect.getsource(fn).encode())
    h.update(" ".join(scons_args(ARCH)).encode())
    # Canonical JSON, so reformatting the profile does not force a rebuild.
    profile = json.loads(PROFILE.read_text(encoding="utf-8"))
    h.update(json.dumps(profile, sort_keys=True, separators=(",", ":")).encode())
    return h.hexdigest()[:16]


def release_tag(k: str) -> str:
    return f"godot-templates-{GODOT_VERSION}-{k}"


def run(cmd: list[str], cwd: Optional[Path] = None) -> None:
    print("+ " + " ".join(cmd), flush=True)
    subprocess.run(cmd, cwd=cwd, check=True)


def fetch_source(src: Path) -> None:
    if not (src / ".git").exists():
        src.parent.mkdir(parents=True, exist_ok=True)
        run(["git", "clone", "--depth", "1", "--branch", GODOT_TAG, GODOT_GIT, str(src)])
    head = subprocess.run(
        ["git", "rev-parse", "HEAD"], cwd=src, check=True, capture_output=True, text=True
    ).stdout.strip()
    if head != GODOT_COMMIT:
        fail(f"{src} is at {head}, not {GODOT_TAG} ({GODOT_COMMIT})")
    dirty = subprocess.run(
        ["git", "status", "--porcelain", "--untracked-files=no"],
        cwd=src, check=True, capture_output=True, text=True,
    ).stdout.strip()
    if dirty:
        fail(f"{src} has local changes to tracked files:\n{dirty}")


def check_profile(src: Path) -> None:
    """Godot applies a build profile's options without checking their names,
    so an option renamed or removed in a new Godot would be silently ignored.
    Fail instead when this source tree does not know one."""
    profile = json.loads(PROFILE.read_text(encoding="utf-8"))
    known = (src / "SConstruct").read_text(encoding="utf-8")
    for detect in (src / "platform").glob("*/detect.py"):
        known += detect.read_text(encoding="utf-8")
    unknown = []
    for option in profile.get("disabled_build_options", {}):
        m = re.fullmatch(r"module_(\w+)_enabled", option)
        if m:
            if not (src / "modules" / m.group(1) / "config.py").is_file():
                unknown.append(option)
        elif f'"{option}"' not in known:
            unknown.append(option)
    if unknown:
        fail(f"{PROFILE.name} sets options Godot {GODOT_TAG} does not have: {', '.join(unknown)}")


def build(arch: str, src: Path, out: Path, jobs: int) -> None:
    fetch_source(src)
    check_profile(src)
    run([*SCONS, f"-j{jobs}", *scons_args(arch), f"build_profile={PROFILE}"], cwd=src)
    # Android builds move the library into the Gradle project's libs.
    abi = {"arm64": "arm64-v8a", "x86_64": "x86_64"}.get(arch, arch)
    built = src / "platform" / "android" / "java" / "lib" / "libs" / "release" / abi / "libgodot_android.so"
    if not built.is_file():
        fail(f"SCons finished but {built} is missing")
    out.mkdir(parents=True, exist_ok=True)
    dest = out / f"libgodot.android.template_release.{arch}.so"
    shutil.copy2(built, dest)
    print(f"{dest}  {dest.stat().st_size} bytes  sha256 {sha256(dest)}")


def elf_comment(data: bytes) -> list[str]:
    """The strings in an ELF64 little-endian file's .comment section: the
    compiler and linker versions that built it."""
    if data[:6] != b"\x7fELF\x02\x01":
        fail("not a 64-bit little-endian ELF file")
    shoff = int.from_bytes(data[0x28:0x30], "little")
    shentsize = int.from_bytes(data[0x3A:0x3C], "little")
    shnum = int.from_bytes(data[0x3C:0x3E], "little")
    shstrndx = int.from_bytes(data[0x3E:0x40], "little")

    def section(i: int) -> tuple[int, int, int]:
        base = shoff + i * shentsize
        name = int.from_bytes(data[base : base + 4], "little")
        off = int.from_bytes(data[base + 0x18 : base + 0x20], "little")
        size = int.from_bytes(data[base + 0x20 : base + 0x28], "little")
        return name, off, size

    _, stroff, _ = section(shstrndx)
    for i in range(shnum):
        name, off, size = section(i)
        end = data.index(b"\0", stroff + name)
        if data[stroff + name : end] == b".comment":
            return [x.decode("utf-8", "replace") for x in data[off : off + size].split(b"\0") if x]
    return []


def check_compiler(engine: bytes, official: bytes) -> None:
    """The stripped engine and the official one must come from the same
    compiler: they load the same libc++_shared.so from the template."""
    ours, theirs = elf_comment(engine), elf_comment(official)
    if not ours or not theirs or ours[0] != theirs[0]:
        fail(
            "the engine was not built by the compiler the official template was "
            f"(ours: {ours[:1]}, official: {theirs[:1]}); build with the NDK Godot "
            "asks for (platform/android/detect.py, get_ndk_version)"
        )
    print(f"compiler matches the official engine: {ours[0]}")


def official_templates() -> Path:
    """Where the Godot editor keeps the official templates for this version."""
    if "GODOT_TEMPLATES" in os.environ:
        return Path(os.environ["GODOT_TEMPLATES"])
    if sys.platform == "darwin":
        base = Path.home() / "Library" / "Application Support" / "Godot"
    elif sys.platform == "win32":
        base = Path(os.environ["APPDATA"]) / "Godot"
    else:
        base = Path(os.environ.get("XDG_DATA_HOME", Path.home() / ".local" / "share")) / "godot"
    return base / "export_templates" / f"{GODOT_VERSION}.stable"


def source_zip() -> Path:
    source = official_templates() / "android_source.zip"
    if not source.is_file():
        fail(f"missing {source}; install Godot's {GODOT_VERSION} export templates")
    return source


AAR = Path("libs") / "release" / "godot-lib.template_release.aar"
AAR_ENTRY = f"jni/{ANDROID_ABI}/libgodot_android.so"


def prepare_android(require: bool) -> None:
    """Install the official Gradle project, then choose its engine."""
    source = source_zip()
    if ANDROID_BUILD.exists():
        shutil.rmtree(ANDROID_BUILD)
    ANDROID_BUILD.mkdir(parents=True)
    with zipfile.ZipFile(source) as archive:
        for info in archive.infolist():
            dest = (ANDROID_BUILD / info.filename).resolve()
            if ANDROID_BUILD.resolve() not in dest.parents and dest != ANDROID_BUILD.resolve():
                fail("Android source template contains an unsafe path")
        archive.extractall(ANDROID_BUILD)
    (ANDROID_BUILD / ".gdignore").touch()
    (ANDROID_BUILD.parent / ".build_version").write_text(f"{GODOT_VERSION}.stable\n")
    (ANDROID_BUILD / "gradlew").chmod(0o755)
    print(f"Android Gradle template unpacked: {ANDROID_BUILD}")
    select_engine(require)


def read_lock() -> dict:
    try:
        return json.loads(LOCK.read_text(encoding="utf-8"))
    except FileNotFoundError:
        return {}


def lock_status() -> tuple[Optional[str], dict]:
    """(why the lock cannot be used, or None; the lock)."""
    lock = read_lock()
    k = key()
    if not lock.get("files", {}).get(ENGINE):
        return f"tools/godot_templates.lock.json pins no {ENGINE}", lock
    if lock.get("key") != k:
        return (
            f"tools/godot_templates.lock.json is for key {lock.get('key')}, the build inputs are {k} "
            "(run the godot-templates workflow on main, then `godot_template.py lock --release`)"
        ), lock
    return None, lock


def pinned_engine(lock: dict) -> Path:
    """The pinned engine, verified against the lock: the local build if it
    matches, else downloaded from the pinned release."""
    want = lock["files"][ENGINE]
    local = DEFAULT_OUT / ENGINE
    if local.is_file() and sha256(local) == want:
        print(f"{local}  sha256 {want}  (matches the lock)")
        return local
    if lock.get("source") == "local":
        fail(f"the lock was written from a local build and {local} no longer matches it")
    DEFAULT_OUT.mkdir(parents=True, exist_ok=True)
    with tempfile.TemporaryDirectory() as tmp:
        cmd = ["gh", "release", "download", lock["release"], "--repo", REPO,
               "--dir", tmp, "--pattern", ENGINE]
        print("+ " + " ".join(cmd), flush=True)
        if subprocess.run(cmd, check=False).returncode != 0:
            # The lock is current, so this build is meant to ship it.
            fail(f"could not download {ENGINE} from {lock['release']}")
        got = Path(tmp) / ENGINE
        digest = sha256(got)
        if digest != want:
            fail(f"{ENGINE} from {lock['release']} has sha256 {digest}, the lock says {want}")
        shutil.move(str(got), local)
    print(f"{local}  sha256 {want}  (downloaded, matches the lock)")
    return local


def write_aar(aar: Path, native: bytes) -> None:
    with tempfile.NamedTemporaryFile(dir=aar.parent, suffix=".aar", delete=False) as temp:
        staged = Path(temp.name)
    try:
        with zipfile.ZipFile(aar) as original, zipfile.ZipFile(staged, "w") as output:
            for info in original.infolist():
                data = native if info.filename == AAR_ENTRY else original.read(info)
                output.writestr(info, data, compress_type=info.compress_type)
    except BaseException:
        staged.unlink(missing_ok=True)
        raise
    staged.replace(aar)


def select_engine(require: bool) -> None:
    aar = ANDROID_BUILD / AAR
    if not aar.is_file():
        fail(f"no {aar}; run `just android-template` first")
    # The official engine, from the source zip, every time: switching back
    # to it must never leave a stripped library in place.
    with zipfile.ZipFile(source_zip()) as src:
        official_aar = src.read(AAR.as_posix())
    aar.write_bytes(official_aar)

    def official(reason: str) -> None:
        if require:
            fail(reason)
        warn(
            f"{reason}. The release APK will carry the OFFICIAL Godot engine "
            "(~71 MB uncompressed), not racer's stripped one (docs/godot-templates.md)."
        )

    if os.environ.get("STRIPPED_ENGINE") == "off":
        official("STRIPPED_ENGINE=off")
        return
    reason, lock = lock_status()
    if reason:
        official(reason)
        return
    engine = pinned_engine(lock)
    native = engine.read_bytes()
    with zipfile.ZipFile(aar) as original:
        check_compiler(native, original.read(AAR_ENTRY))
    write_aar(aar, native)
    print(f"{aar.relative_to(ROOT)}: stripped engine, {len(native)} bytes "
          f"(release exports only; debug exports keep the official engine)")


def check() -> None:
    reason, lock = lock_status()
    if lock.get("source") == "local":
        fail("tools/godot_templates.lock.json was written from a local build (`lock --dir`); "
             "pin a published release with `lock --release` instead")
    if reason:
        warn(f"{reason}; Android release exports fall back to the official Godot engine")
    else:
        print(f"lock pins {lock['release']} for the current key {lock['key']}")


def lock_from_release() -> None:
    k = key()
    with tempfile.TemporaryDirectory() as tmp:
        cmd = ["gh", "release", "download", release_tag(k), "--repo", REPO,
               "--dir", tmp, "--pattern", ENGINE]
        print("+ " + " ".join(cmd), flush=True)
        if subprocess.run(cmd, check=False).returncode != 0:
            fail(f"no release {release_tag(k)}; run the godot-templates workflow on main first")
        f = Path(tmp) / ENGINE
        # Built by the godot-templates workflow, from main, in this repo.
        cmd = [
            "gh", "attestation", "verify", str(f), "--repo", REPO,
            "--signer-workflow", SIGNER_WORKFLOW, "--source-ref", SIGNER_REF,
        ]
        print("+ " + " ".join(cmd), flush=True)
        if subprocess.run(cmd, check=False).returncode != 0:
            fail(f"{ENGINE} in {release_tag(k)} has no valid attestation from {SIGNER_WORKFLOW} on {SIGNER_REF}")
        write_lock(f, local=False)


def write_lock(engine: Path, local: bool) -> None:
    if not engine.is_file():
        fail(f"no {engine}; run `just godot-template` first")
    k = key()
    lock = {
        "godot": GODOT_TAG,
        "commit": GODOT_COMMIT,
        "key": k,
        "release": release_tag(k),
        "files": {ENGINE: sha256(engine)},
    }
    if local:
        lock["source"] = "local"
    LOCK.write_text(json.dumps(lock, indent=2) + "\n", encoding="utf-8")
    print(f"wrote {LOCK.relative_to(ROOT)} for {lock['release']}"
          + (" (local build: do not commit)" if local else ""))


# What `key` hashes the source of. Defined after them; read at call time.
RECIPE = (scons_args, fetch_source, check_profile, build, elf_comment, check_compiler, write_aar)


def main() -> None:
    ap = argparse.ArgumentParser(description=__doc__, formatter_class=argparse.RawDescriptionHelpFormatter)
    sub = ap.add_subparsers(dest="cmd", required=True)
    sub.add_parser("key")
    sub.add_parser("check")
    b = sub.add_parser("build")
    b.add_argument("--arch", default=ARCH)
    b.add_argument("--src", type=Path, default=Path.home() / ".cache" / "racer" / f"godot-{GODOT_TAG}")
    b.add_argument("--out", type=Path, default=DEFAULT_OUT)
    b.add_argument("-j", "--jobs", type=int, default=os.cpu_count() or 4)
    for name in ("prepare-android", "engine"):
        p = sub.add_parser(name)
        p.add_argument("--require", action="store_true")
    lk = sub.add_parser("lock")
    lk_src = lk.add_mutually_exclusive_group()
    lk_src.add_argument("--dir", type=Path, default=DEFAULT_OUT)
    lk_src.add_argument("--release", action="store_true")
    args = ap.parse_args()

    if args.cmd == "key":
        print(key())
    elif args.cmd == "check":
        check()
    elif args.cmd == "build":
        build(args.arch, args.src.resolve(), args.out.resolve(), args.jobs)
    elif args.cmd == "prepare-android":
        prepare_android(args.require)
    elif args.cmd == "engine":
        select_engine(args.require)
    elif args.cmd == "lock":
        if args.release:
            lock_from_release()
        else:
            write_lock(args.dir.resolve() / ENGINE, local=True)


if __name__ == "__main__":
    main()
