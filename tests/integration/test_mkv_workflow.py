"""Integration tests for MKV and WebM thumbnail embedding (T035)."""

from __future__ import annotations

import subprocess
import time
from pathlib import Path

import av
import imageio_ffmpeg
from PIL import Image

from video_thumbnailer.core.frame_extractor import PyAVFrameExtractor
from video_thumbnailer.core.thumbnail_writer import FormatDispatchThumbnailWriter
from video_thumbnailer.core.video_loader import PyAVVideoLoader
from video_thumbnailer.models import TimelinePosition

_FFMPEG = imageio_ffmpeg.get_ffmpeg_exe()


def _make_video(output: Path, fmt: str) -> None:
    if fmt in ("mkv",):
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
    elif fmt == "webm":
        subprocess.run(
            [
                _FFMPEG, "-y",
                "-f", "lavfi", "-i", "testsrc=size=320x240:rate=25:duration=5",
                "-f", "lavfi", "-i", "sine=frequency=440:duration=5",
                "-c:v", "libvpx-vp9", "-c:a", "libopus", "-b:v", "200k",
                str(output),
            ],
            check=True, capture_output=True,
        )


def _run_pipeline(video_path: Path, sample_image: Image.Image) -> tuple[bool, str | None]:
    """Run the full load → extract → write pipeline; return (success, error_msg)."""
    loader = PyAVVideoLoader()
    extractor = PyAVFrameExtractor()
    writer = FormatDispatchThumbnailWriter()

    video = loader.load(str(video_path))
    frame = extractor.extract(video, TimelinePosition(offset_ms=2500))
    result = writer.write(video, frame)
    return result.success, result.error_message


def _find_cover_attachment(video_path: Path) -> bool:
    """Return True if the file has a cover art attachment (any representation).

    PyAV surfaces MKV/WebM ``-attach`` streams as ``type=video`` streams
    with ``attached_pic`` disposition and ``filename`` metadata — not as
    ``type=attachment`` or ``type=data``.  We therefore accept either
    representation.
    """
    with av.open(str(video_path)) as container:
        for stream in container.streams:
            fname = stream.metadata.get("filename", "").lower()
            # MKV -attach: appears as video stream with attached_pic disposition
            if stream.type == "video":
                try:
                    if bool(stream.disposition & stream.disposition.attached_pic):
                        if fname in ("cover.jpg", "cover.png", "cover.jpeg"):
                            return True
                except Exception:
                    pass
            # Fallback: explicit attachment/data streams
            if stream.type in ("attachment", "data"):
                if fname in ("cover.jpg", "cover.png", "cover.jpeg"):
                    return True
    return False


# ---------------------------------------------------------------------------
# MKV
# ---------------------------------------------------------------------------

def test_mkv_full_pipeline_embeds_cover(tmp_path: Path) -> None:
    """Full pipeline embeds a JPEG attachment into an MKV file."""
    video_path = tmp_path / "sample.mkv"
    _make_video(video_path, "mkv")

    t0 = time.monotonic()
    success, _ = _run_pipeline(video_path, Image.new("RGB", (320, 240), color=(100, 150, 200)))
    elapsed_ms = (time.monotonic() - t0) * 1000

    assert success is True
    assert _find_cover_attachment(video_path), "No cover attachment found in MKV output"
    assert elapsed_ms < 5000, f"Pipeline took {elapsed_ms:.0f} ms (limit 5000 ms)"


def test_mkv_duration_preserved_after_embed(tmp_path: Path) -> None:
    """The MKV file duration must remain unchanged after thumbnail embedding."""
    video_path = tmp_path / "sample.mkv"
    _make_video(video_path, "mkv")

    loader = PyAVVideoLoader()
    original = loader.load(str(video_path))

    _run_pipeline(video_path, Image.new("RGB", (320, 240)))

    updated = loader.load(str(video_path))
    # Allow ±500 ms tolerance (container re-mux can shift duration slightly)
    assert abs(updated.duration_ms - original.duration_ms) < 500, (
        f"Duration changed from {original.duration_ms} ms to {updated.duration_ms} ms"
    )


# ---------------------------------------------------------------------------
# WebM
# ---------------------------------------------------------------------------

def test_webm_full_pipeline_embeds_cover(tmp_path: Path) -> None:
    """WebM does not support attachments; the pipeline returns success=True
    with an informational message instead of embedding cover art."""
    video_path = tmp_path / "sample.webm"
    _make_video(video_path, "webm")
    original_bytes = video_path.read_bytes()

    t0 = time.monotonic()
    success, error_msg = _run_pipeline(video_path, Image.new("RGB", (320, 240), color=(200, 100, 50)))
    elapsed_ms = (time.monotonic() - t0) * 1000

    assert success is True
    assert error_msg is not None
    assert "WEBM" in error_msg or "WebM" in error_msg or "does not support" in error_msg
    # File must be unchanged (no ffmpeg ran)
    assert video_path.read_bytes() == original_bytes
    assert elapsed_ms < 5000, f"Pipeline took {elapsed_ms:.0f} ms (limit 5000 ms)"


def test_webm_duration_preserved_after_embed(tmp_path: Path) -> None:
    """The WebM file duration must remain unchanged after thumbnail embedding."""
    video_path = tmp_path / "sample.webm"
    _make_video(video_path, "webm")

    loader = PyAVVideoLoader()
    original = loader.load(str(video_path))

    _run_pipeline(video_path, Image.new("RGB", (320, 240)))

    updated = loader.load(str(video_path))
    assert abs(updated.duration_ms - original.duration_ms) < 500, (
        f"Duration changed from {original.duration_ms} ms to {updated.duration_ms} ms"
    )
