"""Lazy, width-adaptive filmstrip of sampled video frames."""

from __future__ import annotations

from collections import deque
from queue import Empty, Queue
from typing import TYPE_CHECKING

from PIL import Image
from PySide6.QtCore import (
    QPoint,
    QRect,
    QRunnable,
    QSize,
    Qt,
    QThreadPool,
    QTimer,
    Signal,
)
from PySide6.QtGui import QColor, QMouseEvent, QPainter, QPaintEvent, QPen, QPixmap
from PySide6.QtWidgets import QSizePolicy, QWidget

from video_thumbnailer.models import TimelinePosition, VideoFile

if TYPE_CHECKING:
    from video_thumbnailer.core.frame_extractor import PyAVFrameExtractor

__all__ = ["FilmstripWidget"]

_TARGET_TILE_WIDTH = 128
_TILE_HEIGHT = 72
_STRIP_HEIGHT = _TILE_HEIGHT + 8
_MAX_CONCURRENT_EXTRACTIONS = 3


class _FilmstripFrameWorker(QRunnable):
    def __init__(
        self,
        extractor: PyAVFrameExtractor,
        video: VideoFile,
        generation: int,
        frame_index: int,
        position_ms: int,
        results: Queue[tuple[int, int, Image.Image | None, str | None]],
    ) -> None:
        super().__init__()
        self.setAutoDelete(False)
        self._extractor = extractor
        self._video = video
        self._generation = generation
        self._frame_index = frame_index
        self._position_ms = position_ms
        self._results = results

    def run(self) -> None:
        try:
            image = self._extractor.extract(
                self._video, TimelinePosition(offset_ms=self._position_ms)
            )
            self._results.put(
                (self._generation, self._frame_index, image, None)
            )
        except Exception as exc:  # noqa: BLE001
            self._results.put(
                (self._generation, self._frame_index, None, str(exc))
            )


class FilmstripWidget(QWidget):
    """Display evenly sampled frames, extracting only samples needed for its width."""

    positionSelected = Signal(int)

    def __init__(self, parent: QWidget | None = None) -> None:
        super().__init__(parent)
        self.setMinimumHeight(_STRIP_HEIGHT)
        self.setMaximumHeight(_STRIP_HEIGHT)
        self.setSizePolicy(QSizePolicy.Policy.Expanding, QSizePolicy.Policy.Fixed)
        self.setCursor(Qt.CursorShape.PointingHandCursor)

        self._pool = QThreadPool(self)
        self._pool.setMaxThreadCount(_MAX_CONCURRENT_EXTRACTIONS)
        self._results: Queue[tuple[int, int, Image.Image | None, str | None]] = Queue()
        self._result_timer = QTimer(self)
        self._result_timer.setInterval(20)
        self._result_timer.timeout.connect(self._drain_results)
        self._video: VideoFile | None = None
        self._extractor: PyAVFrameExtractor | None = None
        self._generation = 0
        self._frame_step_ms = 40
        self._frame_count = 0
        self._tile_width = float(_TARGET_TILE_WIDTH)
        self._positions: list[tuple[int, int]] = []
        self._selected_position_ms = 0
        self._frames: dict[int, QPixmap] = {}
        self._in_flight: set[tuple[int, int]] = set()
        self._workers: dict[tuple[int, int], _FilmstripFrameWorker] = {}
        self._queue: deque[tuple[int, int, int]] = deque()

    def set_video(
        self,
        video: VideoFile | None,
        extractor: PyAVFrameExtractor | None = None,
    ) -> None:
        self._generation += 1
        self._video = video
        self._extractor = extractor
        self._frames.clear()
        self._queue.clear()
        self._positions.clear()
        self._selected_position_ms = 0

        if video is None or extractor is None:
            self.update()
            return

        self._frame_step_ms = max(1, video.frame_step_ms)
        self._frame_count = max(
            1, (video.duration_ms + self._frame_step_ms - 1) // self._frame_step_ms + 1
        )
        self._update_samples()

    def set_selected_position(self, position_ms: int) -> None:
        self._selected_position_ms = max(0, position_ms)
        self.update()

    def sizeHint(self) -> QSize:
        return QSize(700, _STRIP_HEIGHT)

    def resizeEvent(self, event) -> None:  # noqa: N802
        super().resizeEvent(event)
        self._update_samples()

    def mousePressEvent(self, event: QMouseEvent) -> None:
        if event.button() != Qt.MouseButton.LeftButton or not self._positions:
            return
        tile_index = min(
            int(event.position().x() / self._tile_width),
            len(self._positions) - 1,
        )
        if tile_index < len(self._positions):
            self.positionSelected.emit(self._positions[tile_index][1])

    def paintEvent(self, event: QPaintEvent) -> None:
        painter = QPainter(self)
        painter.setRenderHint(QPainter.RenderHint.Antialiasing)
        painter.fillRect(self.rect(), self.palette().window())

        selected_index = self._selected_tile_index()
        for tile_index, (frame_index, _position_ms) in enumerate(self._positions):
            tile = self._tile_rect(tile_index)
            painter.setPen(
                QPen(
                    QColor("#0078d4")
                    if tile_index == selected_index
                    else QColor("#aeb4bb"),
                    2 if tile_index == selected_index else 1,
                )
            )
            painter.setBrush(QColor("#e4e7eb"))
            painter.drawRect(tile)

            pixmap = self._frames.get(frame_index)
            if pixmap is not None:
                bounds = tile.adjusted(3, 3, -3, -3)
                scaled = pixmap.scaled(
                    bounds.size(),
                    Qt.AspectRatioMode.KeepAspectRatio,
                    Qt.TransformationMode.SmoothTransformation,
                )
                target = QRect(QPoint(0, 0), scaled.size())
                target.moveCenter(bounds.center())
                painter.drawPixmap(target, scaled)

    def _tile_rect(self, tile_index: int) -> QRect:
        left = round(tile_index * self._tile_width)
        right = round((tile_index + 1) * self._tile_width)
        return QRect(
            left,
            (self.height() - _TILE_HEIGHT) // 2,
            max(1, right - left),
            _TILE_HEIGHT,
        )

    def _drain_results(self) -> None:
        while True:
            try:
                generation, frame_index, image, _error = self._results.get_nowait()
            except Empty:
                break

            key = (generation, frame_index)
            self._in_flight.discard(key)
            self._workers.pop(key, None)
            if generation == self._generation and image is not None:
                self._frames[frame_index] = QPixmap.fromImage(
                    _image_to_qimage(image)
                )
                self.update()

        self._dispatch_queue()
        if not self._in_flight and not self._queue:
            self._result_timer.stop()

    def _update_samples(self) -> None:
        if self._video is None or self._extractor is None:
            return

        tile_count = max(
            1, round(self.width() / _TARGET_TILE_WIDTH)
        )
        tile_count = min(tile_count, self._frame_count)
        self._tile_width = max(1.0, self.width() / tile_count)
        if tile_count == 1:
            frame_indices = [self._frame_count // 2]
        else:
            last_index = self._frame_count - 1
            frame_indices = [
                round(index * last_index / (tile_count - 1))
                for index in range(tile_count)
            ]

        self._positions = [
            (
                frame_index,
                min(self._video.duration_ms, frame_index * self._frame_step_ms),
            )
            for frame_index in frame_indices
        ]
        self._queue = deque(
            (self._generation, frame_index, position_ms)
            for frame_index, position_ms in self._positions
            if frame_index not in self._frames
            and (self._generation, frame_index) not in self._in_flight
        )
        self.update()
        self._dispatch_queue()

    def _dispatch_queue(self) -> None:
        while self._queue and len(self._in_flight) < _MAX_CONCURRENT_EXTRACTIONS:
            generation, frame_index, position_ms = self._queue.popleft()
            key = (generation, frame_index)
            if generation != self._generation or key in self._in_flight:
                continue
            if self._extractor is None or self._video is None:
                return

            worker = _FilmstripFrameWorker(
                self._extractor,
                self._video,
                generation,
                frame_index,
                position_ms,
                self._results,
            )
            self._in_flight.add(key)
            self._workers[key] = worker
            self._pool.start(worker)
            self._result_timer.start()

    def _selected_tile_index(self) -> int:
        if not self._positions:
            return -1
        return min(
            range(len(self._positions)),
            key=lambda index: abs(
                self._positions[index][1] - self._selected_position_ms
            ),
        )


def _image_to_qimage(image: Image.Image):
    from PIL.ImageQt import toqimage

    return toqimage(image.convert("RGB"))