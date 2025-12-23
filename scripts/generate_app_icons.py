"""Generate app icon assets from the source logo image.

This script resizes the project logo into a set of commonly used app icon sizes
(Android launcher densities + general-purpose PNG sizes) and writes a
multi-resolution Windows .ico.

Outputs are written to:
- gfx/vibeEmu_512px.png
- gfx/vibeEmu.ico
- gfx/app_icons/... (organized by platform/use)

The source image is expected to be a square PNG with transparency.
"""

from __future__ import annotations

import argparse
from dataclasses import dataclass
from pathlib import Path
from typing import Iterable

from PIL import Image


@dataclass(frozen=True)
class OutputPng:
    """A single PNG output specification."""

    path: Path
    size_px: int


ANDROID_LAUNCHER_SIZES: dict[str, int] = {
    "mipmap-mdpi": 48,
    "mipmap-hdpi": 72,
    "mipmap-xhdpi": 96,
    "mipmap-xxhdpi": 144,
    "mipmap-xxxhdpi": 192,
}

COMMON_PNG_SIZES: tuple[int, ...] = (
    16,
    24,
    32,
    48,
    64,
    96,
    128,
    256,
    512,
)

ICO_SIZES: tuple[int, ...] = (
    16,
    24,
    32,
    48,
    64,
    128,
    256,
)


def _open_source(path: Path) -> Image.Image:
    """Open the source image and normalize it to RGBA."""

    img = Image.open(path)
    img.load()

    if img.mode != "RGBA":
        img = img.convert("RGBA")

    if img.width != img.height:
        raise ValueError(
            f"Source image must be square, got {img.width}x{img.height}: {path}"
        )

    return img


def _resize_square(img: Image.Image, size_px: int) -> Image.Image:
    """Resize to a square PNG icon size using a high-quality filter."""

    if size_px <= 0:
        raise ValueError(f"size_px must be > 0, got {size_px}")

    return img.resize((size_px, size_px), resample=Image.Resampling.LANCZOS)


def _ensure_parent_dir(path: Path) -> None:
    """Create the parent directory for an output file if missing."""

    path.parent.mkdir(parents=True, exist_ok=True)


def write_pngs(source: Image.Image, outputs: Iterable[OutputPng]) -> None:
    """Write a set of PNG outputs."""

    for out in outputs:
        _ensure_parent_dir(out.path)
        resized = _resize_square(source, out.size_px)
        resized.save(out.path, format="PNG")


def write_ico(source: Image.Image, out_path: Path, sizes: tuple[int, ...]) -> None:
    """Write a multi-resolution .ico from the given source image."""

    _ensure_parent_dir(out_path)

    # Pillow builds the ICO frames by resizing from the base image.
    # Keeping a high-res source avoids artifacts in the larger sizes.
    source.save(out_path, format="ICO", sizes=[(s, s) for s in sizes])


def build_outputs(repo_root: Path) -> tuple[list[OutputPng], Path]:
    """Construct output paths under the repository."""

    png_outputs: list[OutputPng] = []

    # The UI crate loads this at runtime for the window icon.
    png_outputs.append(
        OutputPng(repo_root / "gfx" / "vibeEmu_512px.png", size_px=512)
    )

    # General PNG sizes.
    for size in COMMON_PNG_SIZES:
        png_outputs.append(
            OutputPng(
                repo_root / "gfx" / "app_icons" / "png" / f"{size}x{size}.png",
                size_px=size,
            )
        )

    # Android launcher sizes (drop-in compatible directory names).
    for folder, size in ANDROID_LAUNCHER_SIZES.items():
        png_outputs.append(
            OutputPng(
                repo_root
                / "gfx"
                / "app_icons"
                / "android"
                / folder
                / "ic_launcher.png",
                size_px=size,
            )
        )

    ico_path = repo_root / "gfx" / "vibeEmu.ico"
    return png_outputs, ico_path


def parse_args() -> argparse.Namespace:
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument(
        "--repo-root",
        type=Path,
        default=Path(__file__).resolve().parents[1],
        help="Path to the repository root (default: inferred).",
    )
    parser.add_argument(
        "--source",
        type=Path,
        default=Path("gfx/vibeEmu_logo_no_text_407px.png"),
        help="Path to the source PNG, relative to repo root by default.",
    )
    return parser.parse_args()


def main() -> int:
    args = parse_args()
    repo_root: Path = args.repo_root.resolve()
    source_path: Path = (repo_root / args.source).resolve()

    source = _open_source(source_path)
    png_outputs, ico_path = build_outputs(repo_root)

    write_pngs(source, png_outputs)
    write_ico(source, ico_path, ICO_SIZES)

    print(f"Wrote {len(png_outputs)} PNGs")
    print(f"Wrote ICO: {ico_path.relative_to(repo_root)}")
    return 0


if __name__ == "__main__":
    raise SystemExit(main())
