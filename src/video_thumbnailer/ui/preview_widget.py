"""PreviewWidget: side-by-side current thumbnail / selected frame display."""

from __future__ import annotations

from typing import TYPE_CHECKING

from PySide6.QtCore import QSize, Qt
from PySide6.QtGui import QPixmap
from PySide6.QtWidgets import QFrame, QHBoxLayout, QLabel, QVBoxLayout, QWidget

if TYPE_CHECKING:
    from PIL.Image import Image as PILImage

__all__ = ["PreviewWidget"]

_THUMB_W = 240
_THUMB_H = 135


def _pil_to_pixmap(image: PILImage) -> QPixmap:
    """Convert a PIL Image to a QPixmap (via QImage)."""
    from PIL.ImageQt import toqimage

    img_rgb = image.convert("RGB")
    qimage = toqimage(img_rgb)
    return QPixmap.fromImage(qimage)


class PreviewWidget(QWidget):
    """Two-panel widget showing current embedded thumbnail and candidate frame.

    Layout::

        ┌──────────────────────────┬──────────────────────────┐
        │   Current Thumbnail      │   Selected Frame         │
        │  ┌────────────────────┐  │  ┌────────────────────┐  │
        │  │  <image or text>   │  │  │  <image or text>   │  │
        │  └────────────────────┘  │  └────────────────────┘  │
        └──────────────────────────┴──────────────────────────┘
    """

    def __init__(self, parent: QWidget | None = None) -> None:
        super().__init__(parent)

        layout = QHBoxLayout(self)
        layout.setContentsMargins(4, 4, 4, 4)
        layout.setSpacing(8)

        self._current_label = self._make_panel(layout, "Current Thumbnail")
        self._candidate_label = self._make_panel(layout, "Selected Frame")

        self._show_placeholder(self._current_label, "No current thumbnail")
        self._show_placeholder(self._candidate_label, "No frame selected")

    # ------------------------------------------------------------------
    # Public API
    # ------------------------------------------------------------------

    def set_current_thumbnail(self, image: PILImage | None) -> None:
        """Display the current embedded thumbnail, or placeholder if None."""
        if image is None:
            self._show_placeholder(self._current_label, "No current thumbnail")
        else:
            self._show_pixmap(self._current_label, image)

    def set_candidate_frame(self, image: PILImage) -> None:
        """Display a selected video frame as the candidate thumbnail."""
        self._show_pixmap(self._candidate_label, image)

    def clear(self) -> None:
        """Reset both panels to their placeholder states."""
        self._show_placeholder(self._current_label, "No current thumbnail")
        self._show_placeholder(self._candidate_label, "No frame selected")

    def sizeHint(self) -> QSize:
        return QSize(540, 165)

    # ------------------------------------------------------------------
    # Private helpers
    # ------------------------------------------------------------------

    def _make_panel(self, parent_layout: QHBoxLayout, title: str) -> QLabel:
        frame = QFrame()
        frame.setFrameShape(QFrame.Shape.StyledPanel)
        vbox = QVBoxLayout(frame)
        vbox.setContentsMargins(4, 4, 4, 4)
        vbox.setSpacing(2)

        title_label = QLabel(title)
        title_label.setAlignment(Qt.AlignmentFlag.AlignHCenter)
        vbox.addWidget(title_label)

        img_label = QLabel()
        img_label.setAlignment(Qt.AlignmentFlag.AlignCenter)
        img_label.setMinimumSize(_THUMB_W, _THUMB_H)
        vbox.addWidget(img_label)

        parent_layout.addWidget(frame)
        return img_label

    def _show_placeholder(self, label: QLabel, text: str) -> None:
        label.setPixmap(QPixmap())
        label.setText(f"<i style='color: grey;'>{text}</i>")

    def _show_pixmap(self, label: QLabel, image: PILImage) -> None:
        pixmap = _pil_to_pixmap(image)
        scaled = pixmap.scaled(
            _THUMB_W, _THUMB_H,
            Qt.AspectRatioMode.KeepAspectRatio,
            Qt.TransformationMode.SmoothTransformation,
        )
        label.setText("")
        label.setPixmap(scaled)
