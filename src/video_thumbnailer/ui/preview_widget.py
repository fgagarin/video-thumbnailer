"""PreviewWidget: side-by-side current thumbnail / selected frame display."""

from __future__ import annotations

from typing import TYPE_CHECKING

from PySide6.QtCore import QSize, Qt
from PySide6.QtGui import QColor, QIcon, QPainter, QPen, QPixmap
from PySide6.QtWidgets import (
    QFrame,
    QHBoxLayout,
    QLabel,
    QPushButton,
    QSizePolicy,
    QVBoxLayout,
    QWidget,
)

if TYPE_CHECKING:
    from PIL.Image import Image as PILImage

__all__ = ["PreviewWidget"]

_NO_THUMBNAIL_HINT = (
    "No preview yet. Select a frame, then press Save thumbnail."
)

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
        │   Frame                  │   Thumbnail              │
        │  ┌────────────────────┐  │  ┌────────────────────┐  │
        │  │  <frame image>     │  │  │  <thumbnail image> │  │
        │  └────────────────────┘  │  └────────────────────┘  │
        │                          │     Save thumbnail       │
        └──────────────────────────┴──────────────────────────┘
    """

    def __init__(self, parent: QWidget | None = None) -> None:
        super().__init__(parent)

        layout = QHBoxLayout(self)
        layout.setContentsMargins(4, 4, 4, 4)
        layout.setSpacing(8)

        self._candidate_panel, self._candidate_title, self._candidate_label = (
            self._make_panel("Frame")
        )
        layout.addWidget(self._candidate_panel, 1)

        self._thumbnail_column = QWidget()
        thumbnail_layout = QVBoxLayout(self._thumbnail_column)
        thumbnail_layout.setContentsMargins(0, 0, 0, 0)
        thumbnail_layout.setSpacing(8)
        self._current_panel, self._current_title, self._current_label = (
            self._make_panel("Thumbnail")
        )
        thumbnail_layout.addWidget(self._current_panel, 1)
        self.apply_button = QPushButton("Save thumbnail")
        self.apply_button.setIcon(self._white_save_icon())
        self.apply_button.setIconSize(QSize(20, 20))
        self.apply_button.setToolTip("Save the selected frame as the video thumbnail")
        self.apply_button.setAccessibleName("Save thumbnail")
        self.apply_button.setMinimumHeight(34)
        self.apply_button.setFixedWidth(176)
        self.apply_button.setSizePolicy(
            QSizePolicy.Policy.Fixed, QSizePolicy.Policy.Fixed
        )
        self.apply_button.setStyleSheet(
            "QPushButton { background: #0078d4; color: white; "
            "border: 1px solid #005a9e; "
            "border-radius: 6px; }"
            "QPushButton:hover { background: #006cbe; }"
            "QPushButton:pressed { background: #005a9e; }"
            "QPushButton:disabled { background: #8ab9dc; border-color: #8ab9dc; }"
        )
        thumbnail_layout.addWidget(
            self.apply_button,
            0,
            Qt.AlignmentFlag.AlignLeft | Qt.AlignmentFlag.AlignBottom,
        )
        layout.addWidget(self._thumbnail_column, 1)
        self._current_pixmap: QPixmap | None = None
        self._candidate_pixmap: QPixmap | None = None

        self._show_placeholder(self._current_label, _NO_THUMBNAIL_HINT)
        self._show_placeholder(self._candidate_label, "No frame selected")

    # ------------------------------------------------------------------
    # Public API
    # ------------------------------------------------------------------

    def set_current_thumbnail(self, image: PILImage | None) -> None:
        """Display the current embedded thumbnail, or placeholder if None."""
        if image is None:
            self._current_pixmap = None
            self._show_placeholder(
                self._current_label,
                "No preview yet. Select a frame, then press Save thumbnail.",
            )
        else:
            self._current_pixmap = _pil_to_pixmap(image)
            self._refresh_pixmaps()

    def set_candidate_frame(
        self, image: PILImage, position_ms: int | None = None
    ) -> None:
        """Display a selected video frame as the candidate thumbnail."""
        if position_ms is None:
            self._candidate_title.setText("Frame")
        else:
            self._candidate_title.setText(
                f"Frame ({self._format_position(position_ms)})"
            )
        self._candidate_pixmap = _pil_to_pixmap(image)
        self._refresh_pixmaps()

    def clear(self) -> None:
        """Reset both panels to their placeholder states."""
        self._current_pixmap = None
        self._candidate_pixmap = None
        self._show_placeholder(self._current_label, _NO_THUMBNAIL_HINT)
        self._show_placeholder(self._candidate_label, "No frame selected")
        self._candidate_title.setText("Frame")
        self._current_title.setText("Thumbnail")

    def sizeHint(self) -> QSize:
        return QSize(720, 220)

    def resizeEvent(self, event) -> None:  # noqa: N802
        super().resizeEvent(event)
        self._refresh_pixmaps()

    # ------------------------------------------------------------------
    # Private helpers
    # ------------------------------------------------------------------

    def _make_panel(self, title: str) -> tuple[QFrame, QLabel, QLabel]:
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

        return frame, title_label, img_label

    @staticmethod
    def _format_position(position_ms: int) -> str:
        total_seconds, milliseconds = divmod(max(0, position_ms), 1000)
        minutes, seconds = divmod(total_seconds, 60)
        return f"{minutes}:{seconds:02d}.{milliseconds:03d}"

    def _show_placeholder(self, label: QLabel, text: str) -> None:
        label.setPixmap(QPixmap())
        label.setText(f"<i style='color: grey;'>{text}</i>")

    def _white_save_icon(self) -> QIcon:
        pixmap = QPixmap(32, 32)
        pixmap.fill(Qt.GlobalColor.transparent)
        painter = QPainter(pixmap)
        painter.setRenderHint(QPainter.RenderHint.Antialiasing)
        pen = QPen(QColor("#ffffff"), 2.4)
        pen.setCapStyle(Qt.PenCapStyle.RoundCap)
        pen.setJoinStyle(Qt.PenJoinStyle.RoundJoin)
        painter.setPen(pen)
        painter.drawLine(16, 4, 16, 19)
        painter.drawLine(10, 13, 16, 19)
        painter.drawLine(16, 19, 22, 13)
        painter.drawLine(6, 20, 6, 27)
        painter.drawLine(6, 27, 26, 27)
        painter.drawLine(26, 27, 26, 20)
        painter.end()
        return QIcon(pixmap)

    def _refresh_pixmaps(self) -> None:
        for label, pixmap, scale in (
            (self._current_label, self._current_pixmap, 0.9),
            (self._candidate_label, self._candidate_pixmap, 1.0),
        ):
            if pixmap is None or label.width() <= 0 or label.height() <= 0:
                continue
            label.setText("")
            target_size = QSize(
                max(1, round(label.width() * scale)),
                max(1, round(label.height() * scale)),
            )
            label.setPixmap(
                pixmap.scaled(
                    target_size,
                    Qt.AspectRatioMode.KeepAspectRatio,
                    Qt.TransformationMode.SmoothTransformation,
                )
            )
