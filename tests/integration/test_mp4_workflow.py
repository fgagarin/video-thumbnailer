"""Integration tests for end-to-end MP4/MOV workflow."""

from __future__ import annotations

import shutil
import time
from pathlib import Path

import av
import pytest

from video_thumbnailer.core.frame_extractor import PyAVFrameExtractor
from video_thumbnailer.core.thumbnail_writer import FormatDispatchThumbnailWriter
from video_thumbnailer.core.video_loader import PyAVVideoLoader


@pytest.fixture()
def loader() -> PyAVVideoLoader:
    return PyAVVideoLoader()


@pytest.fixture()
def extractor() -> PyAVFrameExtractor:
    return PyAVFrameExtractor()


@pytest.fixture()
def writer() -> FormatDispatchThumbnailWriter:
    return FormatDispatchThumbnailWriter()


def _has_attached_pic_stream(video_path: Path) -> bool:
    container = av.open(str(video_path))
    try:
        for stream in container.streams:
            if stream.type == "video" and bool(stream.disposition & stream.disposition.attached_pic):
                return True
        return False
    finally:
        container.close()


def _generate_mov(tmp_path: Path) -> Path:
    """Generate a small MOV file for testing."""
    import subprocess

    import imageio_ffmpeg

    out = tmp_path / "sample.mov"
    subprocess.run(
        [
            imageio_ffmpeg.get_ffmpeg_exe(), "-y",
            "-f", "lavfi", "-i", "testsrc=size=320x240:rate=25:duration=5",
            "-f", "lavfi", "-i", "sine=frequency=440:duration=5",
            "-c:v", "libx264", "-c:a", "aac", "-pix_fmt", "yuv420p", "-ar", "44100",
            str(out),
        ],
        check=True, capture_output=True,
    )
    return out


class TestMp4Workflow:
    def test_mp4_full_pipeline_success(
        self,
        loader: PyAVVideoLoader,
        extractor: PyAVFrameExtractor,
        writer: FormatDispatchThumbnailWriter,
        sample_video: Path,
        tmp_path: Path,
    ) -> None:
        # Copy to tmp_path so modifications don't affect the fixture
        target = tmp_path / "test.mp4"
        shutil.copy2(str(sample_video), str(target))

        start = time.monotonic()
        vf = loader.load(str(target))
        frame = extractor.extract(vf, type("Pos", (), {"offset_ms": 2500})())
        result = writer.write(vf, frame)
        elapsed = (time.monotonic() - start) * 1000

        assert result.success is True
        assert elapsed < 10_000, f"Pipeline took {elapsed:.0f}ms (limit: 10000ms)"
        assert _has_attached_pic_stream(target), "MP4 has no attached_pic after write"

    def test_mov_full_pipeline_success(
        self,
        loader: PyAVVideoLoader,
        extractor: PyAVFrameExtractor,
        writer: FormatDispatchThumbnailWriter,
        tmp_path: Path,
    ) -> None:
        mov_path = _generate_mov(tmp_path)

        start = time.monotonic()
        vf = loader.load(str(mov_path))
        frame = extractor.extract(vf, type("Pos", (), {"offset_ms": 2500})())
        result = writer.write(vf, frame)
        elapsed = (time.monotonic() - start) * 1000

        assert result.success is True
        assert elapsed < 10_000, f"Pipeline took {elapsed:.0f}ms (limit: 10000ms)"
        assert _has_attached_pic_stream(mov_path), "MOV has no attached_pic after write"

    def test_pipeline_under_3_seconds(
        self,
        loader: PyAVVideoLoader,
        extractor: PyAVFrameExtractor,
        writer: FormatDispatchThumbnailWriter,
        sample_video: Path,
        tmp_path: Path,
    ) -> None:
        target = tmp_path / "test.mp4"
        shutil.copy2(str(sample_video), str(target))

        start = time.monotonic()
        vf = loader.load(str(target))
        frame = extractor.extract(vf, type("Pos", (), {"offset_ms": 0})())
        writer.write(vf, frame)
        elapsed = (time.monotonic() - start) * 1000

        assert elapsed < 3000, f"Full pipeline took {elapsed:.0f}ms (limit: 3000ms)"
