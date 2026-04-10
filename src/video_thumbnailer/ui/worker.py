"""Qt worker runnables for off-thread video operations."""

from __future__ import annotations

from typing import TYPE_CHECKING

from PIL import Image
from PySide6.QtCore import QObject, QRunnable, Signal, Slot

from video_thumbnailer.models import TimelinePosition, VideoFile

if TYPE_CHECKING:
    from video_thumbnailer.core.frame_extractor import PyAVFrameExtractor
    from video_thumbnailer.core.thumbnail_writer import FormatDispatchThumbnailWriter
    from video_thumbnailer.core.video_loader import PyAVVideoLoader
    from video_thumbnailer.platform import CacheInvalidator

__all__ = ["VideoLoadWorker", "FrameExtractWorker", "ApplyWorker"]


# ---------------------------------------------------------------------------
# VideoLoadWorker
# ---------------------------------------------------------------------------

class _VideoLoadSignals(QObject):
    finished: Signal = Signal(object)  # VideoFile
    error: Signal = Signal(str)


class VideoLoadWorker(QRunnable):
    """Load a video file in a worker thread.

    Signals:
        signals.finished(VideoFile): Emitted on success.
        signals.error(str): Emitted on failure with a human-readable message.
    """

    def __init__(self, loader: PyAVVideoLoader, path: str) -> None:
        super().__init__()
        self._loader = loader
        self._path = path
        self.signals = _VideoLoadSignals()

    @Slot()
    def run(self) -> None:
        try:
            video = self._loader.load(self._path)
            self.signals.finished.emit(video)
        except Exception as exc:  # noqa: BLE001
            self.signals.error.emit(str(exc))


# ---------------------------------------------------------------------------
# FrameExtractWorker
# ---------------------------------------------------------------------------

class _FrameExtractSignals(QObject):
    finished: Signal = Signal(object)  # PIL.Image.Image
    error: Signal = Signal(str)


class FrameExtractWorker(QRunnable):
    """Extract a single frame from a video in a worker thread.

    Signals:
        signals.finished(PIL.Image): Emitted on success.
        signals.error(str): Emitted on failure with a human-readable message.
    """

    def __init__(
        self,
        extractor: PyAVFrameExtractor,
        video: VideoFile,
        position: TimelinePosition,
    ) -> None:
        super().__init__()
        self._extractor = extractor
        self._video = video
        self._position = position
        self.signals = _FrameExtractSignals()

    @Slot()
    def run(self) -> None:
        try:
            image = self._extractor.extract(self._video, self._position)
            self.signals.finished.emit(image)
        except Exception as exc:  # noqa: BLE001
            self.signals.error.emit(str(exc))


# ---------------------------------------------------------------------------
# ApplyWorker
# ---------------------------------------------------------------------------

class _ApplySignals(QObject):
    finished: Signal = Signal(object)  # ApplyResult
    error: Signal = Signal(str)


class ApplyWorker(QRunnable):
    """Embed a thumbnail and refresh the file-manager cache in a worker thread.

    Signals:
        signals.finished(ApplyResult): Emitted when the operation completes
            (success or failure — check ``result.success``).
        signals.error(str): Emitted only on unhandled exceptions.
    """

    def __init__(
        self,
        writer: FormatDispatchThumbnailWriter,
        invalidator: CacheInvalidator,
        video: VideoFile,
        thumbnail: Image.Image,
    ) -> None:
        super().__init__()
        self._writer = writer
        self._invalidator = invalidator
        self._video = video
        self._thumbnail = thumbnail
        self.signals = _ApplySignals()

    @Slot()
    def run(self) -> None:
        try:
            result = self._writer.write(self._video, self._thumbnail)
            if result.success:
                self._invalidator.invalidate(self._video)
            self.signals.finished.emit(result)
        except Exception as exc:  # noqa: BLE001
            self.signals.error.emit(str(exc))
