#!/usr/bin/env python3
"""Reconstruct the OHOS UI source from its pinned original without editing it."""

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
    with tempfile.TemporaryDirectory(prefix="photocraft-ui-overlay-") as temporary:
        reconstructed = Path(temporary)
        shutil.copytree(upstream / "crates/ui-egui/src", reconstructed / "src")
        subprocess.run(
            [
                "patch", "-s", "-p1", "-d", str(reconstructed),
                "-i", str(package / "ohos-async-files.patch"),
            ],
            check=True,
            stdin=subprocess.DEVNULL,
        )
        generated = {p.relative_to(reconstructed / "src") for p in (reconstructed / "src").rglob("*") if p.is_file()}
        vendored = {p.relative_to(package / "src") for p in (package / "src").rglob("*") if p.is_file()}
        if generated != vendored:
            raise SystemExit("overlay source file inventory differs")
        for relative in sorted(generated):
            if (reconstructed / "src" / relative).read_bytes() != (package / "src" / relative).read_bytes():
                raise SystemExit(f"overlay source differs: {relative}")
    print("Pinned UI source + patch reproduces the OHOS overlay exactly.")


if __name__ == "__main__":
    main()
