"""Main application window for video-thumbnailer."""

from __future__ import annotations

from typing import TYPE_CHECKING

from PIL import Image
from PySide6.QtCore import Qt, QThreadPool
from PySide6.QtGui import QDropEvent, QPixmap
from PySide6.QtWidgets import (
    QLabel,
    QMainWindow,
    QMessageBox,
    QProgressDialog,
    QPushButton,
    QVBoxLayout,
    QWidget,
)

from video_thumbnailer.models import ApplyResult, TimelinePosition, VideoFile
from video_thumbnailer.ui.preview_widget import PreviewWidget
from video_thumbnailer.ui.timeline_widget import TimelineWidget
from video_thumbnailer.ui.worker import ApplyWorker, FrameExtractWorker, VideoLoadWorker

if TYPE_CHECKING:
    from video_thumbnailer.core.frame_extractor import PyAVFrameExtractor
    from video_thumbnailer.core.thumbnail_writer import FormatDispatchThumbnailWriter
    from video_thumbnailer.core.video_loader import PyAVVideoLoader
    from video_thumbnailer.platform import CacheInvalidator

__all__ = ["MainWindow"]

_DROP_ZONE_MIN_HEIGHT = 200


class MainWindow(QMainWindow):
    """Primary application window.

    Accepts drag-and-drop of video files, provides a timeline scrubber,
    frame preview, and an Apply Thumbnail button.
    """

    def __init__(
        self,
        loader: PyAVVideoLoader,
        extractor: PyAVFrameExtractor,
        writer: FormatDispatchThumbnailWriter,
        invalidator: CacheInvalidator,
        parent: QWidget | None = None,
    ) -> None:
        super().__init__(parent)
        self._loader = loader
        self._extractor = extractor
        self._writer = writer
        self._invalidator = invalidator

        self._video: VideoFile | None = None
        self._current_frame: Image.Image | None = None
        self._pool = QThreadPool.globalInstance()
        self._active_workers: int = 0
        self._progress_dialog: QProgressDialog | None = None

        self.setWindowTitle("Video Thumbnailer")
        self.setMinimumSize(500, 500)
        self.setAcceptDrops(True)

        self._build_ui()

    # ------------------------------------------------------------------
    # UI construction
    # ------------------------------------------------------------------

    def _build_ui(self) -> None:
        central = QWidget()
        self.setCentralWidget(central)
        layout = QVBoxLayout(central)
        layout.setContentsMargins(16, 16, 16, 16)
        layout.setSpacing(12)

        # Drop zone
        self._drop_label = QLabel("Drop a video file here")
        self._drop_label.setMinimumHeight(_DROP_ZONE_MIN_HEIGHT)
        self._drop_label.setAlignment(Qt.AlignmentFlag.AlignCenter)
        self._drop_label.setStyleSheet(
            "QLabel {"
            "  border: 2px dashed #aaaaaa;"
            "  border-radius: 8px;"
            "  color: #888888;"
            "  font-size: 16px;"
            "}"
        )
        layout.addWidget(self._drop_label)

        # Timeline scrubber
        self._timeline = TimelineWidget()
        self._timeline.setEnabled(False)
        self._timeline.positionChanged.connect(self._on_scrub)
        layout.addWidget(self._timeline)

        # Side-by-side preview (hidden until a video is loaded)
        self._preview = PreviewWidget()
        self._preview.hide()
        layout.addWidget(self._preview)

        # Apply button
        self._apply_btn = QPushButton("Apply Thumbnail")
        self._apply_btn.setEnabled(False)
        self._apply_btn.clicked.connect(self._on_apply_clicked)
        layout.addWidget(self._apply_btn)

    # ------------------------------------------------------------------
    # Drag-and-drop
    # ------------------------------------------------------------------

    def dragEnterEvent(self, event: QDropEvent) -> None:
        if event.mimeData().hasUrls():
            event.acceptProposedAction()
        else:
            event.ignore()

    def dropEvent(self, event: QDropEvent) -> None:
        urls = event.mimeData().urls()
        if not urls:
            return
        path = urls[0].toLocalFile()
        self._load_video(path)

    # ------------------------------------------------------------------
    # Video loading
    # ------------------------------------------------------------------

    def _load_video(self, path: str) -> None:
        self._set_busy(True)
        self._drop_label.setText(f"Loading {path}…")
        worker = VideoLoadWorker(self._loader, path)
        worker.signals.finished.connect(self._on_video_loaded)
        worker.signals.error.connect(self._on_load_error)
        self._pool.start(worker)

    def _on_video_loaded(self, video: VideoFile) -> None:
        self._video = video
        self._current_frame = None
        self._drop_label.setText(
            f"{video.path}\n"
            f"{video.format.name}  {video.duration_ms // 1000}s  "
            f"{video.width}×{video.height}"
        )
        self._timeline.set_duration(video.duration_ms)
        self._timeline.setEnabled(True)
        self._preview.set_current_thumbnail(video.existing_thumbnail)
        self._preview.show()
        self._apply_btn.setEnabled(False)
        self._set_busy(False)

    def _on_load_error(self, message: str) -> None:
        self._drop_label.setText("Drop a video file here")
        self._set_busy(False)
        QMessageBox.critical(self, "Load Error", message)

    # ------------------------------------------------------------------
    # Frame extraction
    # ------------------------------------------------------------------

    def _on_scrub(self, offset_ms: int) -> None:
        if self._video is None:
            return
        self._set_busy(True)
        position = TimelinePosition(offset_ms=offset_ms)
        worker = FrameExtractWorker(self._extractor, self._video, position)
        worker.signals.finished.connect(self._on_frame_extracted)
        worker.signals.error.connect(self._on_extract_error)
        self._pool.start(worker)

    def _on_frame_extracted(self, image: Image.Image) -> None:
        self._current_frame = image
        self._preview.set_candidate_frame(image)
        self._apply_btn.setEnabled(True)
        self._set_busy(False)

    def _on_extract_error(self, message: str) -> None:
        self._set_busy(False)
        QMessageBox.warning(self, "Frame Extraction Error", message)

    # ------------------------------------------------------------------
    # Apply thumbnail
    # ------------------------------------------------------------------

    def _on_apply_clicked(self) -> None:
        if self._video is None or self._current_frame is None:
            return

        if self._video.existing_thumbnail is not None:
            answer = QMessageBox.question(
                self,
                "Overwrite existing thumbnail?",
                "This video already has an embedded thumbnail. Overwrite it?",
                QMessageBox.StandardButton.Yes | QMessageBox.StandardButton.No,
                QMessageBox.StandardButton.No,
            )
            if answer != QMessageBox.StandardButton.Yes:
                return

        self._set_busy(True)
        dlg = QProgressDialog("Applying thumbnail\u2026", "", 0, 0, self)
        dlg.setWindowModality(Qt.WindowModality.WindowModal)
        dlg.show()
        self._progress_dialog = dlg

        worker = ApplyWorker(
            self._writer, self._invalidator, self._video, self._current_frame
        )
        worker.signals.finished.connect(self._on_apply_done)
        worker.signals.error.connect(self._on_apply_error)
        self._pool.start(worker)

    def _on_apply_done(self, result: ApplyResult) -> None:
        self._close_progress()
        self._set_busy(False)

        if result.success:
            # Update the cached existing_thumbnail so subsequent confirmation dialogs
            # reflect reality and the Linux XDG writer has the right image.
            if self._video is not None and self._current_frame is not None:
                self._video = VideoFile(
                    path=self._video.path,
                    format=self._video.format,
                    duration_ms=self._video.duration_ms,
                    width=self._video.width,
                    height=self._video.height,
                    existing_thumbnail=self._current_frame,
                    is_writable=self._video.is_writable,
                )
                self._preview.set_current_thumbnail(self._current_frame)
            QMessageBox.information(self, "Success", "Thumbnail applied successfully.")
        else:
            QMessageBox.critical(
                self,
                "Apply Failed",
                result.error_message or "An unknown error occurred.",
            )

    def _on_apply_error(self, message: str) -> None:
        self._close_progress()
        self._set_busy(False)
        QMessageBox.critical(self, "Apply Error", message)

    # ------------------------------------------------------------------
    # Helpers
    # ------------------------------------------------------------------

    def _set_busy(self, busy: bool) -> None:
        self._timeline.setEnabled(not busy and self._video is not None)
        self._apply_btn.setEnabled(not busy and self._current_frame is not None)
        self._drop_label.setAcceptDrops(not busy)

    def _close_progress(self) -> None:
        if self._progress_dialog is not None:
            self._progress_dialog.close()
            self._progress_dialog = None


def _pil_to_pixmap(image: Image.Image) -> QPixmap:
    """Convert a PIL Image to a QPixmap."""
    from PySide6.QtGui import QImage

    rgb = image.convert("RGB")
    w, h = rgb.size
    data = rgb.tobytes()
    qimage = QImage(data, w, h, w * 3, QImage.Format.Format_RGB888)
    return QPixmap.fromImage(qimage)
