"""PreviewWidget: side-by-side current thumbnail / selected frame display."""

from __future__ import annotations

import time
from typing import TYPE_CHECKING

from PySide6.QtCore import QSize, Qt, QTimer, Slot
from PySide6.QtGui import QColor, QIcon, QPainter, QPen, QPixmap
from PySide6.QtWidgets import (
    QFrame,
    QHBoxLayout,
    QLabel,
    QMessageBox,
    QPlainTextEdit,
    QPushButton,
    QSizePolicy,
    QStyle,
    QToolButton,
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
        self._save_progress = QPlainTextEdit()
        self._save_progress.setReadOnly(True)
        self._save_progress.setFrameShape(QFrame.Shape.NoFrame)
        self._save_progress.setSizePolicy(
            QSizePolicy.Policy.Ignored, QSizePolicy.Policy.Ignored
        )
        self._save_progress.hide()
        current_layout = self._current_panel.layout()
        assert isinstance(current_layout, QVBoxLayout)
        current_layout.addWidget(self._save_progress, 1)
        self._save_stages: list[tuple[str, float]] = []
        self._active_stage: str | None = None
        self._stage_started = 0.0
        self._progress_timer = QTimer(self)
        self._progress_timer.setInterval(200)
        self._progress_timer.timeout.connect(self._refresh_save_progress)
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
        self._save_duration_label = QLabel()
        self._save_duration_label.setStyleSheet("color: #666666; font-size: 12px;")
        self._save_duration_label.hide()

        button_row = QHBoxLayout()
        button_row.setContentsMargins(0, 0, 0, 0)
        button_row.setSpacing(8)
        button_row.addWidget(
            self.apply_button, 0, Qt.AlignmentFlag.AlignBottom
        )
        self.reencode_warning_button = QToolButton()
        self.reencode_warning_button.setIcon(
            self.style().standardIcon(QStyle.StandardPixmap.SP_MessageBoxWarning)
        )
        self.reencode_warning_button.setIconSize(QSize(20, 20))
        self.reencode_warning_button.setFixedSize(34, 34)
        self.reencode_warning_button.setAutoRaise(True)
        self.reencode_warning_button.setToolTip("Why will this video be re-encoded?")
        self.reencode_warning_button.setAccessibleName("Video re-encoding warning")
        self.reencode_warning_button.clicked.connect(self._show_reencode_reason)
        self.reencode_warning_button.hide()
        self._reencode_reason: str | None = None
        button_row.addWidget(
            self.reencode_warning_button, 0, Qt.AlignmentFlag.AlignBottom
        )
        button_row.addWidget(
            self._save_duration_label, 0, Qt.AlignmentFlag.AlignBottom
        )
        button_row.addStretch(1)
        thumbnail_layout.addLayout(button_row)
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
        self.finish_save_progress()
        self.set_reencode_reason(None)
        self._current_pixmap = None
        self._candidate_pixmap = None
        self._show_placeholder(self._current_label, _NO_THUMBNAIL_HINT)
        self._show_placeholder(self._candidate_label, "No frame selected")
        self._candidate_title.setText("Frame")
        self._current_title.setText("Thumbnail")
        self._save_duration_label.hide()

    def set_last_save_duration(self, seconds: float | None) -> None:
        """Show the elapsed time of the most recent save next to the button.

        Args:
            seconds: Wall-clock duration of the save; hides the label if None.
        """
        if seconds is None:
            self._save_duration_label.hide()
            return
        self._save_duration_label.setText(f"Saved in {seconds:.1f}s")
        self._save_duration_label.show()

    def set_reencode_reason(self, reason: str | None) -> None:
        """Show the warning only when saving will require a full re-encode."""
        self._reencode_reason = reason
        self.reencode_warning_button.setVisible(reason is not None)

    def _show_reencode_reason(self) -> None:
        if self._reencode_reason is not None:
            QMessageBox.information(
                self,
                "Video re-encoding",
                "Saving this thumbnail will re-encode the video, which can take "
                "longer and change its video codec.\n\n"
                f"Reason: {self._reencode_reason}",
            )

    def start_save_progress(self) -> None:
        self._save_stages = []
        self._active_stage = None
        self._save_progress.clear()
        self._save_duration_label.hide()
        self._current_label.hide()
        self._save_progress.show()
        self._progress_timer.start()

    @Slot(str, object)
    def update_save_progress(self, name: str, elapsed: float | None) -> None:
        if elapsed is None:
            self._active_stage = name
            self._stage_started = time.monotonic()
        else:
            self._save_stages.append((name, elapsed))
            self._active_stage = None
        self._refresh_save_progress(scroll_to_end=True)

    def finish_save_progress(self) -> None:
        self._progress_timer.stop()
        self._active_stage = None
        self._save_progress.hide()
        self._current_label.show()

    def _refresh_save_progress(self, *, scroll_to_end: bool = False) -> None:
        lines = [f"{name}: {elapsed:.1f} s" for name, elapsed in self._save_stages]
        if self._active_stage is not None:
            elapsed = time.monotonic() - self._stage_started
            lines.append(f"> {self._active_stage}: {elapsed:.1f} s (running)")
        scrollbar = self._save_progress.verticalScrollBar()
        scroll_position = scrollbar.value()
        self._save_progress.setPlainText("\n".join(lines))
        scrollbar.setValue(scrollbar.maximum() if scroll_to_end else scroll_position)

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
