"""
Generate test fixture video files used by the test suite.

Usage:
    python tests/generate_fixtures.py

Generates the following files in tests/fixtures/ (skipped if they already exist):
    sample.mp4              — 10-second 320x240 H.264 video
    sample.mov              — 10-second 320x240 H.264 video (QuickTime container)
    sample.mkv              — 10-second 320x240 H.264 video (Matroska container)
    sample.avi              — 10-second 320x240 H.264 video (AVI container)
    sample.webm             — 10-second 320x240 VP9 video (WebM container)
    sample.flv              — 10-second 320x240 H.264 video (FLV container)
    sample_with_thumb.mp4   — MP4 with embedded JPEG cover art
    sample_with_thumb.mkv   — MKV with JPEG attachment (cover.jpg)
    sample_4k.mp4           — 5-second 3840x2160 H.264 video (4K, for benchmarks)
    sample_1080p.mp4        — 5-second 1920x1080 H.264 video (for benchmarks)

Requires ffmpeg to be installed and on PATH.
"""

from __future__ import annotations

import os
import subprocess
import tempfile
from pathlib import Path

FIXTURES_DIR = Path(__file__).parent / "fixtures"


def _ffmpeg(*args: str) -> None:
    """Run ffmpeg with the given arguments, raising on non-zero exit."""
    subprocess.run(["ffmpeg", "-y", *args], check=True, capture_output=True)


def _generate_base(output: Path, duration: int, width: int, height: int, extra_args: list[str] | None = None) -> None:
    """Generate a synthetic test video using the lavfi test source."""
    if output.exists():
        return
    base: list[str] = [
        "-f", "lavfi",
        "-i", f"testsrc=size={width}x{height}:rate=25:duration={duration}",
        "-f", "lavfi",
        "-i", f"sine=frequency=440:duration={duration}",
    ]
    if extra_args:
        base.extend(extra_args)
    base.append(str(output))
    _ffmpeg(*base)
    print(f"  created {output.name}")


def generate_fixtures() -> None:
    """Generate all fixture files. Idempotent — skips existing files."""
    FIXTURES_DIR.mkdir(parents=True, exist_ok=True)

    # Basic 10-second 320x240 samples per format
    _generate_base(
        FIXTURES_DIR / "sample.mp4", 10, 320, 240,
        ["-c:v", "libx264", "-c:a", "aac", "-pix_fmt", "yuv420p"],
    )
    _generate_base(
        FIXTURES_DIR / "sample.mov", 10, 320, 240,
        ["-c:v", "libx264", "-c:a", "aac", "-pix_fmt", "yuv420p"],
    )
    _generate_base(
        FIXTURES_DIR / "sample.mkv", 10, 320, 240,
        ["-c:v", "libx264", "-c:a", "aac", "-pix_fmt", "yuv420p"],
    )
    _generate_base(
        FIXTURES_DIR / "sample.avi", 10, 320, 240,
        ["-c:v", "libx264", "-c:a", "aac", "-pix_fmt", "yuv420p"],
    )
    _generate_base(
        FIXTURES_DIR / "sample.webm", 10, 320, 240,
        ["-c:v", "libvpx-vp9", "-c:a", "libopus", "-b:v", "200k"],
    )
    _generate_base(
        FIXTURES_DIR / "sample.flv", 10, 320, 240,
        ["-c:v", "libx264", "-c:a", "aac", "-pix_fmt", "yuv420p", "-ar", "44100"],
    )

    # Benchmark fixtures
    _generate_base(
        FIXTURES_DIR / "sample_1080p.mp4", 5, 1920, 1080,
        ["-c:v", "libx264", "-c:a", "aac", "-pix_fmt", "yuv420p"],
    )
    _generate_base(
        FIXTURES_DIR / "sample_4k.mp4", 5, 3840, 2160,
        ["-c:v", "libx264", "-c:a", "aac", "-pix_fmt", "yuv420p", "-preset", "ultrafast"],
    )

    # MP4 with embedded cover art
    mp4_with_thumb = FIXTURES_DIR / "sample_with_thumb.mp4"
    if not mp4_with_thumb.exists():
        base_mp4 = FIXTURES_DIR / "sample.mp4"
        with tempfile.NamedTemporaryFile(suffix=".jpg", delete=False) as tmp:
            cover_path = tmp.name
        try:
            # Generate a solid-colour JPEG as cover art
            _ffmpeg(
                "-f", "lavfi", "-i", "color=c=blue:size=320x240:rate=1:duration=1",
                "-frames:v", "1", "-q:v", "2", cover_path,
            )
            _ffmpeg(
                "-i", str(base_mp4),
                "-i", cover_path,
                "-map", "0", "-map", "1",
                "-c", "copy",
                "-disposition:v:1", "attached_pic",
                str(mp4_with_thumb),
            )
        finally:
            os.unlink(cover_path)
        print(f"  created {mp4_with_thumb.name}")

    # MKV with JPEG attachment (cover.jpg)
    mkv_with_thumb = FIXTURES_DIR / "sample_with_thumb.mkv"
    if not mkv_with_thumb.exists():
        base_mkv = FIXTURES_DIR / "sample.mkv"
        with tempfile.NamedTemporaryFile(suffix=".jpg", delete=False, prefix="cover_") as tmp:
            cover_path = tmp.name
        try:
            _ffmpeg(
                "-f", "lavfi", "-i", "color=c=red:size=320x240:rate=1:duration=1",
                "-frames:v", "1", "-q:v", "2", cover_path,
            )
            _ffmpeg(
                "-i", str(base_mkv),
                "-attach", cover_path,
                "-metadata:s:t", "mimetype=image/jpeg",
                "-metadata:s:t", "filename=cover.jpg",
                "-c", "copy",
                str(mkv_with_thumb),
            )
        finally:
            os.unlink(cover_path)
        print(f"  created {mkv_with_thumb.name}")

    print("All fixtures ready.")


if __name__ == "__main__":
    print(f"Generating fixtures in {FIXTURES_DIR} ...")
    generate_fixtures()
