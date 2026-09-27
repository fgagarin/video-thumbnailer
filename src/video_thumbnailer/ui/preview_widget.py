"""PreviewWidget: side-by-side current thumbnail / selected frame display."""

from __future__ import annotations

from typing import TYPE_CHECKING

from PySide6.QtCore import QSize, Qt
from PySide6.QtGui import QPixmap
from PySide6.QtWidgets import (
    QFrame,
    QHBoxLayout,
    QLabel,
    QSizePolicy,
    QVBoxLayout,
    QWidget,
)

if TYPE_CHECKING:
    from PIL.Image import Image as PILImage

__all__ = ["PreviewWidget"]

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

        self._current_title, self._current_label = self._make_panel(
            layout, "Current Thumbnail"
        )
        self._candidate_title, self._candidate_label = self._make_panel(
            layout, "Selected Frame"
        )
        self._current_pixmap: QPixmap | None = None
        self._candidate_pixmap: QPixmap | None = None

        self._show_placeholder(self._current_label, "No current thumbnail")
        self._show_placeholder(self._candidate_label, "No frame selected")

    # ------------------------------------------------------------------
    # Public API
    # ------------------------------------------------------------------

    def set_current_thumbnail(self, image: PILImage | None) -> None:
        """Display the current embedded thumbnail, or placeholder if None."""
        if image is None:
            self._current_pixmap = None
            self._show_placeholder(self._current_label, "No current thumbnail")
        else:
            self._current_pixmap = _pil_to_pixmap(image)
            self._refresh_pixmaps()

    def set_candidate_frame(
        self, image: PILImage, position_ms: int | None = None
    ) -> None:
        """Display a selected video frame as the candidate thumbnail."""
        if position_ms is None:
            self._candidate_title.setText("Selected Frame")
        else:
            self._candidate_title.setText(
                f"Selected Frame ({self._format_position(position_ms)})"
            )
        self._candidate_pixmap = _pil_to_pixmap(image)
        self._refresh_pixmaps()

    def clear(self) -> None:
        """Reset both panels to their placeholder states."""
        self._current_pixmap = None
        self._candidate_pixmap = None
        self._show_placeholder(self._current_label, "No current thumbnail")
        self._show_placeholder(self._candidate_label, "No frame selected")
        self._candidate_title.setText("Selected Frame")

    def sizeHint(self) -> QSize:
        return QSize(540, 220)

    def resizeEvent(self, event) -> None:  # noqa: N802
        super().resizeEvent(event)
        self._refresh_pixmaps()

    # ------------------------------------------------------------------
    # Private helpers
    # ------------------------------------------------------------------

    def _make_panel(
        self, parent_layout: QHBoxLayout, title: str
    ) -> tuple[QLabel, QLabel]:
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
        img_label.setMinimumSize(0, 0)
        img_label.setSizePolicy(
            QSizePolicy.Policy.Ignored, QSizePolicy.Policy.Ignored
        )
        vbox.addWidget(img_label, 1)

        parent_layout.addWidget(frame)
        return title_label, img_label

    @staticmethod
    def _format_position(position_ms: int) -> str:
        total_seconds, milliseconds = divmod(max(0, position_ms), 1000)
        minutes, seconds = divmod(total_seconds, 60)
        return f"{minutes}:{seconds:02d}.{milliseconds:03d}"

    def _show_placeholder(self, label: QLabel, text: str) -> None:
        label.setPixmap(QPixmap())
        label.setText(f"<i style='color: grey;'>{text}</i>")

    def _refresh_pixmaps(self) -> None:
        for label, pixmap in (
            (self._current_label, self._current_pixmap),
            (self._candidate_label, self._candidate_pixmap),
        ):
            if pixmap is None or label.width() <= 0 or label.height() <= 0:
                continue
            label.setText("")
            label.setPixmap(
                pixmap.scaled(
                    label.size(),
                    Qt.AspectRatioMode.KeepAspectRatio,
                    Qt.TransformationMode.SmoothTransformation,
                )
            )
