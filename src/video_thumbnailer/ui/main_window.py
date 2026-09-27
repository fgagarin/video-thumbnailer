"""Main application window for video-thumbnailer."""

from __future__ import annotations

import random
import subprocess
import sys
from pathlib import Path
from typing import TYPE_CHECKING

from PIL import Image
from PySide6.QtCore import QSettings, Qt, QThreadPool, QUrl, Signal
from PySide6.QtGui import (
    QDesktopServices,
    QDropEvent,
    QKeySequence,
    QPixmap,
    QShortcut,
)
from PySide6.QtWidgets import (
    QFileDialog,
    QHBoxLayout,
    QLabel,
    QMainWindow,
    QMessageBox,
    QProgressDialog,
    QPushButton,
    QStyle,
    QToolButton,
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
_VIDEO_FILE_FILTER = (
    "Video files (*.mp4 *.mov *.m4v *.mkv *.webm *.avi *.flv);;All files (*)"
)


class _ClickableLabel(QLabel):
    clicked = Signal()

    def mouseReleaseEvent(self, event) -> None:  # noqa: N802
        super().mouseReleaseEvent(event)
        if (
            event.button() == Qt.MouseButton.LeftButton
            and not self.selectedText()
        ):
            self.clicked.emit()


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
        self._current_frame_position_ms: int | None = None
        self._frame_extract_active = False
        self._frame_extract_target_ms: int | None = None
        self._pool = QThreadPool.globalInstance()
        self._active_workers: int = 0
        self._progress_dialog: QProgressDialog | None = None
        self._settings = QSettings("Video Thumbnailer", "Video Thumbnailer")
        last_open_directory = self._settings.value(
            "lastOpenDirectory", str(Path.home()), type=str
        )
        self._last_open_directory = (
            last_open_directory
            if last_open_directory and Path(last_open_directory).is_dir()
            else str(Path.home())
        )

        self.setWindowTitle("Video Thumbnailer")
        self.setMinimumSize(900, 560)
        self.resize(1280, 720)
        self.setAcceptDrops(True)
        self.setFocusPolicy(Qt.FocusPolicy.StrongFocus)

        self._build_ui()
        self._install_shortcuts()

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
        self._drop_label = _ClickableLabel("Drop a video file here")
        self._drop_label.setMinimumHeight(_DROP_ZONE_MIN_HEIGHT)
        self._drop_label.setAlignment(Qt.AlignmentFlag.AlignCenter)
        self._drop_label.setCursor(Qt.CursorShape.PointingHandCursor)
        self._drop_label.clicked.connect(self._open_file_dialog)
        self._drop_label.setWordWrap(True)
        self._drop_label.setTextFormat(Qt.TextFormat.PlainText)
        self._drop_label.setTextInteractionFlags(
            Qt.TextInteractionFlag.TextSelectableByMouse
        )
        self._drop_label.setStyleSheet(
            "QLabel {"
            "  border: 2px dashed #aaaaaa;"
            "  border-radius: 8px;"
            "  color: #888888;"
            "  font-size: 16px;"
            "}"
        )
        layout.addWidget(self._drop_label)

        self._path_container = QWidget()
        path_layout = QHBoxLayout(self._path_container)
        path_layout.setContentsMargins(0, 0, 0, 0)
        path_layout.setSpacing(8)

        self._open_video_btn = QToolButton()
        self._open_video_btn.setIcon(
            self.style().standardIcon(QStyle.StandardPixmap.SP_DialogOpenButton)
        )
        self._open_video_btn.setToolTip("Open video file")
        self._open_video_btn.setAccessibleName("Open video file")
        self._open_video_btn.clicked.connect(self._open_file_dialog)
        path_layout.addWidget(self._open_video_btn)

        self._path_label = _ClickableLabel()
        self._path_label.setWordWrap(True)
        self._path_label.setTextFormat(Qt.TextFormat.PlainText)
        self._path_label.setTextInteractionFlags(
            Qt.TextInteractionFlag.TextSelectableByMouse
        )
        self._path_label.setCursor(Qt.CursorShape.PointingHandCursor)
        self._path_label.setToolTip("Show video in containing folder")
        self._path_label.clicked.connect(self._show_video_in_folder)
        self._path_label.setStyleSheet("color: #666666; font-size: 12px;")
        path_layout.addWidget(self._path_label, 1)

        self._path_container.hide()
        layout.addWidget(self._path_container)

        # Side-by-side preview (hidden until a video is loaded)
        self._preview = PreviewWidget()
        self._preview.hide()
        layout.addWidget(self._preview)

        # Timeline scrubber
        self._timeline = TimelineWidget()
        self._timeline.setEnabled(False)
        self._timeline.hide()
        self._timeline.positionChanged.connect(self._on_scrub)
        layout.addWidget(self._timeline)

        # Apply button
        self._apply_btn = QPushButton("Apply Thumbnail")
        self._apply_btn.setEnabled(False)
        self._apply_btn.hide()
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

    def _install_shortcuts(self) -> None:
        prev_shortcut = QShortcut(QKeySequence(Qt.Key.Key_Left), self)
        prev_shortcut.setAutoRepeat(True)
        prev_shortcut.activated.connect(lambda: self._step_selected_frame(-1))
        next_shortcut = QShortcut(QKeySequence(Qt.Key.Key_Right), self)
        next_shortcut.setAutoRepeat(True)
        next_shortcut.activated.connect(lambda: self._step_selected_frame(1))
        open_shortcut = QShortcut(QKeySequence.StandardKey.Open, self)
        open_shortcut.activated.connect(self._open_file_dialog)
        save_shortcut = QShortcut(QKeySequence.StandardKey.Save, self)
        save_shortcut.activated.connect(self._on_apply_clicked)
        help_shortcut = QShortcut(QKeySequence("Shift+/"), self)
        help_shortcut.activated.connect(self._show_hotkeys)
        self._frame_shortcuts = [prev_shortcut, next_shortcut]
        self._shortcuts = [open_shortcut, save_shortcut, help_shortcut]

    def _open_file_dialog(self, checked: bool = False) -> None:
        path, _ = QFileDialog.getOpenFileName(
            self,
            "Open video",
            self._last_open_directory,
            _VIDEO_FILE_FILTER,
        )
        if path:
            self._load_video(path)

    def _load_video(self, path: str) -> None:
        directory = str(Path(path).parent)
        self._last_open_directory = directory
        self._settings.setValue("lastOpenDirectory", directory)
        self._settings.sync()
        self._set_busy(True)
        self._drop_label.setText(f"Loading {path}…")
        worker = VideoLoadWorker(self._loader, path)
        worker.signals.finished.connect(self._on_video_loaded)
        worker.signals.error.connect(self._on_load_error)
        self._pool.start(worker)

    def _on_video_loaded(self, video: VideoFile) -> None:
        self._video = video
        self._current_frame = None
        self._current_frame_position_ms = None
        self._drop_label.setText(
            f"{video.path}\n"
            f"{video.format.name}  {video.duration_ms // 1000}s  "
            f"{video.width}×{video.height}"
        )
        self._timeline.set_duration(video.duration_ms)
        self._timeline.setEnabled(True)
        self._drop_label.hide()
        self._path_label.setText(video.path)
        self._path_container.show()
        self._preview.clear()
        self._preview.set_current_thumbnail(video.existing_thumbnail)
        self._preview.show()
        self._timeline.show()
        self._apply_btn.show()
        self._apply_btn.setEnabled(False)
        self._timeline.set_position(self._initial_position_ms(video))
        self._set_busy(False)
        self._on_scrub(self._timeline.current_position_ms())

    def _on_load_error(self, message: str) -> None:
        self._drop_label.setText("Drop a video file here")
        self._drop_label.show()
        self._path_container.hide()
        self._preview.hide()
        self._timeline.hide()
        self._apply_btn.hide()
        self._set_busy(False)
        QMessageBox.critical(self, "Load Error", message)

    # ------------------------------------------------------------------
    # Frame extraction
    # ------------------------------------------------------------------

    def _on_scrub(self, offset_ms: int) -> None:
        if self._video is None:
            return
        if self._frame_extract_active:
            self._frame_extract_target_ms = offset_ms
            return

        self._frame_extract_active = True
        self._frame_extract_target_ms = offset_ms
        self._set_busy(True)
        position = TimelinePosition(offset_ms=offset_ms)
        worker = FrameExtractWorker(self._extractor, self._video, position)
        worker.signals.finished.connect(self._on_frame_extracted)
        worker.signals.error.connect(self._on_extract_error)
        self._pool.start(worker)

    def _step_selected_frame(self, direction: int) -> None:
        if self._video is None:
            return

        current_ms = self._timeline.current_position_ms()
        step_ms = max(1, self._video.frame_step_ms)
        next_ms = max(0, min(self._video.duration_ms, current_ms + direction * step_ms))
        if next_ms == current_ms:
            return

        self._timeline.set_position(next_ms)
        self._on_scrub(next_ms)

    def _on_frame_extracted(self, image: Image.Image) -> None:
        self._frame_extract_active = False
        requested_ms = self._frame_extract_target_ms
        self._frame_extract_target_ms = None
        if (
            requested_ms is not None
            and requested_ms != self._timeline.current_position_ms()
        ):
            self._on_scrub(self._timeline.current_position_ms())
            return

        self._current_frame = image
        self._current_frame_position_ms = self._timeline.current_position_ms()
        self._preview.set_candidate_frame(
            image, self._timeline.current_position_ms()
        )
        self._apply_btn.setEnabled(True)
        self._set_busy(False)

    def _on_extract_error(self, message: str) -> None:
        self._frame_extract_active = False
        requested_ms = self._frame_extract_target_ms
        self._frame_extract_target_ms = None
        if (
            requested_ms is not None
            and requested_ms != self._timeline.current_position_ms()
        ):
            self._on_scrub(self._timeline.current_position_ms())
            return

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
            self._writer,
            self._invalidator,
            self._video,
            self._current_frame,
            self._current_frame_position_ms,
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
                    frame_step_ms=self._video.frame_step_ms,
                    thumbnail_frame_number=(
                        round(
                            self._current_frame_position_ms
                            / max(1, self._video.frame_step_ms)
                        )
                        if self._current_frame_position_ms is not None
                        else self._video.thumbnail_frame_number
                    ),
                    thumbnail_position_ms=self._current_frame_position_ms,
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

    def _show_video_in_folder(self) -> None:
        if self._video is None:
            return
        video_path = self._video.path
        if sys.platform == "win32":
            try:
                subprocess.Popen(["explorer.exe", "/select,", video_path])
            except OSError:
                self._open_containing_folder(video_path)
        elif sys.platform == "darwin":
            try:
                subprocess.Popen(["open", "-R", video_path])
            except OSError:
                self._open_containing_folder(video_path)
        else:
            self._open_containing_folder(video_path)

    @staticmethod
    def _open_containing_folder(video_path: str) -> None:
        QDesktopServices.openUrl(QUrl.fromLocalFile(str(Path(video_path).parent)))

    def _show_hotkeys(self) -> None:
        QMessageBox.information(
            self,
            "Keyboard shortcuts",
            "Ctrl+O  Open video\n"
            "Ctrl+S  Save preview as thumbnail\n"
            "Left / Right  Previous / next frame\n"
            "Shift+/  Show keyboard shortcuts",
        )

    @staticmethod
    def _initial_position_ms(video: VideoFile) -> int:
        if video.thumbnail_position_ms is not None:
            return max(0, min(video.duration_ms, video.thumbnail_position_ms))
        if video.thumbnail_frame_number is not None:
            frame_position = video.thumbnail_frame_number * video.frame_step_ms
            return max(0, min(video.duration_ms, frame_position))

        lower = video.duration_ms // 3
        upper = video.duration_ms * 2 // 3
        return random.randint(lower, max(lower, upper))

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
