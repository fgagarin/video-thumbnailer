"""Shared pytest fixtures for the video-thumbnailer test suite."""

from __future__ import annotations

import os
import subprocess
from pathlib import Path

import imageio_ffmpeg
import pytest
from PIL import Image

FIXTURES_DIR = Path(__file__).parent / "fixtures"

_FFMPEG = imageio_ffmpeg.get_ffmpeg_exe()


def _generate_video(output: Path, duration: int, width: int, height: int, extra: list[str]) -> None:
    """Generate a synthetic test video using ffmpeg lavfi test source."""
    subprocess.run(
        [
            _FFMPEG, "-y",
            "-f", "lavfi", "-i", f"testsrc=size={width}x{height}:rate=25:duration={duration}",
            "-f", "lavfi", "-i", f"sine=frequency=440:duration={duration}",
            *extra,
            str(output),
        ],
        check=True,
        capture_output=True,
    )


@pytest.fixture()
def sample_video(tmp_path: Path) -> Path:
    """Return the path to a short (5-second) 320x240 MP4 test video in tmp_path.

    The video is generated freshly for each test to allow safe in-place modification.
    """
    output = tmp_path / "sample.mp4"
    _generate_video(output, 5, 320, 240, ["-c:v", "libx264", "-c:a", "aac", "-pix_fmt", "yuv420p"])
    return output


@pytest.fixture()
def sample_pil_image() -> Image.Image:
    """Return a 320x240 solid-blue PIL Image in RGB mode."""
    return Image.new("RGB", (320, 240), color=(0, 0, 200))


@pytest.fixture()
def video_with_thumb(tmp_path: Path) -> Path:
    """Return the path to an MP4 with an embedded JPEG cover-art stream.

    Generates the file fresh in tmp_path for safe modification.
    """
    base = tmp_path / "base.mp4"
    output = tmp_path / "with_thumb.mp4"
    _generate_video(base, 5, 320, 240, ["-c:v", "libx264", "-c:a", "aac", "-pix_fmt", "yuv420p"])

    cover = tmp_path / "cover.jpg"
    subprocess.run(
        [
            _FFMPEG, "-y",
            "-f", "lavfi", "-i", "color=c=blue:size=320x240:rate=1:duration=1",
            "-frames:v", "1", "-q:v", "2", str(cover),
        ],
        check=True,
        capture_output=True,
    )
    subprocess.run(
        [
            _FFMPEG, "-y",
            "-i", str(base),
            "-i", str(cover),
            "-map", "0", "-map", "1",
            "-c", "copy",
            "-disposition:v:1", "attached_pic",
            str(output),
        ],
        check=True,
        capture_output=True,
    )
    return output


@pytest.fixture()
def read_only_video(tmp_path: Path) -> Path:
    """Return the path to a read-only copy of a small MP4 in tmp_path.

    Permissions are restored to writable in teardown so pytest can clean up.
    """
    source = tmp_path / "readonly.mp4"
    _generate_video(source, 5, 320, 240, ["-c:v", "libx264", "-c:a", "aac", "-pix_fmt", "yuv420p"])
    os.chmod(source, 0o444)
    yield source  # type: ignore[misc]
    os.chmod(source, 0o644)


def _make_video_for_format(fmt: str, tmp_path: Path) -> Path:
    """Helper to generate a fixture video for the given format extension."""
    output = tmp_path / f"sample.{fmt}"
    if fmt in ("mp4", "mov", "mkv", "avi", "flv"):
        _generate_video(output, 5, 320, 240, ["-c:v", "libx264", "-c:a", "aac", "-pix_fmt", "yuv420p"])
    elif fmt == "webm":
        _generate_video(output, 5, 320, 240, ["-c:v", "libvpx-vp9", "-c:a", "libopus", "-b:v", "200k"])
    else:
        raise ValueError(f"Unsupported fixture format: {fmt}")
    return output
