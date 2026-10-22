"""Check real Godot audio teardown after building and importing the project.

Run: python tools/check_audio_shutdown.py --godot /path/to/godot
Both title tunes must start and quit without leaked playback or resources.
"""

import argparse
from pathlib import Path
import subprocess


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--godot", default="godot")
    args = parser.parse_args()
    root = Path(__file__).resolve().parent.parent
    for season in ("none", "halloween"):
        result = subprocess.run(
            [
                args.godot, "--headless", "--verbose", "--path", str(root),
                "--quit-after", "120", "--", "--racer-season", season,
            ],
            capture_output=True, text=True, errors="replace", timeout=180,
        )
        output = result.stdout + result.stderr
        failures = ("ERROR:", "WARNING:", "Leaked instance:", "resources still in use")
        if (result.returncode or "racer: sim epoch" not in output
                or any(marker in output for marker in failures)):
            print(output)
            raise SystemExit(f"Audio shutdown failed: {season} (exit {result.returncode})")
        print(f"Audio shutdown clean: {season}")


if __name__ == "__main__":
    main()
