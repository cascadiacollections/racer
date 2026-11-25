#!/usr/bin/env python3
"""Fail unless the F-Droid version agrees with itself.

Three places must match, and the version code must be Salt Line's scheme
for that version name (major*100000000 + minor*1000000 + patch*10000 + 9999):

    Cargo.toml              [workspace.package] version
    export_presets.cfg      the F-Droid preset's version/name and version/code
    fdroid/version.txt      versionName and versionCode

The fastlane changelog for that versionCode must exist. Title and short
description must fit F-Droid's limits. Store presets are not part of this
check. Standard library only.
"""

from __future__ import annotations

import re
import sys
from pathlib import Path
from typing import NoReturn

ROOT = Path(__file__).resolve().parent.parent
SEMVER = re.compile(r"^(\d+)\.(\d+)\.(\d+)$")
# versionCode = major*10^8 + minor*10^6 + patch*10^4 + 9999. Android caps
# versionCode at 2_100_000_000, which is what bounds major at 20.
MAX_MAJOR = 20
MAX_MINOR = 99
MAX_PATCH = 99
RELEASE_BUILD = 9999
TITLE_LIMIT = 50
SHORT_LIMIT = 80
FULL_LIMIT = 4000


def fail(msg: str) -> NoReturn:
    print(f"error: {msg}", file=sys.stderr)
    sys.exit(1)


def read(rel: str) -> str:
    path = ROOT / rel
    try:
        return path.read_text(encoding="utf-8")
    except FileNotFoundError:
        fail(f"{rel} not found")


def cargo_version(text: str) -> str:
    section = re.search(r"^\[workspace\.package\]\s*$([\s\S]*?)(?=^\[|\Z)", text, re.M)
    if section is None:
        fail("Cargo.toml has no [workspace.package] section")
    found = re.search(r'^version\s*=\s*"([^"]+)"', section.group(1), re.M)
    if found is None:
        fail("Cargo.toml has no [workspace.package] version")
    return found.group(1)


def fdroid_preset(text: str) -> dict[str, str]:
    """Options of the preset named F-Droid."""
    names: dict[str, str] = {}
    options: dict[str, dict[str, str]] = {}
    section: str | None = None
    for raw in text.splitlines():
        line = raw.strip()
        if not line or line.startswith(";"):
            continue
        header = re.match(r"^\[(preset\.\d+)(\.options)?\]$", line)
        if header:
            section = header.group(1) + (".options" if header.group(2) else "")
            continue
        if section is None or "=" not in line:
            continue
        key, value = line.split("=", 1)
        value = value.strip().strip('"')
        if section.endswith(".options"):
            preset = section[: -len(".options")]
            options.setdefault(preset, {})[key] = value
        elif key == "name":
            names[section] = value
    matches = [preset for preset, name in names.items() if name == "F-Droid"]
    if len(matches) != 1:
        fail(f"expected one F-Droid preset, found {len(matches)}")
    return options.get(matches[0], {})


def version_file(text: str) -> tuple[str, str]:
    code: str | None = None
    name: str | None = None
    for line in text.splitlines():
        if not line or line.startswith("#"):
            continue
        if "=" not in line:
            fail(f"fdroid/version.txt has a line that is not key=value: {line!r}")
        key, value = line.split("=", 1)
        if key == "versionCode":
            code = value
        elif key == "versionName":
            name = value
        else:
            fail(f"fdroid/version.txt has an unexpected key {key!r}")
    if code is None or name is None:
        fail("fdroid/version.txt needs versionCode and versionName")
    return code, name


def version_code(major: int, minor: int, patch: int) -> int:
    if major > MAX_MAJOR or minor > MAX_MINOR or patch > MAX_PATCH:
        fail(
            f"{major}.{minor}.{patch} is outside the F-Droid versionCode scheme "
            f"(major <= {MAX_MAJOR}, minor and patch <= {MAX_MINOR})"
        )
    return major * 100_000_000 + minor * 1_000_000 + patch * 10_000 + RELEASE_BUILD


def parse_semver(version: str) -> tuple[int, int, int]:
    match = SEMVER.match(version)
    if match is None:
        fail(f"{version!r} is not a plain MAJOR.MINOR.PATCH version")
    return int(match.group(1)), int(match.group(2)), int(match.group(3))


def check_listing(code: str) -> None:
    meta = ROOT / "fastlane/metadata/android/en-US"
    title = read("fastlane/metadata/android/en-US/title.txt").strip()
    short = read("fastlane/metadata/android/en-US/short_description.txt").strip()
    full = read("fastlane/metadata/android/en-US/full_description.txt").strip()
    if len(title) > TITLE_LIMIT:
        fail(f"title.txt is {len(title)} characters; F-Droid allows {TITLE_LIMIT}")
    if len(short) > SHORT_LIMIT:
        fail(f"short_description.txt is {len(short)} characters; F-Droid allows {SHORT_LIMIT}")
    if len(full) > FULL_LIMIT:
        fail(f"full_description.txt is {len(full)} characters; F-Droid allows {FULL_LIMIT}")
    if not title or not short or not full:
        fail("fastlane title, short description and full description must be non-empty")
    changelog = meta / "changelogs" / f"{code}.txt"
    if not changelog.is_file() or not changelog.read_text(encoding="utf-8").strip():
        fail(f"missing fastlane changelog for versionCode {code}: {changelog.relative_to(ROOT)}")


def sanity() -> None:
    if version_code(0, 1, 0) != 1_009_999:
        fail("version code scheme: 0.1.0 must be 1009999")
    if version_code(1, 2, 3) != 102_039_999:
        fail("version code scheme: 1.2.3 must be 102039999")


def main() -> None:
    sanity()
    name = cargo_version(read("Cargo.toml"))
    major, minor, patch = parse_semver(name)
    expected = str(version_code(major, minor, patch))
    preset = fdroid_preset(read("export_presets.cfg"))
    file_code, file_name = version_file(read("fdroid/version.txt"))
    found = {
        "Cargo.toml version": name,
        "F-Droid preset version/name": preset.get("version/name", ""),
        "fdroid/version.txt versionName": file_name,
    }
    codes = {
        "scheme for " + name: expected,
        "F-Droid preset version/code": preset.get("version/code", ""),
        "fdroid/version.txt versionCode": file_code,
    }
    width = max(len(label) for label in (*found, *codes))
    for label, value in found.items():
        print(f"{label:<{width}}  {value}")
    for label, value in codes.items():
        print(f"{label:<{width}}  {value}")
    if len(set(found.values())) != 1:
        fail("version names disagree")
    if len(set(codes.values())) != 1:
        fail("version codes disagree")
    check_listing(expected)
    print(f"ok: {name} ({expected})")


if __name__ == "__main__":
    main()
