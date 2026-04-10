"""Unit tests for FormatDispatchThumbnailWriter."""

from __future__ import annotations

from pathlib import Path

import av
import pytest
from PIL import Image

from video_thumbnailer.core.thumbnail_writer import FormatDispatchThumbnailWriter
from video_thumbnailer.core.video_loader import PyAVVideoLoader
from video_thumbnailer.models import ApplyError, VideoFile, VideoFormat


@pytest.fixture()
def loader() -> PyAVVideoLoader:
    return PyAVVideoLoader()


@pytest.fixture()
def writer() -> FormatDispatchThumbnailWriter:
    return FormatDispatchThumbnailWriter()


def _make_flv_video_file(path: Path) -> VideoFile:
    """Return a VideoFile stub with FLV format pointing at a real file."""
    import subprocess

    import imageio_ffmpeg

    out = path / "sample.flv"
    subprocess.run(
        [
            imageio_ffmpeg.get_ffmpeg_exe(), "-y",
            "-f", "lavfi", "-i", "testsrc=size=320x240:rate=25:duration=3",
            "-f", "lavfi", "-i", "sine=frequency=440:duration=3",
            "-c:v", "libx264", "-c:a", "aac", "-pix_fmt", "yuv420p", "-ar", "44100",
            str(out),
        ],
        check=True, capture_output=True,
    )
    return VideoFile(
        path=str(out),
        format=VideoFormat.FLV,
        duration_ms=3000,
        width=320,
        height=240,
        existing_thumbnail=None,
        is_writable=True,
    )


class TestThumbnailWriter:
    def test_write_mp4_success(
        self,
        loader: PyAVVideoLoader,
        writer: FormatDispatchThumbnailWriter,
        sample_video: Path,
        sample_pil_image: Image.Image,
    ) -> None:
        vf = loader.load(str(sample_video))
        result = writer.write(vf, sample_pil_image)
        assert result.success is True
        assert result.elapsed_ms >= 0

    def test_write_read_only_returns_error(
        self,
        loader: PyAVVideoLoader,
        writer: FormatDispatchThumbnailWriter,
        read_only_video: Path,
        sample_pil_image: Image.Image,
    ) -> None:
        vf = loader.load(str(read_only_video))
        result = writer.write(vf, sample_pil_image)
        assert result.success is False
        assert result.error_code == ApplyError.FILE_NOT_WRITABLE

    def test_write_read_only_file_unchanged(
        self,
        loader: PyAVVideoLoader,
        writer: FormatDispatchThumbnailWriter,
        read_only_video: Path,
        sample_pil_image: Image.Image,
    ) -> None:
        original_bytes = read_only_video.read_bytes()
        vf = loader.load(str(read_only_video))
        writer.write(vf, sample_pil_image)
        assert read_only_video.read_bytes() == original_bytes

    def test_write_unsupported_format_returns_error(
        self,
        writer: FormatDispatchThumbnailWriter,
        tmp_path: Path,
        sample_pil_image: Image.Image,
    ) -> None:
        # Use VideoFormat.UNSUPPORTED — the format check fires before any file I/O
        unsupported_vf = VideoFile(
            path=str(tmp_path / "placeholder.rm"),
            format=VideoFormat.UNSUPPORTED,
            duration_ms=3000,
            width=320,
            height=240,
            existing_thumbnail=None,
            is_writable=True,
        )
        result = writer.write(unsupported_vf, sample_pil_image)
        assert result.success is False
        assert result.error_code == ApplyError.UNSUPPORTED_FORMAT

    def test_write_thumbnail_is_scaled(
        self,
        loader: PyAVVideoLoader,
        writer: FormatDispatchThumbnailWriter,
        sample_video: Path,
    ) -> None:
        # Start with a large thumbnail (4K-ish) and verify it gets downscaled
        large_img = Image.new("RGB", (3840, 2160), color=(255, 0, 0))
        vf = loader.load(str(sample_video))
        result = writer.write(vf, large_img)
        assert result.success is True

        # Verify the embedded cover art is within 640×360
        container = av.open(str(sample_video))
        try:
            for stream in container.streams:
                if stream.type == "video" and bool(stream.disposition & stream.disposition.attached_pic):
                    for packet in container.demux(stream):
                        if packet.size == 0:
                            continue
                        import io

                        cover = Image.open(io.BytesIO(bytes(packet)))
                        assert cover.width <= 640
                        assert cover.height <= 360
                        return
        finally:
            container.close()
        pytest.fail("No attached_pic stream found after write")
