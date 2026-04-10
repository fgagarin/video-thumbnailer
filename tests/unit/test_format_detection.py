"""Unit tests for format detection in PyAVVideoLoader (T034)."""

from __future__ import annotations

import os
from pathlib import Path

import pytest

from video_thumbnailer.core.video_loader import PyAVVideoLoader, _detect_format
from video_thumbnailer.models import VideoFormat

# ---------------------------------------------------------------------------
# _detect_format() unit tests (fast, no file I/O)
# ---------------------------------------------------------------------------


def test_detect_format_mp4() -> None:
    assert _detect_format("mov,mp4,m4a,3gp,3g2,mj2", "/path/to/video.mp4") == VideoFormat.MP4


def test_detect_format_mov() -> None:
    assert _detect_format("mov,mp4,m4a,3gp,3g2,mj2", "/path/to/video.mov") == VideoFormat.MOV


def test_detect_format_mkv() -> None:
    assert _detect_format("matroska,webm", "/path/to/video.mkv") == VideoFormat.MKV


def test_detect_format_webm() -> None:
    assert _detect_format("matroska,webm", "/path/to/video.webm") == VideoFormat.WEBM


def test_detect_format_avi() -> None:
    assert _detect_format("avi", "/path/to/video.avi") == VideoFormat.AVI


def test_detect_format_flv() -> None:
    assert _detect_format("flv", "/path/to/video.flv") == VideoFormat.FLV


def test_detect_format_unsupported() -> None:
    assert _detect_format("rm", "/path/to/video.rm") == VideoFormat.UNSUPPORTED


def test_detect_format_unknown_extension_with_mp4_container() -> None:
    """An unknown extension with the combined mov/mp4 format name defaults to MP4."""
    result = _detect_format("mov,mp4,m4a,3gp,3g2,mj2", "/path/to/video.xyz")
    assert result == VideoFormat.MP4


# ---------------------------------------------------------------------------
# PyAVVideoLoader.load() on generated fixture files
# ---------------------------------------------------------------------------


def _make_raw_video(output: Path, fmt: str) -> None:
    """Generate a small synthetic video in the given format."""
    import subprocess

    import imageio_ffmpeg
    ffmpeg = imageio_ffmpeg.get_ffmpeg_exe()
    if fmt in ("mp4", "mov", "mkv", "avi", "flv"):
        subprocess.run(
            [
                ffmpeg, "-y",
                "-f", "lavfi", "-i", "testsrc=size=320x240:rate=25:duration=3",
                "-f", "lavfi", "-i", "sine=frequency=440:duration=3",
                "-c:v", "libx264", "-c:a", "aac", "-pix_fmt", "yuv420p",
                str(output),
            ],
            check=True,
            capture_output=True,
        )
    elif fmt == "webm":
        subprocess.run(
            [
                ffmpeg, "-y",
                "-f", "lavfi", "-i", "testsrc=size=320x240:rate=25:duration=3",
                "-f", "lavfi", "-i", "sine=frequency=440:duration=3",
                "-c:v", "libvpx-vp9", "-c:a", "libopus", "-b:v", "200k",
                str(output),
            ],
            check=True,
            capture_output=True,
        )


@pytest.mark.parametrize(
    ("extension", "expected_format"),
    [
        ("mp4", VideoFormat.MP4),
        ("mov", VideoFormat.MOV),
        ("mkv", VideoFormat.MKV),
        ("webm", VideoFormat.WEBM),
        ("avi", VideoFormat.AVI),
        ("flv", VideoFormat.FLV),
    ],
)
def test_load_detects_correct_format(
    tmp_path: Path, extension: str, expected_format: VideoFormat
) -> None:
    """All six supported formats are detected correctly by the loader."""
    video_path = tmp_path / f"sample.{extension}"
    _make_raw_video(video_path, extension)
    loader = PyAVVideoLoader()
    result = loader.load(str(video_path))
    assert result.format == expected_format


def test_load_mp4_renamed_to_xyz_still_detected_via_container_header(tmp_path: Path) -> None:
    """An MP4 file renamed to .xyz must still be detected as MP4.

    PyAV reads the container bytes (not the filename), and our _detect_format
    function defaults to MP4 when encountering the combined mov/mp4 format
    with an unrecognised file extension.
    """
    video_path = tmp_path / "sample.mp4"
    _make_raw_video(video_path, "mp4")

    renamed = tmp_path / "sample.xyz"
    os.rename(video_path, renamed)

    loader = PyAVVideoLoader()
    result = loader.load(str(renamed))
    assert result.format == VideoFormat.MP4
