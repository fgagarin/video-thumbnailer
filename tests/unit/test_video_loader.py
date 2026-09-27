"""Unit tests for PyAVVideoLoader."""

from __future__ import annotations

import os
from pathlib import Path

import pytest
from mutagen.mp4 import MP4, MP4FreeForm

from video_thumbnailer.core.video_loader import PyAVVideoLoader
from video_thumbnailer.exceptions import UnsupportedFormatError
from video_thumbnailer.models import VideoFile, VideoFormat


@pytest.fixture()
def loader() -> PyAVVideoLoader:
    return PyAVVideoLoader()


class TestVideoLoaderBasic:
    def test_loads_saved_thumbnail_source_metadata(
        self, loader: PyAVVideoLoader, sample_video: Path
    ) -> None:
        tags = MP4(str(sample_video))
        tags.tags = tags.tags or {}
        tags.tags[
            "----:com.apple.iTunes:video-thumbnailer-frame-number"
        ] = [MP4FreeForm(b"42")]
        tags.tags[
            "----:com.apple.iTunes:video-thumbnailer-position-ms"
        ] = [MP4FreeForm(b"1680")]
        tags.save()

        video = loader.load(str(sample_video))

        assert video.thumbnail_frame_number == 42
        assert video.thumbnail_position_ms == 1680

    def test_load_mp4_returns_video_file(self, loader: PyAVVideoLoader, sample_video: Path) -> None:
        vf = loader.load(str(sample_video))
        assert isinstance(vf, VideoFile)
        assert vf.format == VideoFormat.MP4
        assert vf.duration_ms > 0
        assert vf.width == 320
        assert vf.height == 240

    def test_load_mp4_no_cover_art(self, loader: PyAVVideoLoader, sample_video: Path) -> None:
        vf = loader.load(str(sample_video))
        assert vf.existing_thumbnail is None

    def test_load_mp4_with_cover_art(
        self, loader: PyAVVideoLoader, video_with_thumb: Path
    ) -> None:
        vf = loader.load(str(video_with_thumb))
        assert vf.existing_thumbnail is not None

    def test_load_read_only_is_writable_false(
        self, loader: PyAVVideoLoader, read_only_video: Path
    ) -> None:
        vf = loader.load(str(read_only_video))
        assert vf.is_writable is False

    def test_load_writable_is_writable_true(
        self, loader: PyAVVideoLoader, sample_video: Path
    ) -> None:
        vf = loader.load(str(sample_video))
        assert vf.is_writable is True

    def test_load_nonexistent_raises(self, loader: PyAVVideoLoader, tmp_path: Path) -> None:
        with pytest.raises(FileNotFoundError):
            loader.load(str(tmp_path / "ghost.mp4"))

    def test_load_text_file_raises_unsupported(
        self, loader: PyAVVideoLoader, tmp_path: Path
    ) -> None:
        bad = tmp_path / "not_a_video.txt"
        bad.write_text("hello")
        with pytest.raises((UnsupportedFormatError, Exception)):
            loader.load(str(bad))

    def test_load_mov_format(self, loader: PyAVVideoLoader, tmp_path: Path) -> None:
        """Generate a small MOV and verify format detection."""
        import subprocess

        import imageio_ffmpeg

        out = tmp_path / "sample.mov"
        subprocess.run(
            [
                imageio_ffmpeg.get_ffmpeg_exe(), "-y",
                "-f", "lavfi", "-i", "testsrc=size=320x240:rate=25:duration=3",
                "-f", "lavfi", "-i", "sine=frequency=440:duration=3",
                "-c:v", "libx264", "-c:a", "aac", "-pix_fmt", "yuv420p",
                str(out),
            ],
            check=True,
            capture_output=True,
        )
        vf = loader.load(str(out))
        assert vf.format == VideoFormat.MOV

    def test_path_is_absolute_in_result(
        self, loader: PyAVVideoLoader, sample_video: Path
    ) -> None:
        vf = loader.load(str(sample_video))
        assert os.path.isabs(vf.path)
