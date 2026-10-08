#!/usr/bin/env python3
"""Reconstruct the OHOS engine source/tests from the pinned clean checkout."""

from pathlib import Path
import shutil
import subprocess
import tempfile


def main() -> None:
    package = Path(__file__).resolve().parent
    upstream = package.parents[1] / "upstream"
    expected = "4337a6227a823a28728e68aed844feab62b3314d"
    revision = subprocess.check_output(
        ["git", "-C", str(upstream), "rev-parse", "HEAD"], text=True
    ).strip()
    if revision != expected:
        raise SystemExit(f"upstream revision mismatch: {revision}")
    with tempfile.TemporaryDirectory(prefix="photocraft-engine-overlay-") as temporary:
        reconstructed = Path(temporary)
        for directory in ("src", "tests"):
            shutil.copytree(upstream / "crates/engine" / directory, reconstructed / directory)
        subprocess.run(
            ["patch", "-s", "-p1", "-d", str(reconstructed), "-i", str(package / "ohos-print-spool.patch")],
            check=True,
            stdin=subprocess.DEVNULL,
        )
        for directory in ("src", "tests"):
            generated = {p.relative_to(reconstructed / directory) for p in (reconstructed / directory).rglob("*") if p.is_file()}
            vendored = {p.relative_to(package / directory) for p in (package / directory).rglob("*") if p.is_file()}
            if generated != vendored:
                raise SystemExit(f"overlay {directory} file inventory differs")
            for relative in sorted(generated):
                if (reconstructed / directory / relative).read_bytes() != (package / directory / relative).read_bytes():
                    raise SystemExit(f"overlay source differs: {directory}/{relative}")
    print("Pinned engine source/tests + patch reproduce the OHOS overlay exactly.")


if __name__ == "__main__":
    main()
