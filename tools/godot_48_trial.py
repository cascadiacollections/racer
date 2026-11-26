#!/usr/bin/env python3
"""Opt-in Godot 4.8-beta5 compatibility trial.

The stable pin stays Godot 4.7.2: editor, CI, export templates and the
stripped Android engine (tools/godot_template.py, tools/godot_templates.lock.json).
This script never writes that lock and never passes a different version to
those commands.

    godot_48_trial.py self-test
        Pin and Python-floor checks. No downloads.

    godot_48_trial.py report
        What the locked gdext crate can say about a 4.8 API.

    godot_48_trial.py fetch
        Download the official 4.8-beta5 Linux editor and export templates,
        and the 4.7.2 pair used only as a size baseline. SHA-512 is the
        digest published in that release's SHA512-SUMS.txt.

    godot_48_trial.py provenance
        Editor --version, engine commit, template build key (version.txt,
        which must equal the editor version), and the packaged native
        libraries inside the official templates.

    godot_48_trial.py run
        report, fetch, provenance, build the GDExtension as committed
        (api-4-7), headless import and the audio shutdown check, a Linux
        release export, and the same export on 4.7.2 for sizes. Then, in a
        throwaway target directory, try api-custom against the 4.8 editor.
        That probe is informational: there is no api-4-8 feature to switch
        the project to, and a 4.8-generated library would refuse to load
        on 4.7.2.

    godot_48_trial.py build-engine [--dry-run]
        Custom Android arm64 engine from the 4.8-beta5 commit, for a later
        device experiment. Refuses Python older than 3.10 before it clones
        anything. Does not publish, attest, or lock. Not used by CI.

Downloads land in ~/.cache/racer/godot-trial. Exports land in
build/godot-48-trial, which is gitignored with the rest of build/.
"""

from __future__ import annotations

import argparse
import hashlib
import json
import os
import re
import shutil
import stat
import subprocess
import sys
import tempfile
import zipfile
from pathlib import Path
from typing import NoReturn, Optional

ROOT = Path(__file__).resolve().parent.parent
CACHE = Path.home() / ".cache" / "racer" / "godot-trial"
OUT = ROOT / "build" / "godot-48-trial"
GODOT_GIT = "https://github.com/godotengine/godot.git"
BUILDS = "https://github.com/godotengine/godot-builds/releases/download"

# Stable pin this trial is required to leave alone. self-test reads the
# same strings out of the files CI and the editor actually use.
STABLE_VERSION = "4.7.2"
STABLE_TAG = "4.7.2-stable"
STABLE_COMMIT = "ed1daf0bf001b61586d9930840f2f1394092c079"

# Official 4.8 beta 5, 24 November 2026. The source tag is not on
# godotengine/godot; godot-builds names the commit in the release notes.
TRIAL_VERSION = "4.8-beta5"
TRIAL_COMMIT = "a661dfdbccc664ccca57ad76c2bb5dcd8f3fc768"
TRIAL_COMMIT_SHORT = TRIAL_COMMIT[:9]
# SConstruct on that commit calls EnsurePythonVersion(3, 10) (GH-122597).
# The stable 4.7.2 build script stays on its own 3.9+ note.
TRIAL_PYTHON = (3, 10)

# Trial-only edits to tools/godot_build_profile.json for the stripped 4.8
# engine. The shared profile is the 4.7.2 one and is never rewritten: a
# 4.7.2 build could not take these, and its lock key hashes the file.
#
# Dropped: options 4.8 no longer has. RegEx moved into core.
TRIAL_PROFILE_DROPS = ("module_regex_enabled",)
# Set: options 4.8 adds that the 4.7.2 profile cannot name. 4.8 defines
# RD_ENABLED from `rendering_device` rather than from `vulkan`, so with only
# vulkan=false display_server_android.cpp calls free_vulkan_global_context(),
# which is declared under VULKAN_ENABLED, and the Android template fails to
# compile (seen on beta3; same code on beta2 and master).
TRIAL_PROFILE_SETS: dict[str, object] = {"rendering_device": False}

# SHA-512 lines copied from SHA512-SUMS.txt on each godot-builds release.
RELEASES = {
    "4.8-beta5": {
        "tag": "4.8-beta5",
        "commit": TRIAL_COMMIT,
        "version_prefix": "4.8.beta5",
        "editor": "Godot_v4.8-beta5_linux.x86_64.zip",
        "editor_sha512": "37f81c4e3d6c0a9d8ab5304dfe24d32cbd31590cb847b10530466276eb1dac338cd653f8fe6005e1610906cd887ea804d875b85a63da10b3220085fad9ceab18",
        "templates": "Godot_v4.8-beta5_export_templates.tpz",
        "templates_sha512": "aecd46cee205a929fb5997f7d7b273b010edb4a09f66abde2f7072b24275683a57b235d6cf03bd050fb2db4063556433a4dd65658efa9f59d4a328d789fbbb25",
    },
    "4.7.2": {
        "tag": "4.7.2-stable",
        "commit": STABLE_COMMIT,
        "version_prefix": "4.7.2.stable",
        "editor": "Godot_v4.7.2-stable_linux.x86_64.zip",
        "editor_sha512": "9aa00f7a605200940bce3027a567b782f49bd8e940dd06ae9e987bd65aee1b1467edd56ed84fcdcbdd44354bf613bdbb4e5d2913e925850368e150c59ed54c65",
        "templates": "Godot_v4.7.2-stable_export_templates.tpz",
        "templates_sha512": "ca4d71c4d7b81dfc15d1a98baa07534aa95b03fdda78a0075b06672e1648d2e5f40980c9adc28d23e1b92e732ee7bf3461997aa804af74ec2fcd7a93ccb84079",
    },
}


def fail(msg: str) -> NoReturn:
    print(f"error: {msg}", file=sys.stderr)
    sys.exit(1)


def run(cmd: list[str], cwd: Optional[Path] = None, env: Optional[dict[str, str]] = None) -> None:
    print("+ " + " ".join(cmd), flush=True)
    subprocess.run(cmd, cwd=cwd, env=env, check=True)


def version_at_least(major: int, minor: int, minimum: tuple[int, int]) -> bool:
    return (major, minor) >= minimum


def require_trial_python() -> None:
    """Python floor for a custom 4.8 engine build. Not used by fetch/run."""
    got = sys.version_info
    if not version_at_least(got[0], got[1], TRIAL_PYTHON):
        fail(
            f"a Godot {TRIAL_VERSION} engine build needs Python >= {TRIAL_PYTHON[0]}.{TRIAL_PYTHON[1]} "
            f"(SConstruct EnsurePythonVersion on {TRIAL_COMMIT_SHORT}, GH-122597); "
            f"this interpreter is {got[0]}.{got[1]}.{got[2]}. "
            "tools/godot_template.py is unchanged and still builds 4.7.2."
        )


def sha512_file(path: Path) -> str:
    h = hashlib.sha512()
    with path.open("rb") as f:
        for chunk in iter(lambda: f.read(1 << 20), b""):
            h.update(chunk)
    return h.hexdigest()


def download(url: str, dest: Path, digest: str) -> None:
    if dest.is_file() and sha512_file(dest) == digest:
        print(f"{dest.name}  sha512 ok")
        return
    dest.parent.mkdir(parents=True, exist_ok=True)
    partial = dest.with_suffix(dest.suffix + ".partial")
    if partial.exists():
        partial.unlink()
    run(["curl", "-fL", "--retry", "3", "--retry-delay", "2", "-o", str(partial), url])
    got = sha512_file(partial)
    if got != digest:
        partial.unlink(missing_ok=True)
        fail(f"{dest.name} sha512 {got}, release sums say {digest}")
    partial.replace(dest)
    print(f"{dest.name}  {dest.stat().st_size} bytes  sha512 ok")


def release_urls(spec: dict[str, str]) -> tuple[str, str]:
    base = f"{BUILDS}/{spec['tag']}"
    return f"{base}/{spec['editor']}", f"{base}/{spec['templates']}"


def fetch_release(label: str) -> tuple[Path, Path]:
    spec = RELEASES[label]
    folder = CACHE / label
    editor_url, templates_url = release_urls(spec)
    editor = folder / spec["editor"]
    templates = folder / spec["templates"]
    download(editor_url, editor, spec["editor_sha512"])
    download(templates_url, templates, spec["templates_sha512"])
    return editor, templates


def read_zip_text(archive: Path, name: str) -> str:
    with zipfile.ZipFile(archive) as zf:
        try:
            return zf.read(name).decode("utf-8")
        except KeyError:
            fail(f"{archive.name} has no {name}")


def safe_members(zf: zipfile.ZipFile, dest: Path, names: list[str]) -> None:
    root = dest.resolve()
    for name in names:
        info = zf.getinfo(name)
        target = (dest / Path(name).name).resolve()
        if target != root and root not in target.parents:
            fail(f"unsafe path in archive: {name}")
        target.parent.mkdir(parents=True, exist_ok=True)
        with zf.open(info) as src, target.open("wb") as out:
            shutil.copyfileobj(src, out)


def extract_editor(archive: Path, dest: Path) -> Path:
    if dest.exists():
        shutil.rmtree(dest)
    dest.mkdir(parents=True)
    with zipfile.ZipFile(archive) as zf:
        names = [n for n in zf.namelist() if not n.endswith("/")]
        bins = [n for n in names if Path(n).name.startswith("Godot_v")]
        if len(bins) != 1:
            fail(f"{archive.name} editor entries: {bins}")
        safe_members(zf, dest, bins)
        binary = dest / Path(bins[0]).name
    binary.chmod(binary.stat().st_mode | stat.S_IEXEC)
    return binary


def editor_version(binary: Path) -> str:
    out = subprocess.run(
        [str(binary), "--version"], check=True, capture_output=True, text=True
    )
    line = next((ln.strip() for ln in out.stdout.splitlines() if ln.strip()), "")
    if not line:
        fail(f"{binary} --version printed nothing: {out.stderr}")
    return line


def isolated_env(home: Path, binary: Path) -> dict[str, str]:
    """Keep the trial off the machine's real Godot config and templates."""
    env = os.environ.copy()
    env["XDG_DATA_HOME"] = str(home / "share")
    env["XDG_CONFIG_HOME"] = str(home / "config")
    env["XDG_CACHE_HOME"] = str(home / "cache")
    env["GODOT_BIN"] = str(binary)
    return env


def install_linux_templates(tpz: Path, home: Path, version: str) -> Path:
    dest = home / "share" / "godot" / "export_templates" / version
    if dest.exists():
        shutil.rmtree(dest)
    dest.mkdir(parents=True)
    wanted = []
    with zipfile.ZipFile(tpz) as zf:
        for name in zf.namelist():
            base = Path(name).name
            if name == "templates/version.txt" or base in {
                "linux_release.x86_64",
                "linux_debug.x86_64",
            }:
                wanted.append(name)
        if "templates/version.txt" not in wanted or not any(
            n.endswith("linux_release.x86_64") for n in wanted
        ):
            fail(f"{tpz.name} is missing version.txt or linux_release.x86_64")
        root = dest.resolve()
        for name in wanted:
            target = (dest / Path(name).name).resolve()
            if root not in target.parents and target != root:
                fail(f"unsafe template path {name}")
            with zf.open(name) as src, target.open("wb") as out:
                shutil.copyfileobj(src, out)
            if "linux_" in target.name:
                target.chmod(target.stat().st_mode | stat.S_IEXEC)
    text = (dest / "version.txt").read_text(encoding="utf-8").strip()
    if text != version:
        fail(f"installed version.txt is {text}, editor is {version}")
    return dest


def elf_desc(header: bytes) -> str:
    if header[:4] != b"\x7fELF":
        return "not-elf"
    bits = {1: "32", 2: "64"}.get(header[4], "?")
    machine = int.from_bytes(header[18:20], "little")
    names = {183: "aarch64", 62: "x86_64", 3: "x86", 40: "arm"}
    return f"ELF{bits} {names.get(machine, hex(machine))}"


def template_native_libs(tpz: Path) -> list[str]:
    """Sizes of the official template's packaged engines, including arm64."""
    lines = []
    with zipfile.ZipFile(tpz) as zf:
        interesting = (
            "android_release.apk",
            "android_debug.apk",
            "linux_release.x86_64",
            "linux_release.arm64",
            "ios.zip",
            "android_source.zip",
        )
        for info in zf.infolist():
            base = Path(info.filename).name
            if base in interesting:
                lines.append(f"  {base}  {info.file_size} bytes (uncompressed in the tpz)")
        source_name = next(
            (i.filename for i in zf.infolist() if i.filename.endswith("android_source.zip")),
            None,
        )
    if source_name is None:
        lines.append("  android_source.zip missing")
        return lines
    # The nested zip is seekable only on disk. It holds the arm64 libgodot.
    with tempfile.TemporaryDirectory(prefix="godot-tpz-") as tmp:
        nested = Path(tmp) / "android_source.zip"
        with zipfile.ZipFile(tpz) as zf, nested.open("wb") as out:
            with zf.open(source_name) as src:
                shutil.copyfileobj(src, out)
        aar_path = Path(tmp) / "release.aar"
        with zipfile.ZipFile(nested) as inner:
            aar_name = next(
                (n for n in inner.namelist() if n.endswith("godot-lib.template_release.aar")),
                None,
            )
            if aar_name is None:
                lines.append("  release godot-lib AAR missing from android_source.zip")
                return lines
            with inner.open(aar_name) as src, aar_path.open("wb") as out:
                shutil.copyfileobj(src, out)
        # The engine .so is inside the AAR, not loose in the source zip.
        with zipfile.ZipFile(aar_path) as aar:
            so_name = next(
                (n for n in aar.namelist() if n.endswith("arm64-v8a/libgodot_android.so")),
                None,
            )
            if so_name is None:
                lines.append("  arm64 libgodot_android.so missing from the release AAR")
                return lines
            info = aar.getinfo(so_name)
            with aar.open(info) as raw:
                header = raw.read(64)
            lines.append(
                f"  {so_name}  {info.file_size} bytes  {elf_desc(header)}  (release AAR)"
            )
    return lines


def prepare(label: str) -> dict[str, object]:
    spec = RELEASES[label]
    editor_zip, tpz = fetch_release(label)
    # version.txt is the template build key and the export-templates directory
    # name (GODOT_VERSION_FULL_CONFIG, e.g. 4.7.2.stable or 4.8.beta5).
    # `godot --version` is longer: it appends the build and the commit
    # (4.7.2.stable.official.ed1daf0bf).
    version_txt = read_zip_text(tpz, "templates/version.txt").strip()
    home = CACHE / label / "home"
    binary = extract_editor(editor_zip, CACHE / label / "editor")
    reported = editor_version(binary)
    if version_txt != spec["version_prefix"]:
        fail(f"{label}: template build key is {version_txt}, expected {spec['version_prefix']}")
    if not (reported == version_txt or reported.startswith(version_txt + ".")):
        fail(f"{label}: editor --version {reported} does not match template build key {version_txt}")
    short = spec["commit"][:9]
    if short not in reported:
        fail(f"{label}: editor version {reported} does not name commit {short}")
    install_linux_templates(tpz, home, version_txt)
    print(f"{label}: editor {reported}; template build key {version_txt}")
    return {
        "label": label,
        "binary": binary,
        "home": home,
        "version": reported,
        "build_key": version_txt,
        "commit": spec["commit"],
        "tpz": tpz,
        "editor_zip": editor_zip,
    }


def godot_feature_report() -> list[str]:
    meta = json.loads(
        subprocess.check_output(
            ["cargo", "metadata", "--format-version", "1", "--locked"],
            cwd=ROOT,
            text=True,
        )
    )
    godot_pkg = next(p for p in meta["packages"] if p["name"] == "godot")
    api_pkg = next(p for p in meta["packages"] if p["name"] == "gdextension-api")
    declared = sorted(godot_pkg["features"])
    enabled: list[str] = []
    for node in meta["resolve"]["nodes"]:
        # Cargo 1.99 ids look like `...index#godot@0.5.5`; older ones start
        # with `godot 0.5.5`. `#godot@` does not match godot-core.
        ident = node["id"]
        if ident.startswith("godot ") or "#godot@" in ident:
            enabled = sorted(node.get("features", []))
            break
    api_levels = [name for name in declared if name.startswith("api-")]
    lines = [
        f"godot crate {godot_pkg['version']} (Cargo.lock), gdextension-api {api_pkg['version']}",
        "declared api features: " + ", ".join(api_levels),
        "features enabled for this dependency: " + ", ".join(enabled),
    ]
    if "api-4-8" in declared:
        lines.append(
            "api-4-8 is declared by this crate. The project still enables api-4-7; "
            "switching would be a pin change, which this trial does not do."
        )
    else:
        lines.append(
            "No api-4-8 feature. Pregenerated bindings in this gdext release stop at api-4-7 "
            "(gdextension-api's version modules are 4.2 through 4.7)."
        )
        lines.append(
            "api-custom generates bindings from a Godot binary (GDRUST_GODOT_BIN; "
            "GODOT4_BIN still works as a deprecated alias). api-custom-json reads "
            "GDRUST_GODOT_API_JSON plus, when the binary is newer than the prebuilt "
            "header, GDRUST_GODOT_INTERFACE_JSON. godot-bindings 0.5.5 dumps that "
            "header itself when the binary's minor is past the prebuilt JSON."
        )
        lines.append(
            "gdext loads when the runtime version is >= the API it was compiled for, "
            "and refuses a runtime older than that. An api-4-7 extension is the build "
            "that can run on 4.7.2 and on a newer 4.8. An api-custom build against 4.8 "
            "would refuse to load on 4.7.2, so it stays a probe."
        )
    if "api-4-7" not in enabled:
        fail("racer-godot is not building godot with api-4-7 enabled")
    if "api-custom" in enabled or "api-4-8" in enabled:
        fail("racer-godot resolved a 4.8-facing API feature; the stable features changed")
    return lines


def assert_stable_pin() -> None:
    checks = [
        (ROOT / "tools" / "godot_template.py", f'GODOT_VERSION = "{STABLE_VERSION}"'),
        (ROOT / "tools" / "godot_template.py", f'GODOT_COMMIT = "{STABLE_COMMIT}"'),
        (ROOT / "project.godot", 'PackedStringArray("4.7", "GL Compatibility")'),
        (ROOT / "racer.gdextension", "compatibility_minimum = 4.7"),
        (
            ROOT / "crates" / "racer-godot" / "Cargo.toml",
            'features = ["api-4-7", "lazy-function-tables"]',
        ),
        (ROOT / ".github" / "workflows" / "ci.yml", f"GODOT_VERSION: {STABLE_VERSION}"),
    ]
    for path, needle in checks:
        if needle not in path.read_text(encoding="utf-8"):
            fail(f"stable pin missing {needle} in {path.relative_to(ROOT)}")
    lock = json.loads((ROOT / "tools" / "godot_templates.lock.json").read_text(encoding="utf-8"))
    if lock.get("godot") != STABLE_TAG or lock.get("commit") != STABLE_COMMIT:
        fail("tools/godot_templates.lock.json is not the 4.7.2-stable pin")
    # The trial runs only when someone dispatches godot-4.8-trial.yml. It must
    # not come back into ci.yml, where it would bill every pull request.
    ci = (ROOT / ".github" / "workflows" / "ci.yml").read_text(encoding="utf-8")
    if "godot_48_trial.py run" in ci or "godot-48-trial:" in ci:
        fail("ci.yml must not run the 4.8 trial; it lives in godot-4.8-trial.yml")
    trial = (ROOT / ".github" / "workflows" / "godot-4.8-trial.yml").read_text(encoding="utf-8")
    on = re.search(r"^on:\n(.*?)^\S", trial, re.S | re.M)
    if not on:
        fail("godot-4.8-trial.yml has no on: block")
    triggers = set(re.findall(r"^  ([a-z_]+):", on.group(1), re.M))
    if triggers != {"workflow_dispatch"}:
        fail(f"godot-4.8-trial.yml must be workflow_dispatch only, found {sorted(triggers)}")
    if re.search(r"^\s+GODOT_VERSION:", trial, re.M):
        fail("godot-4.8-trial.yml must not set GODOT_VERSION")


def snapshot_status() -> set[str]:
    out = subprocess.check_output(["git", "status", "--porcelain"], cwd=ROOT, text=True)
    return {line for line in out.splitlines() if line}


def restore_godot_rewrites(before: set[str]) -> None:
    """Put back tracked files Godot rewrote while opening the project."""
    after = snapshot_status()
    for line in sorted(after - before):
        path = line[3:]
        # Godot writes a sidecar next to each imported asset. Those are not
        # source, and a trial must not leave them in the tree.
        if line.startswith("??") and (path.endswith(".import") or path.endswith(".uid")):
            target = ROOT / path
            if target.is_file():
                target.unlink()
                print(f"removed {path} (Godot import sidecar)")
            continue
        if line.startswith("??"):
            continue
        if line.startswith(" M") or line.startswith("M "):
            print(f"restoring {path} (Godot rewrote it)")
            subprocess.run(["git", "checkout", "--", path], cwd=ROOT, check=True)


def project_bytes() -> dict[Path, bytes]:
    paths = [ROOT / "project.godot", ROOT / "export_presets.cfg", ROOT / "racer.gdextension"]
    return {p: p.read_bytes() for p in paths}


def restore_bytes(saved: dict[Path, bytes]) -> None:
    for path, data in saved.items():
        if path.read_bytes() != data:
            path.write_bytes(data)
            print(f"restored {path.relative_to(ROOT)}")


def cargo_build_extension() -> None:
    # The editor loads linux.debug; a release export packages linux.release.
    run(["cargo", "build", "-p", "racer-godot"], cwd=ROOT)
    run(["cargo", "build", "--release", "-p", "racer-godot"], cwd=ROOT)


def find_ndk() -> Optional[Path]:
    for key in ("ANDROID_NDK_HOME", "ANDROID_NDK_ROOT", "ANDROID_NDK_LATEST_HOME"):
        val = os.environ.get(key)
        if val and Path(val).is_dir():
            return Path(val)
    sdk = os.environ.get("ANDROID_HOME")
    if not sdk:
        return None
    found = sorted(Path(sdk).joinpath("ndk").glob("*"))
    return found[-1] if found else None


def cargo_android_lib(ndk: Path) -> Optional[Path]:
    host = "darwin-x86_64" if sys.platform == "darwin" else "linux-x86_64"
    clang = ndk / "toolchains" / "llvm" / "prebuilt" / host / "bin" / "aarch64-linux-android24-clang"
    if not clang.is_file():
        print(f"NDK at {ndk} has no {clang.name}; skipping the arm64 extension")
        return None
    env = os.environ.copy()
    env["ANDROID_NDK_HOME"] = str(ndk)
    subprocess.run(["rustup", "target", "add", "aarch64-linux-android"], check=True, env=env)
    run(
        [
            "cargo", "build", "--release", "-p", "racer-godot",
            "--target", "aarch64-linux-android",
            "--config", f'target.aarch64-linux-android.linker="{clang}"',
        ],
        cwd=ROOT,
        env=env,
    )
    lib = ROOT / "target" / "aarch64-linux-android" / "release" / "libracer_godot.so"
    if not lib.is_file():
        fail("android build finished without libracer_godot.so")
    return lib


def describe_so(path: Path) -> str:
    header = path.read_bytes()[:64]
    return f"{path.relative_to(ROOT)}  {path.stat().st_size} bytes  {elf_desc(header)}"


# 4.8-beta5 warns when the desktop GameMode daemon is absent. racer does not
# use that daemon; 4.7.2 simply did not print the line. It is not a load failure.
_GAME_MODE_WARNING = "WARNING: GameMode: Failed to enable"


def audio_shutdown(binary: Path, env: dict[str, str]) -> None:
    """Same launch as tools/check_audio_shutdown.py, plus the GameMode note."""
    for season in ("none", "halloween"):
        result = subprocess.run(
            [
                str(binary), "--headless", "--verbose", "--path", str(ROOT),
                "--quit-after", "120", "--", "--racer-season", season,
            ],
            cwd=ROOT,
            env=env,
            capture_output=True,
            text=True,
            errors="replace",
            timeout=180,
        )
        output = result.stdout + result.stderr
        other = [
            line for line in output.splitlines()
            if ("WARNING:" in line or "ERROR:" in line or "Leaked instance:" in line
                or "resources still in use" in line)
            and _GAME_MODE_WARNING not in line
        ]
        if result.returncode or "racer: sim epoch" not in output or other:
            print(output[-8000:])
            fail(f"audio shutdown failed: {season} (exit {result.returncode})")
        if _GAME_MODE_WARNING in output:
            print(f"noted, not a failure: {_GAME_MODE_WARNING} ({season})")
        print(f"Audio shutdown clean: {season}")


def godot_import_and_smoke(prepared: dict[str, object]) -> None:
    binary = prepared["binary"]
    assert isinstance(binary, Path)
    home = prepared["home"]
    assert isinstance(home, Path)
    env = isolated_env(home, binary)
    godot_dir = ROOT / ".godot"
    if godot_dir.exists():
        shutil.rmtree(godot_dir)
    (ROOT / "build").mkdir(exist_ok=True)
    (ROOT / "build" / ".gdignore").touch()
    run(["bash", "tools/godot_headless.sh", "--frame-delay", "800", "--import"], cwd=ROOT, env=env)
    audio_shutdown(binary, env)


def godot_export_linux(prepared: dict[str, object]) -> Path:
    binary = prepared["binary"]
    assert isinstance(binary, Path)
    home = prepared["home"]
    assert isinstance(home, Path)
    label = str(prepared["label"])
    dest = OUT / "export" / label / "racer.x86_64"
    dest.parent.mkdir(parents=True, exist_ok=True)
    env = isolated_env(home, binary)
    run(
        ["bash", "tools/godot_headless.sh", "--export-release", "Linux", str(dest)],
        cwd=ROOT,
        env=env,
    )
    if not dest.is_file():
        fail(f"{label} export did not write {dest}")
    return dest


def export_listing(dest: Path) -> list[str]:
    lines = []
    root = dest.parent
    files = [p for p in root.rglob("*") if p.is_file()]
    total = 0
    for path in sorted(files):
        size = path.stat().st_size
        total += size
        extra = ""
        if path.suffix == ".so" or path.name.startswith("lib"):
            extra = "  " + elf_desc(path.read_bytes()[:64])
        lines.append(f"  {path.relative_to(root)}  {size} bytes{extra}")
    lines.append(f"  total  {total} bytes")
    return lines


def probe_api_custom(binary: Path) -> list[str]:
    """Typecheck racer-godot against bindings dumped from the 4.8 editor.

    The manifest edit is restored before this returns, including on failure.
    The output goes to its own target directory so the api-4-7 library the
    editor just loaded stays in target/debug.
    """
    manifest = ROOT / "crates" / "racer-godot" / "Cargo.toml"
    lock = ROOT / "Cargo.lock"
    original = manifest.read_text(encoding="utf-8")
    original_lock = lock.read_bytes()
    old = 'godot = { version = "0.5", features = ["api-4-7", "lazy-function-tables"] }'
    new = 'godot = { version = "0.5", features = ["api-custom", "lazy-function-tables"] }'
    if old not in original:
        return ["api-custom probe skipped: the godot dependency line is not the stable pin"]
    log = OUT / "api-custom.log"
    log.parent.mkdir(parents=True, exist_ok=True)
    env = os.environ.copy()
    env["GDRUST_GODOT_BIN"] = str(binary)
    env["CARGO_TARGET_DIR"] = str(ROOT / "target" / "godot-48-api-custom")
    lines = [
        "api-custom probe: cargo check -p racer-godot with api-custom, "
        f"GDRUST_GODOT_BIN={binary}",
    ]
    manifest.write_text(original.replace(old, new, 1), encoding="utf-8")
    try:
        proc = subprocess.run(
            ["cargo", "check", "-p", "racer-godot"],
            cwd=ROOT,
            env=env,
            text=True,
            capture_output=True,
        )
        log.write_text(proc.stdout + "\n" + proc.stderr, encoding="utf-8")
        if proc.returncode == 0:
            lines.append(
                "api-custom probe: typecheck succeeded against bindings generated from "
                "the 4.8-beta5 editor. That is not the library this repo ships. "
                "gdext would refuse to load it on 4.7.2 (runtime older than the compiled API)."
            )
        else:
            tail = "\n".join((proc.stderr or proc.stdout).splitlines()[-40:])
            lines.append(
                f"api-custom probe: failed (exit {proc.returncode}). "
                "godot 0.5.5 has no pregenerated 4.8 API, and generating one from "
                "this editor did not typecheck racer-godot. The api-4-7 build is unchanged."
            )
            lines.append(tail)
    finally:
        manifest.write_text(original, encoding="utf-8")
        if lock.read_bytes() != original_lock:
            lock.write_bytes(original_lock)
            print("restored Cargo.lock after the api-custom probe")
        if manifest.read_text(encoding="utf-8") != original:
            fail("the api-custom probe left crates/racer-godot/Cargo.toml modified")
    return lines


def write_report(lines: list[str]) -> None:
    OUT.mkdir(parents=True, exist_ok=True)
    text = "\n".join(lines) + "\n"
    (OUT / "report.txt").write_text(text, encoding="utf-8")
    print(text)


def cmd_report() -> list[str]:
    assert_stable_pin()
    lines = ["Stable pin left at Godot 4.7.2 / api-4-7.", *godot_feature_report()]
    return lines


def cmd_fetch(labels: list[str]) -> None:
    for label in labels:
        fetch_release(label)


def cmd_provenance(labels: list[str]) -> list[str]:
    lines = []
    for label in labels:
        prepared = prepare(label)
        version = str(prepared["version"])
        commit = str(prepared["commit"])
        lines.append(f"## {label}")
        lines.append(f"editor version: {version}")
        lines.append(f"engine commit: {commit}")
        lines.append(f"template build key: {prepared['build_key']} (version.txt)")
        lines.append("packaged native libraries:")
        tpz = prepared["tpz"]
        assert isinstance(tpz, Path)
        lines.extend(template_native_libs(tpz))
    return lines


def cmd_run(skip_baseline: bool, skip_probe: bool) -> None:
    before = snapshot_status()
    saved = project_bytes()
    lines = cmd_report()
    labels = ["4.8-beta5"] if skip_baseline else ["4.7.2", "4.8-beta5"]
    try:
        prepared = {label: prepare(label) for label in labels}
        for label, item in prepared.items():
            lines.append(f"## provenance {label}")
            lines.append(f"editor version: {item['version']}")
            lines.append(f"engine commit: {item['commit']}")
            lines.append(f"template build key: {item['build_key']} (version.txt)")
            lines.append("official template native libraries:")
            tpz = item["tpz"]
            assert isinstance(tpz, Path)
            lines.extend(template_native_libs(tpz))
        cargo_build_extension()
        lines.append("## gdextension built with the committed features (api-4-7)")
        lines.append(describe_so(ROOT / "target" / "debug" / "libracer_godot.so"))
        lines.append(describe_so(ROOT / "target" / "release" / "libracer_godot.so"))
        ndk = find_ndk()
        if ndk is None:
            lines.append(
                "arm64 libracer_godot.so: not built in this environment (no Android NDK). "
                "racer.gdextension maps android.release.arm64 to "
                "target/aarch64-linux-android/release/libracer_godot.so. "
                "That library is the same api-4-7 build for either engine; "
                "The godot-4.8-trial workflow builds it (the runner sets ANDROID_NDK_LATEST_HOME)."
            )
        else:
            arm = cargo_android_lib(ndk)
            if arm is not None:
                lines.append("arm64 release extension (api-4-7, not yet inside an APK):")
                lines.append(describe_so(arm))
        for label in labels:
            print(f"=== {label}: import, audio shutdown, Linux export ===", flush=True)
            godot_import_and_smoke(prepared[label])
            dest = godot_export_linux(prepared[label])
            lines.append(f"## Linux release export ({label}, official templates, api-4-7 extension)")
            lines.extend(export_listing(dest))
        if not skip_probe:
            trial = prepared["4.8-beta5"]
            binary = trial["binary"]
            assert isinstance(binary, Path)
            lines.append("## api-custom")
            lines.extend(probe_api_custom(binary))
    except BaseException as exc:
        # SystemExit too: the audio check and fail() leave that way, and the
        # report should still say where the trial stopped.
        if isinstance(exc, KeyboardInterrupt):
            raise
        lines.append(f"## trial failed\n{type(exc).__name__}: {exc}")
        raise
    finally:
        restore_bytes(saved)
        restore_godot_rewrites(before)
        write_report(lines)


def sconstruct_python(src: Path) -> tuple[int, int]:
    text = (src / "SConstruct").read_text(encoding="utf-8")
    match = re.search(r"EnsurePythonVersion\((\d+),\s*(\d+)\)", text)
    if not match:
        fail(f"{src}/SConstruct has no EnsurePythonVersion(...)")
    return int(match.group(1)), int(match.group(2))


def fetch_trial_source(src: Path) -> None:
    """Shallow checkout of the beta commit. There is no 4.8-beta5 git tag."""
    src.parent.mkdir(parents=True, exist_ok=True)
    if not (src / ".git").exists():
        run(["git", "init", str(src)])
        run(["git", "-C", str(src), "remote", "add", "origin", GODOT_GIT])
    run(["git", "-C", str(src), "fetch", "--depth", "1", "origin", TRIAL_COMMIT])
    run(["git", "-C", str(src), "checkout", "--detach", "FETCH_HEAD"])
    head = subprocess.check_output(
        ["git", "-C", str(src), "rev-parse", "HEAD"], text=True
    ).strip()
    if head != TRIAL_COMMIT:
        fail(f"{src} is at {head}, not {TRIAL_COMMIT}")
    dirty = subprocess.check_output(
        ["git", "-C", str(src), "status", "--porcelain", "--untracked-files=no"],
        text=True,
    ).strip()
    if dirty:
        fail(f"{src} has local changes to tracked files:\n{dirty}")


def trial_profile() -> dict[str, object]:
    """The shared profile with the trial-only drops and sets applied. The file is not modified."""
    import godot_template as gt

    profile = json.loads(gt.PROFILE.read_text(encoding="utf-8"))
    options = profile.get("disabled_build_options", {})
    if not isinstance(options, dict):
        fail(f"{gt.PROFILE.name} disabled_build_options is not an object")
    missing = [name for name in TRIAL_PROFILE_DROPS if name not in options]
    if missing:
        fail(f"trial profile drops are not in {gt.PROFILE.name}: {', '.join(missing)}")
    clash = sorted(name for name in TRIAL_PROFILE_SETS if name in options)
    if clash:
        fail(f"trial profile sets are already in {gt.PROFILE.name}: {', '.join(clash)}")
    for name in TRIAL_PROFILE_DROPS:
        del options[name]
    options.update(TRIAL_PROFILE_SETS)
    return profile


def option_known(src: Path, option: str, known: str) -> bool:
    match = re.fullmatch(r"module_(\w+)_enabled", option)
    if match:
        return (src / "modules" / match.group(1) / "config.py").is_file()
    return f'"{option}"' in known


def check_trial_profile(src: Path) -> dict[str, object]:
    """Same name check as the stable builder, against the trial tree.

    Godot applies a profile's options without checking their names. An option
    4.8 renamed would otherwise be ignored. A trial drop the tree still has is
    stale and refused too, so a drop cannot outlive the reason for it. This
    does not change the stable script's tag, which `key` hashes.
    """
    import godot_template as gt

    profile = trial_profile()
    known = (src / "SConstruct").read_text(encoding="utf-8")
    for detect in (src / "platform").glob("*/detect.py"):
        known += detect.read_text(encoding="utf-8")
    options = profile["disabled_build_options"]
    assert isinstance(options, dict)
    unknown = [str(option) for option in options if not option_known(src, str(option), known)]
    stale = [option for option in TRIAL_PROFILE_DROPS if option_known(src, option, known)]
    if unknown or stale:
        fail(
            f"{gt.PROFILE.name} (with the trial drops and sets) does not match Godot "
            f"{TRIAL_VERSION} ({TRIAL_COMMIT_SHORT}): unknown={', '.join(unknown) or '-'} "
            f"stale_drops={', '.join(stale) or '-'}"
        )
    return profile


def cmd_build_engine(dry_run: bool, jobs: int) -> None:
    require_trial_python()
    src = Path.home() / ".cache" / "racer" / f"godot-{TRIAL_VERSION}"
    if "4.7" in str(src):
        fail(f"refusing to build the trial engine into the stable cache ({src})")
    out = OUT / "engine"
    print(
        f"Python {sys.version.split()[0]} meets the {TRIAL_VERSION} engine-build floor "
        f"({TRIAL_PYTHON[0]}.{TRIAL_PYTHON[1]}). The 4.7.2 build is a different command."
    )
    if dry_run:
        print(f"dry run: would fetch {TRIAL_COMMIT} into {src}")
        print(f"dry run: would write the arm64 library under {out}")
        print("dry run: would not write tools/godot_templates.lock.json")
        print(
            f"dry run: would build with the shared profile minus {', '.join(TRIAL_PROFILE_DROPS)} "
            f"plus {', '.join(f'{k}={v}' for k, v in TRIAL_PROFILE_SETS.items())}"
        )
        if not os.environ.get("ANDROID_HOME"):
            print("ANDROID_HOME is unset; a real build stops before SCons.")
        return
    android = os.environ.get("ANDROID_HOME")
    if not android:
        fail("ANDROID_HOME is unset; Godot's Android build needs the SDK")
    if shutil.which("uvx") is None and shutil.which("uv") is None:
        fail("uv is not on PATH; the stable recipe runs SCons through uvx")
    fetch_trial_source(src)
    minimum = sconstruct_python(src)
    if not version_at_least(sys.version_info[0], sys.version_info[1], minimum):
        fail(
            f"{src}/SConstruct requires Python >= {minimum[0]}.{minimum[1]}; "
            f"this interpreter is {sys.version.split()[0]}"
        )
    # Imported here so self-test does not load the stable build script.
    import godot_template as gt

    profile = check_trial_profile(src)
    out.mkdir(parents=True, exist_ok=True)
    profile_path = src.parent / f"racer-{TRIAL_VERSION}-trial-profile.json"
    profile_path.write_text(json.dumps(profile, indent="\t") + "\n", encoding="utf-8")
    print(
        f"trial profile {profile_path}: drops {', '.join(TRIAL_PROFILE_DROPS)}; "
        f"sets {', '.join(f'{k}={v}' for k, v in TRIAL_PROFILE_SETS.items())}"
    )
    env = os.environ.copy()
    # Release notes: a custom build is recognized as beta5 only with this set.
    env["GODOT_VERSION_STATUS"] = "beta5"
    env["ANDROID_HOME"] = android
    cmd = [
        *gt.SCONS,
        f"-j{jobs}",
        *gt.scons_args(gt.ARCH),
        f"build_profile={profile_path}",
    ]
    print("+ " + " ".join(cmd), flush=True)
    subprocess.run(cmd, cwd=src, env=env, check=True)
    built = (
        src / "platform" / "android" / "java" / "lib" / "libs" / "release"
        / "arm64-v8a" / "libgodot_android.so"
    )
    if not built.is_file():
        fail(f"SCons finished but {built} is missing")
    dest = out / "libgodot.android.template_release.arm64.so"
    shutil.copy2(built, dest)
    print(
        f"{dest}  {dest.stat().st_size} bytes  sha256 {hashlib.sha256(dest.read_bytes()).hexdigest()}"
    )
    print("not locked. tools/godot_templates.lock.json still pins 4.7.2.")


def cmd_self_test() -> None:
    assert_stable_pin()
    if version_at_least(3, 9, TRIAL_PYTHON):
        fail("3.9 must not pass the 4.8 engine-build floor")
    if not version_at_least(3, 10, TRIAL_PYTHON):
        fail("3.10 must pass the 4.8 engine-build floor")
    if not version_at_least(3, 12, TRIAL_PYTHON):
        fail("3.12 must pass the 4.8 engine-build floor")
    for spec in RELEASES.values():
        for key in ("editor_sha512", "templates_sha512"):
            digest = spec[key]
            if len(digest) != 128 or any(c not in "0123456789abcdef" for c in digest):
                fail(f"bad sha512 for {spec['editor'] if 'editor' in key else spec['templates']}")
        if len(spec["commit"]) != 40:
            fail(f"commit for {spec['tag']} is not 40 hex chars")
    import godot_template as gt

    shared = json.loads(gt.PROFILE.read_text(encoding="utf-8"))["disabled_build_options"]
    adjusted = trial_profile()["disabled_build_options"]
    if not isinstance(adjusted, dict):
        fail("the trial profile's disabled_build_options is not an object")
    if set(shared) - set(adjusted) != set(TRIAL_PROFILE_DROPS):
        fail("the trial profile must drop exactly the trial drops")
    if adjusted.get("rendering_device") is not False or shared.get("vulkan") is not False:
        fail("a Vulkan-less 4.8 build must also set rendering_device=false")
    if "rendering_device" in shared:
        fail("rendering_device must stay out of the shared 4.7.2 profile")
    with tempfile.TemporaryDirectory() as tmp:
        tree = Path(tmp)
        (tree / "platform").mkdir()
        names = []
        for option in adjusted:
            match = re.fullmatch(r"module_(\w+)_enabled", option)
            if match:
                (tree / "modules" / match.group(1)).mkdir(parents=True)
                (tree / "modules" / match.group(1) / "config.py").write_text("")
            else:
                names.append(f'"{option}"')
        (tree / "SConstruct").write_text("\n".join(names))
        check_trial_profile(tree)
    if TRIAL_COMMIT_SHORT != "a661dfdbc":
        fail("short commit no longer matches the 4.8-beta5 release notes")
    print("self-test ok: stable pin is 4.7.2, trial Python floor rejects 3.9, trial profile drops regex and sets rendering_device=false")


def main() -> None:
    parser = argparse.ArgumentParser(description=__doc__, formatter_class=argparse.RawDescriptionHelpFormatter)
    sub = parser.add_subparsers(dest="cmd", required=True)
    sub.add_parser("self-test")
    sub.add_parser("report")
    sub.add_parser("fetch")
    sub.add_parser("provenance")
    run_p = sub.add_parser("run")
    run_p.add_argument("--skip-baseline", action="store_true")
    run_p.add_argument("--skip-probe", action="store_true")
    build_p = sub.add_parser("build-engine")
    build_p.add_argument("--dry-run", action="store_true")
    build_p.add_argument("-j", "--jobs", type=int, default=os.cpu_count() or 4)
    args = parser.parse_args()

    if args.cmd == "self-test":
        cmd_self_test()
    elif args.cmd == "report":
        print("\n".join(cmd_report()))
    elif args.cmd == "fetch":
        cmd_fetch(["4.7.2", "4.8-beta5"])
    elif args.cmd == "provenance":
        print("\n".join(cmd_provenance(["4.7.2", "4.8-beta5"])))
    elif args.cmd == "run":
        cmd_run(args.skip_baseline, args.skip_probe)
    elif args.cmd == "build-engine":
        cmd_build_engine(args.dry_run, args.jobs)


if __name__ == "__main__":
    main()
