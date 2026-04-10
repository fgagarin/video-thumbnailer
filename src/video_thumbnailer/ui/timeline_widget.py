"""Interactive timeline scrubber widget for video-thumbnailer."""

from __future__ import annotations

from PySide6.QtCore import QSize, Qt, QTimer, Signal  # noqa: TCH002
from PySide6.QtGui import QColor, QMouseEvent, QPainter, QPaintEvent, QPen
from PySide6.QtWidgets import QWidget

__all__ = ["TimelineWidget"]

_TRACK_HEIGHT = 6
_HANDLE_RADIUS = 8
_LABEL_HEIGHT = 14
_DEBOUNCE_MS = 150


class TimelineWidget(QWidget):
    """A custom scrubber widget for seeking within a video timeline.

    Signals:
        positionChanged(int): Emitted with the new position in milliseconds
            after a debounce delay once the user stops dragging.

    Usage::

        widget = TimelineWidget(parent)
        widget.set_duration(60_000)          # 60-second video
        widget.positionChanged.connect(on_seek)
    """

    positionChanged = Signal(int)

    def __init__(self, parent: QWidget | None = None) -> None:
        super().__init__(parent)
        self._duration_ms: int = 0
        self._position_ms: int = 0
        self._dragging: bool = False
        self.setMinimumHeight(_TRACK_HEIGHT + _HANDLE_RADIUS * 2 + _LABEL_HEIGHT + 4)

        self._debounce_timer = QTimer(self)
        self._debounce_timer.setSingleShot(True)
        self._debounce_timer.setInterval(_DEBOUNCE_MS)
        self._debounce_timer.timeout.connect(self._emit_position)

    # ------------------------------------------------------------------
    # Public API
    # ------------------------------------------------------------------

    def set_duration(self, ms: int) -> None:
        """Set the total video duration and reset the scrubber to position 0.

        Args:
            ms: Total video duration in milliseconds (>= 0).
        """
        self._duration_ms = max(0, ms)
        self._position_ms = 0
        self.update()

    def set_position(self, ms: int) -> None:
        """Move the scrubber handle to ``ms`` without emitting positionChanged.

        Args:
            ms: Target position in milliseconds.
        """
        self._position_ms = max(0, min(ms, self._duration_ms))
        self.update()

    def sizeHint(self) -> QSize:
        return QSize(300, 40)

    # ------------------------------------------------------------------
    # Qt event overrides
    # ------------------------------------------------------------------

    def mousePressEvent(self, event: QMouseEvent) -> None:
        if self._duration_ms <= 0:
            return
        if event.button() == Qt.MouseButton.LeftButton:
            self._dragging = True
            self._update_position_from_x(event.position().x())

    def mouseMoveEvent(self, event: QMouseEvent) -> None:
        if self._dragging and self._duration_ms > 0:
            self._update_position_from_x(event.position().x())

    def mouseReleaseEvent(self, event: QMouseEvent) -> None:
        if event.button() == Qt.MouseButton.LeftButton and self._dragging:
            self._dragging = False
            self._update_position_from_x(event.position().x())
            # Force immediate emission on release (cancel pending debounce)
            self._debounce_timer.stop()
            self._emit_position()

    def paintEvent(self, event: QPaintEvent) -> None:
        painter = QPainter(self)
        painter.setRenderHint(QPainter.RenderHint.Antialiasing)

        w = self.width()

        track_y = _HANDLE_RADIUS + 2
        track_rect_y = track_y - _TRACK_HEIGHT // 2

        # Background track
        painter.setPen(Qt.PenStyle.NoPen)
        painter.setBrush(QColor("#cccccc"))
        painter.drawRoundedRect(0, track_rect_y, w, _TRACK_HEIGHT, 3, 3)

        # Filled portion
        if self._duration_ms > 0:
            fill_w = int(w * self._position_ms / self._duration_ms)
            painter.setBrush(QColor("#0078d4"))
            painter.drawRoundedRect(0, track_rect_y, fill_w, _TRACK_HEIGHT, 3, 3)

            # Handle
            handle_x = fill_w
            painter.setBrush(QColor("#005a9e"))
            painter.setPen(QPen(QColor("#ffffff"), 2))
            painter.drawEllipse(
                handle_x - _HANDLE_RADIUS,
                track_y - _HANDLE_RADIUS,
                _HANDLE_RADIUS * 2,
                _HANDLE_RADIUS * 2,
            )

        # Time label "M:SS / M:SS"
        def _fmt(ms: int) -> str:
            total_s = ms // 1000
            return f"{total_s // 60}:{total_s % 60:02d}"

        label = f"{_fmt(self._position_ms)} / {_fmt(self._duration_ms)}"
        painter.setPen(QColor("#333333"))
        label_y = track_y + _HANDLE_RADIUS + 2
        painter.drawText(
            0, label_y, w, _LABEL_HEIGHT, Qt.AlignmentFlag.AlignHCenter, label
        )

    # ------------------------------------------------------------------
    # Private helpers
    # ------------------------------------------------------------------

    def _update_position_from_x(self, x: float) -> None:
        if self._duration_ms <= 0 or self.width() <= 0:
            return
        ratio = max(0.0, min(1.0, x / self.width()))
        self._position_ms = int(ratio * self._duration_ms)
        self.update()
        # Restart debounce timer; position is emitted after the user pauses
        self._debounce_timer.start()

    def _emit_position(self) -> None:
        self.positionChanged.emit(self._position_ms)
