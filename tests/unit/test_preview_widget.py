"""Unit tests for PreviewWidget."""

from __future__ import annotations

import pytest
from PIL import Image
from PySide6.QtCore import QSize

from video_thumbnailer.ui.preview_widget import PreviewWidget


@pytest.fixture()
def preview(qtbot) -> PreviewWidget:  # type: ignore[type-arg]
    widget = PreviewWidget()
    qtbot.addWidget(widget)
    widget.show()
    return widget


@pytest.fixture()
def pil_image() -> Image.Image:
    return Image.new("RGB", (320, 240), color=(100, 150, 200))


class TestPreviewWidget:
    def test_both_panels_visible_after_construction(self, preview: PreviewWidget) -> None:
        assert preview.isVisible()
        # Both internal labels should exist
        assert preview._current_label is not None
        assert preview._candidate_label is not None

    def test_set_current_thumbnail_none_shows_placeholder(
        self, preview: PreviewWidget
    ) -> None:
        preview.set_current_thumbnail(None)
        text = preview._current_label.text()
        assert "No current thumbnail" in text

    def test_set_current_thumbnail_image_shows_pixmap(
        self, preview: PreviewWidget, pil_image: Image.Image
    ) -> None:
        preview.set_current_thumbnail(pil_image)
        pixmap = preview._current_label.pixmap()
        assert pixmap is not None
        assert not pixmap.isNull()

    def test_set_candidate_frame_shows_pixmap(
        self, preview: PreviewWidget, pil_image: Image.Image
    ) -> None:
        preview.set_candidate_frame(pil_image)
        pixmap = preview._candidate_label.pixmap()
        assert pixmap is not None
        assert not pixmap.isNull()

    def test_clear_resets_both_panels(
        self, preview: PreviewWidget, pil_image: Image.Image
    ) -> None:
        preview.set_current_thumbnail(pil_image)
        preview.set_candidate_frame(pil_image)
        preview.clear()
        assert "No current thumbnail" in preview._current_label.text()
        assert "No frame selected" in preview._candidate_label.text()

    def test_size_hint(self, preview: PreviewWidget) -> None:
        assert preview.sizeHint() == QSize(540, 165)
