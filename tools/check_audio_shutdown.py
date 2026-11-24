"""Check real Godot audio teardown after building and importing the project.

Run: python tools/check_audio_shutdown.py --godot /path/to/godot

Quits from the title, once for each tune, and from a short free drive with
the engine loop up. Fails if Godot reports a leaked playback or a resource
still in use. This checks resource cleanup, not audible playback.

The free drive is the pinned hand-made draft in racer-sim's layout tests
(`PINNED_DRAFT`). It is not a demo: DEMO is silent, so quitting from it
would not exercise the engine loop.
"""

import argparse
from pathlib import Path
import subprocess

# crates/racer-sim/src/layout/tests.rs, PINNED_DRAFT. A free drive, so it
# races without the solver and the engine loop is playing before the first
# frame returns.
RACE_LAYOUT = (
    "RD047GE70224Y86Y8QZR4290080G0ZZ0010200J87Z51Z0F0A2RY8ZJ3ZMRZJ7J5H8F0FR1FZZWAW5QR09CZ40GC1DC1V9M"
)

MARKERS = ("ERROR:", "WARNING:", "Leaked instance:", "resources still in use")


def plans(root: Path, godot: str) -> list[tuple[str, list[str]]]:
    """(name, argv) for each headless quit."""
    out = []
    for season in ("none", "halloween"):
        out.append(
            (
                f"title {season}",
                [
                    godot,
                    "--headless",
                    "--verbose",
                    "--path",
                    str(root),
                    "--quit-after",
                    "120",
                    "--",
                    "--racer-season",
                    season,
                ],
            )
        )
    out.append(
        (
            "race",
            [
                godot,
                "--headless",
                "--verbose",
                "--path",
                str(root),
                "--quit-after",
                "180",
                "--",
                "--racer-season",
                "none",
                "--racer-layout",
                RACE_LAYOUT,
            ],
        )
    )
    return out


def problem(output: str, returncode: int, race: bool) -> str | None:
    """Why `output` is a failed shutdown, or None if it is clean."""
    if returncode:
        return f"exit {returncode}"
    if "racer: sim epoch" not in output:
        return "the game did not start"
    if race and "racer: --racer-layout:" not in output:
        return "the race did not start"
    for marker in MARKERS:
        if marker in output:
            return marker
    return None


def main() -> None:
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--godot", default="godot")
    args = parser.parse_args()
    root = Path(__file__).resolve().parent.parent
    for name, cmd in plans(root, args.godot):
        result = subprocess.run(
            cmd,
            capture_output=True,
            text=True,
            errors="replace",
            timeout=180,
        )
        output = result.stdout + result.stderr
        why = problem(output, result.returncode, race=name == "race")
        if why is not None:
            print(output)
            raise SystemExit(f"Audio shutdown failed: {name} ({why})")
        print(f"Audio shutdown clean: {name}")


if __name__ == "__main__":
    main()
