"""Integration tests for AVI and FLV thumbnail workflows (T036)."""

from __future__ import annotations

import subprocess
from pathlib import Path
from unittest.mock import patch

import imageio_ffmpeg

from video_thumbnailer.core.frame_extractor import PyAVFrameExtractor
from video_thumbnailer.core.thumbnail_writer import FormatDispatchThumbnailWriter
from video_thumbnailer.core.video_loader import PyAVVideoLoader
from video_thumbnailer.models import TimelinePosition

_FFMPEG = imageio_ffmpeg.get_ffmpeg_exe()


def _make_video(output: Path, fmt: str) -> None:
    subprocess.run(
        [
            _FFMPEG, "-y",
            "-f", "lavfi", "-i", "testsrc=size=320x240:rate=25:duration=5",
            "-f", "lavfi", "-i", "sine=frequency=440:duration=5",
            "-c:v", "libx264", "-c:a", "aac", "-pix_fmt", "yuv420p",
            str(output),
        ],
        check=True, capture_output=True,
    )


def _run_pipeline(video_path: Path) -> tuple[bool, str | None]:
    loader = PyAVVideoLoader()
    extractor = PyAVFrameExtractor()
    writer = FormatDispatchThumbnailWriter()
    video = loader.load(str(video_path))
    frame = extractor.extract(video, TimelinePosition(offset_ms=2500))
    result = writer.write(video, frame)
    return result.success, result.error_message


# ---------------------------------------------------------------------------
# AVI
# ---------------------------------------------------------------------------

def test_avi_pipeline_succeeds(tmp_path: Path) -> None:
    """AVI pipeline returns success=True (even though no cover art is embedded)."""
    video_path = tmp_path / "sample.avi"
    _make_video(video_path, "avi")
    success, _ = _run_pipeline(video_path)
    assert success is True


def test_avi_pipeline_error_message_mentions_avi(tmp_path: Path) -> None:
    """AVI success result must note that AVI does not support embedded cover art."""
    video_path = tmp_path / "sample.avi"
    _make_video(video_path, "avi")
    _, error_msg = _run_pipeline(video_path)
    assert error_msg is not None
    assert "AVI does not support" in error_msg


def test_avi_file_not_corrupted_after_pipeline(tmp_path: Path) -> None:
    """AVI file must remain playable (same duration) after the pipeline runs."""
    video_path = tmp_path / "sample.avi"
    _make_video(video_path, "avi")

    loader = PyAVVideoLoader()
    original = loader.load(str(video_path))

    _run_pipeline(video_path)

    updated = loader.load(str(video_path))
    assert abs(updated.duration_ms - original.duration_ms) < 500, (
        f"Duration changed from {original.duration_ms} ms to {updated.duration_ms} ms"
    )


def test_avi_atomicity_on_failure(tmp_path: Path) -> None:
    """If the AVI re-mux fails, the original file must be left intact."""
    video_path = tmp_path / "sample.avi"
    _make_video(video_path, "avi")
    original_bytes = video_path.read_bytes()

    with patch("subprocess.run", side_effect=subprocess.CalledProcessError(1, "ffmpeg")):
        loader = PyAVVideoLoader()
        extractor = PyAVFrameExtractor()
        writer = FormatDispatchThumbnailWriter()
        video = loader.load(str(video_path))
        frame = extractor.extract(video, TimelinePosition(offset_ms=2500))
        result = writer.write(video, frame)
        assert result.success is False

    assert video_path.read_bytes() == original_bytes, "AVI file was corrupted after failed write"


# ---------------------------------------------------------------------------
# FLV
# ---------------------------------------------------------------------------

def test_flv_pipeline_succeeds(tmp_path: Path) -> None:
    """FLV pipeline returns success=True (XDG cache only, no ffmpeg call)."""
    video_path = tmp_path / "sample.flv"
    _make_video(video_path, "flv")
    success, _ = _run_pipeline(video_path)
    assert success is True


def test_flv_pipeline_error_message_mentions_flv(tmp_path: Path) -> None:
    """FLV success result must note that FLV does not support embedded cover art."""
    video_path = tmp_path / "sample.flv"
    _make_video(video_path, "flv")
    _, error_msg = _run_pipeline(video_path)
    assert error_msg is not None
    assert "does not support" in error_msg


def test_flv_file_unchanged_after_pipeline(tmp_path: Path) -> None:
    """FLV file bytes must be unchanged (FLV path skips ffmpeg entirely)."""
    video_path = tmp_path / "sample.flv"
    _make_video(video_path, "flv")
    original_bytes = video_path.read_bytes()

    _run_pipeline(video_path)

    assert video_path.read_bytes() == original_bytes, "FLV file was unexpectedly modified"
