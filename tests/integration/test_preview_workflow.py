"""Integration tests for the PreviewWidget with real video data."""

from __future__ import annotations

from pathlib import Path

import pytest

from video_thumbnailer.core.frame_extractor import PyAVFrameExtractor
from video_thumbnailer.core.video_loader import PyAVVideoLoader
from video_thumbnailer.models import TimelinePosition
from video_thumbnailer.ui.preview_widget import PreviewWidget


@pytest.fixture()
def loader() -> PyAVVideoLoader:
    return PyAVVideoLoader()


@pytest.fixture()
def extractor() -> PyAVFrameExtractor:
    return PyAVFrameExtractor()


@pytest.fixture()
def preview(qtbot) -> PreviewWidget:  # type: ignore[type-arg]
    widget = PreviewWidget()
    qtbot.addWidget(widget)
    widget.show()
    return widget


class TestPreviewWorkflow:
    def test_video_with_thumb_populates_left_panel(
        self,
        loader: PyAVVideoLoader,
        preview: PreviewWidget,
        video_with_thumb: Path,
    ) -> None:
        vf = loader.load(str(video_with_thumb))
        preview.set_current_thumbnail(vf.existing_thumbnail)
        pixmap = preview._current_label.pixmap()
        assert pixmap is not None, "Expected existing_thumbnail to be found"
        assert not pixmap.isNull(), "Left panel pixmap should not be null for video with thumb"

    def test_scrub_populates_right_panel(
        self,
        loader: PyAVVideoLoader,
        extractor: PyAVFrameExtractor,
        preview: PreviewWidget,
        sample_video: Path,
    ) -> None:
        vf = loader.load(str(sample_video))
        frame = extractor.extract(vf, TimelinePosition(offset_ms=3000))
        preview.set_candidate_frame(frame)
        pixmap = preview._candidate_label.pixmap()
        assert pixmap is not None
        assert not pixmap.isNull(), "Right panel pixmap should not be null after scrub"

    def test_both_panels_populated_simultaneously(
        self,
        loader: PyAVVideoLoader,
        extractor: PyAVFrameExtractor,
        preview: PreviewWidget,
        video_with_thumb: Path,
    ) -> None:
        vf = loader.load(str(video_with_thumb))
        preview.set_current_thumbnail(vf.existing_thumbnail)
        frame = extractor.extract(vf, TimelinePosition(offset_ms=2000))
        preview.set_candidate_frame(frame)
        # Both panels populated without clearing each other
        assert not (preview._current_label.pixmap() or preview._current_label.pixmap()).isNull()
        assert not preview._candidate_label.pixmap().isNull()

    def test_video_without_thumb_shows_placeholder(
        self,
        loader: PyAVVideoLoader,
        preview: PreviewWidget,
        sample_video: Path,
    ) -> None:
        vf = loader.load(str(sample_video))
        assert vf.existing_thumbnail is None, "sample_video should have no thumb"
        preview.set_current_thumbnail(vf.existing_thumbnail)
        text = preview._current_label.text()
        assert "No current thumbnail" in text
