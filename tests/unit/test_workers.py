"""Unit tests for Qt worker runnables (T039 coverage boost)."""

from __future__ import annotations

from unittest.mock import MagicMock

from PIL import Image

from video_thumbnailer.core.frame_extractor import PyAVFrameExtractor
from video_thumbnailer.core.thumbnail_writer import FormatDispatchThumbnailWriter
from video_thumbnailer.core.video_loader import PyAVVideoLoader
from video_thumbnailer.models import (
    ApplyResult,
    TimelinePosition,
    VideoFile,
    VideoFormat,
)
from video_thumbnailer.ui.worker import ApplyWorker, FrameExtractWorker, VideoLoadWorker


def _make_vf() -> VideoFile:
    return VideoFile(
        path="/fake/video.mp4",
        format=VideoFormat.MP4,
        duration_ms=5000,
        width=320,
        height=240,
        existing_thumbnail=None,
        is_writable=True,
    )


# ---------------------------------------------------------------------------
# Helpers: run a QRunnable synchronously and capture signals
# ---------------------------------------------------------------------------

def _run_worker(worker):
    """Execute worker.run() synchronously and return captured signal values."""
    finished_values = []
    error_values = []
    worker.signals.finished.connect(lambda v: finished_values.append(v))
    worker.signals.error.connect(lambda v: error_values.append(v))
    worker.run()
    return finished_values, error_values


# ---------------------------------------------------------------------------
# VideoLoadWorker
# ---------------------------------------------------------------------------

class TestVideoLoadWorker:
    def test_run_success_emits_finished(self) -> None:
        loader = MagicMock(spec=PyAVVideoLoader)
        vf = _make_vf()
        loader.load.return_value = vf

        worker = VideoLoadWorker(loader, "/fake/video.mp4")
        finished, errors = _run_worker(worker)

        assert len(finished) == 1
        assert finished[0] is vf
        assert errors == []

    def test_run_error_emits_error_string(self) -> None:
        loader = MagicMock(spec=PyAVVideoLoader)
        loader.load.side_effect = FileNotFoundError("file not found")

        worker = VideoLoadWorker(loader, "/no/such/file.mp4")
        finished, errors = _run_worker(worker)

        assert finished == []
        assert len(errors) == 1
        assert "file not found" in errors[0]


# ---------------------------------------------------------------------------
# FrameExtractWorker
# ---------------------------------------------------------------------------

class TestFrameExtractWorker:
    def test_run_success_emits_finished(self) -> None:
        extractor = MagicMock(spec=PyAVFrameExtractor)
        img = Image.new("RGB", (10, 10))
        extractor.extract.return_value = img
        vf = _make_vf()
        pos = TimelinePosition(offset_ms=2000)

        worker = FrameExtractWorker(extractor, vf, pos)
        finished, errors = _run_worker(worker)

        assert len(finished) == 1
        assert finished[0] is img
        assert errors == []

    def test_run_error_emits_error_string(self) -> None:
        from video_thumbnailer.exceptions import ExtractionError

        extractor = MagicMock(spec=PyAVFrameExtractor)
        extractor.extract.side_effect = ExtractionError("/fake.mp4", 2000, "codec fail")
        vf = _make_vf()
        pos = TimelinePosition(offset_ms=2000)

        worker = FrameExtractWorker(extractor, vf, pos)
        finished, errors = _run_worker(worker)

        assert finished == []
        assert len(errors) == 1
        assert "codec fail" in errors[0]


# ---------------------------------------------------------------------------
# ApplyWorker
# ---------------------------------------------------------------------------

class TestApplyWorker:
    def test_run_success_calls_invalidator(self) -> None:
        writer = MagicMock(spec=FormatDispatchThumbnailWriter)
        writer.write.return_value = ApplyResult(success=True)
        invalidator = MagicMock()
        vf = _make_vf()
        thumbnail = Image.new("RGB", (10, 10))

        worker = ApplyWorker(writer, invalidator, vf, thumbnail)
        finished, errors = _run_worker(worker)

        invalidator.invalidate.assert_called_once_with(vf)
        assert len(finished) == 1
        assert finished[0].success is True
        assert errors == []

    def test_run_failure_result_skips_invalidator(self) -> None:
        writer = MagicMock(spec=FormatDispatchThumbnailWriter)
        from video_thumbnailer.models import ApplyError
        writer.write.return_value = ApplyResult(
            success=False,
            error_code=ApplyError.FFMPEG_ERROR,
            error_message="ffmpeg failed",
        )
        invalidator = MagicMock()
        vf = _make_vf()
        thumbnail = Image.new("RGB", (10, 10))

        worker = ApplyWorker(writer, invalidator, vf, thumbnail)
        finished, errors = _run_worker(worker)

        invalidator.invalidate.assert_not_called()
        assert len(finished) == 1
        assert finished[0].success is False

    def test_run_exception_emits_error_string(self) -> None:
        writer = MagicMock(spec=FormatDispatchThumbnailWriter)
        writer.write.side_effect = RuntimeError("unexpected error")
        invalidator = MagicMock()
        vf = _make_vf()
        thumbnail = Image.new("RGB", (10, 10))

        worker = ApplyWorker(writer, invalidator, vf, thumbnail)
        finished, errors = _run_worker(worker)

        assert finished == []
        assert len(errors) == 1
        assert "unexpected error" in errors[0]
