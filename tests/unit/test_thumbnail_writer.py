"""Unit tests for FormatDispatchThumbnailWriter."""

from __future__ import annotations

import os
import subprocess
from pathlib import Path
from unittest.mock import MagicMock, patch

import av
import pytest
from mutagen.mp4 import MP4
from PIL import Image, ImageStat

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
    def test_reencode_reason_uses_concat_eligibility(
        self,
        loader: PyAVVideoLoader,
        writer: FormatDispatchThumbnailWriter,
        sample_video: Path,
    ) -> None:
        video = loader.load(str(sample_video))
        assert writer.reencode_reason(video) is None

        with av.open(str(sample_video)) as container:
            disposition = container.streams.video[0].disposition
        stream = MagicMock()
        stream.disposition = disposition
        stream.codec_context.name = "vp9"
        stream.codec_context.codec.id = av.Codec("vp9", "r").id
        stream.codec_context.codec.long_name = "Google VP9"
        stream.codec_context.profile = "Main"
        stream.codec_context.format.name = "yuv420p"
        stream.width = 1920
        stream.height = 1080
        stream.average_rate = 60
        mocked_container = MagicMock()
        mocked_container.streams.video = [stream]
        mocked_container.__enter__.return_value = mocked_container
        with patch(
            "video_thumbnailer.core.thumbnail_writer.av.open",
            return_value=mocked_container,
        ):
            reason = writer.reencode_reason(video)

        assert reason is not None
        assert "VP9" in reason
        assert "H.264" in reason

    def test_av1_mp4_prepends_frame_without_reencoding_source(
        self,
        tmp_path: Path,
        loader: PyAVVideoLoader,
        writer: FormatDispatchThumbnailWriter,
    ) -> None:
        import imageio_ffmpeg

        path = tmp_path / "sample_av1.mp4"
        subprocess.run(
            [
                imageio_ffmpeg.get_ffmpeg_exe(), "-y",
                "-f", "lavfi", "-i", "testsrc=size=320x240:rate=25:duration=3",
                "-f", "lavfi", "-i", "sine=frequency=440:duration=3",
                "-c:v", "libaom-av1", "-cpu-used", "8", "-row-mt", "1",
                "-threads", "4", "-pix_fmt", "yuv420p", "-c:a", "aac",
                str(path),
            ],
            check=True, capture_output=True,
        )
        video = loader.load(str(path))
        assert writer.reencode_reason(video) is None
        source_first_mean = ImageStat.Stat(self._first_playable_frame(path)).mean
        with av.open(str(path)) as source:
            source_packets = [
                bytes(packet) for packet in source.demux(video=0) if packet.size
            ]

        stages: list[str] = []
        selected = Image.new("RGB", (320, 240), (240, 20, 20))
        result = writer.write(
            video, selected, position_ms=1000,
            on_progress=lambda name, elapsed: stages.append(name) if elapsed is None else None,
        )

        assert result.success, result.error_message
        assert result.duration_shift_ms == 40
        assert "Copying video streams" in stages
        assert "Encoding video" not in stages
        with av.open(str(path)) as output:
            assert output.streams.video[0].codec_context.codec.id == av.Codec("av1", "r").id
            output_packets = [
                bytes(packet) for packet in output.demux(video=0) if packet.size
            ]
        assert output_packets[1:] == source_packets
        with av.open(str(path)) as output:
            frames = list(output.decode(output.streams.video[0]))
            assert len(frames) == 76
            assert output.streams.audio[0].codec_context.name == "aac"
        first_original_mean = ImageStat.Stat(frames[1].to_image().convert("RGB")).mean
        assert all(
            abs(before - after) < 5
            for before, after in zip(source_first_mean, first_original_mean)
        )
        first_frame = self._first_playable_frame(path)
        mean = ImageStat.Stat(first_frame).mean
        assert mean[0] > mean[1] + 80
        assert mean[0] > mean[2] + 80
        reopened = loader.load(str(path))
        assert reopened.thumbnail_position_ms == 1040

    def _first_playable_frame(self, video_path: Path) -> Image.Image:
        with av.open(str(video_path)) as container:
            playable = [
                stream
                for stream in container.streams.video
                if not bool(stream.disposition & stream.disposition.attached_pic)
            ]
            assert playable, "Expected at least one playable video stream"
            for frame in container.decode(playable[0]):
                return frame.to_image().convert("RGB")  # type: ignore[no-untyped-call]
        pytest.fail("No playable frame decoded from video")

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

    def test_write_reports_timed_stages(
        self,
        loader: PyAVVideoLoader,
        writer: FormatDispatchThumbnailWriter,
        sample_video: Path,
        sample_pil_image: Image.Image,
    ) -> None:
        events: list[tuple[str, float | None]] = []

        result = writer.write(
            loader.load(str(sample_video)), sample_pil_image,
            on_progress=lambda name, elapsed: events.append((name, elapsed)),
        )

        assert result.success
        started = [name for name, elapsed in events if elapsed is None]
        completed = [name for name, elapsed in events if elapsed is not None]
        assert started == completed
        assert started[0] == "Preparing thumbnail"
        assert "Encoding preview frame" in started or "Encoding video" in started
        assert all(elapsed >= 0 for _, elapsed in events if elapsed is not None)

    def test_write_mp4_runs_ffmpeg_without_console(
        self,
        loader: PyAVVideoLoader,
        writer: FormatDispatchThumbnailWriter,
        sample_video: Path,
        sample_pil_image: Image.Image,
    ) -> None:
        video = loader.load(str(sample_video))

        with patch(
            "video_thumbnailer.core.thumbnail_writer.subprocess.run",
            wraps=subprocess.run,
        ) as run:
            result = writer.write(video, sample_pil_image)

        assert result.success is True
        assert run.call_count > 0
        expected_flags = subprocess.CREATE_NO_WINDOW if os.name == "nt" else 0
        assert all(
            call.kwargs["creationflags"] == expected_flags
            for call in run.call_args_list
        )

    def test_write_mp4_adds_covr_tag(
        self,
        loader: PyAVVideoLoader,
        writer: FormatDispatchThumbnailWriter,
        sample_video: Path,
        sample_pil_image: Image.Image,
    ) -> None:
        vf = loader.load(str(sample_video))
        result = writer.write(vf, sample_pil_image)

        assert result.success is True

        tags = MP4(str(sample_video)).tags
        assert tags is not None
        assert "covr" in tags
        assert len(tags["covr"]) == 1

    def test_write_mp4_saves_selected_frame_source(
        self,
        loader: PyAVVideoLoader,
        writer: FormatDispatchThumbnailWriter,
        sample_video: Path,
        sample_pil_image: Image.Image,
    ) -> None:
        video = loader.load(str(sample_video))

        result = writer.write(video, sample_pil_image, position_ms=1680)

        assert result.success is True
        reopened = loader.load(str(sample_video))
        # The concat fast path prepends a frame, shifting every original frame
        # (including the one selected) forward by duration_shift_ms. Use the
        # pre-write frame_step_ms: reopening can recompute a slightly different
        # average frame rate once an extra frame has been prepended.
        expected_position_ms = 1680 + result.duration_shift_ms
        assert reopened.thumbnail_position_ms == expected_position_ms
        assert reopened.thumbnail_frame_number == round(
            expected_position_ms / video.frame_step_ms
        )

    def test_write_mp4_sets_selected_frame_as_first_video_frame(
        self,
        loader: PyAVVideoLoader,
        writer: FormatDispatchThumbnailWriter,
        sample_video: Path,
    ) -> None:
        vf = loader.load(str(sample_video))
        with av.open(str(sample_video)) as container:
            source_packets = [
                bytes(packet)
                for packet in container.demux(video=0)
                if packet.size > 0
            ]
        selected = Image.new("RGB", (320, 240), color=(240, 20, 20))

        result = writer.write(vf, selected)

        assert result.success is True

        first_frame = self._first_playable_frame(sample_video)
        mean = ImageStat.Stat(first_frame).mean
        assert mean[0] > 150
        assert mean[0] > mean[1] + 80
        assert mean[0] > mean[2] + 80

        with av.open(str(sample_video)) as container:
            playable = [
                stream
                for stream in container.streams.video
                if not bool(stream.disposition & stream.disposition.attached_pic)
            ]
            assert playable
            assert sum(1 for _ in container.decode(playable[0])) >= 100

        with av.open(str(sample_video)) as container:
            output_packets = [
                bytes(packet)
                for packet in container.demux(video=0)
                if packet.size > 0
            ]
        assert output_packets[2:] == source_packets[1:]

        with av.open(str(sample_video)) as container:
            audio_streams = container.streams.audio
            assert audio_streams
            audio_packets = [
                packet for packet in container.demux(audio_streams[0])
                if packet.size > 0
            ]
            assert audio_packets
            assert sum(len(packet.decode()) for packet in audio_packets) > 0

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
