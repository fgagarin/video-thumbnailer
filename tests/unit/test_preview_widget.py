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
        assert (
            preview._candidate_title.parentWidget().geometry().x()
            < preview._current_title.parentWidget().geometry().x()
        )

    def test_set_current_thumbnail_none_shows_placeholder(
        self, preview: PreviewWidget
    ) -> None:
        preview.set_current_thumbnail(None)
        text = preview._current_label.text()
        assert "Save thumbnail" in text

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

    def test_candidate_frame_shows_selected_timestamp(
        self, preview: PreviewWidget, pil_image: Image.Image
    ) -> None:
        preview.set_candidate_frame(pil_image, 61_234)

        assert preview._candidate_title.text() == "Frame (1:01.234)"

    def test_save_button_is_compact_blue_and_at_bottom_of_thumbnail_panel(
        self, preview: PreviewWidget
    ) -> None:
        button = preview.apply_button
        thumbnail_panel = preview._current_title.parentWidget()
        thumbnail_layout = preview._thumbnail_column.layout()
        assert button.text() == "Save thumbnail"
        assert "selected frame" in button.toolTip().lower()
        assert button.width() == 176
        assert button.sizePolicy().horizontalPolicy().name == "Fixed"
        assert "#0078d4" in button.styleSheet()
        assert thumbnail_layout.indexOf(button) == thumbnail_layout.count() - 1
        assert thumbnail_panel.geometry().bottom() < button.geometry().top()
        assert thumbnail_panel.geometry().x() == button.geometry().x()

        icon_pixmap = button.icon().pixmap(32, 32)
        colors = icon_pixmap.toImage()
        green_pixels = [
            colors.pixelColor(x, y)
            for y in range(colors.height())
            for x in range(colors.width())
            if colors.pixelColor(x, y).alpha() > 0
        ]
        assert green_pixels
        assert all(
            color.red() > 230
            and color.green() > 230
            and color.blue() > 230
            for color in green_pixels
        )

    def test_clear_resets_both_panels(
        self, preview: PreviewWidget, pil_image: Image.Image
    ) -> None:
        preview.set_current_thumbnail(pil_image)
        preview.set_candidate_frame(pil_image)
        preview.clear()
        assert "Save thumbnail" in preview._current_label.text()
        assert "No frame selected" in preview._candidate_label.text()
        assert preview._candidate_title.text() == "Frame"
        assert preview._current_title.text() == "Thumbnail"

    def test_size_hint(self, preview: PreviewWidget) -> None:
        assert preview.sizeHint() == QSize(720, 220)

    def test_candidate_image_scales_with_widget_and_keeps_aspect_ratio(
        self, preview: PreviewWidget, qtbot
    ) -> None:
        image = Image.new("RGB", (1920, 1080))
        preview.resize(760, 300)
        preview.set_candidate_frame(image)
        preview.show()
        qtbot.wait(50)

        large_pixmap = preview._candidate_label.pixmap()
        assert large_pixmap is not None
        large_ratio = large_pixmap.width() / large_pixmap.height()
        assert abs(large_ratio - 16 / 9) < 0.02

        preview.resize(460, 260)
        qtbot.wait(50)
        small_pixmap = preview._candidate_label.pixmap()
        assert small_pixmap is not None
        assert small_pixmap.width() < large_pixmap.width()
        assert abs(small_pixmap.width() / small_pixmap.height() - 16 / 9) < 0.02

    def test_thumbnail_image_is_smaller_than_frame(
        self, preview: PreviewWidget, pil_image: Image.Image, qtbot
    ) -> None:
        preview.resize(760, 300)
        preview.set_candidate_frame(pil_image)
        preview.set_current_thumbnail(pil_image)
        qtbot.wait(50)

        frame_pixmap = preview._candidate_label.pixmap()
        thumbnail_pixmap = preview._current_label.pixmap()
        assert frame_pixmap is not None and thumbnail_pixmap is not None
        assert thumbnail_pixmap.width() < frame_pixmap.width()
        assert thumbnail_pixmap.height() < frame_pixmap.height()
