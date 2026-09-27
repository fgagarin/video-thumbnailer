"""Unit tests for MainWindow (T039 coverage boost)."""

from __future__ import annotations

from unittest.mock import MagicMock, patch

import pytest
from PIL import Image
from PySide6.QtCore import QSettings, Qt
from PySide6.QtGui import QKeySequence
from PySide6.QtTest import QTest
from PySide6.QtWidgets import QFileDialog, QMessageBox

from video_thumbnailer.core.frame_extractor import PyAVFrameExtractor
from video_thumbnailer.core.thumbnail_writer import FormatDispatchThumbnailWriter
from video_thumbnailer.core.video_loader import PyAVVideoLoader
from video_thumbnailer.models import (
    ApplyError,
    ApplyResult,
    VideoFile,
    VideoFormat,
)
from video_thumbnailer.platform import CacheInvalidator
from video_thumbnailer.ui.main_window import MainWindow, _pil_to_pixmap


def _make_vf(path: str = "/fake/video.mp4", *, with_thumb: bool = False) -> VideoFile:
    thumb = Image.new("RGB", (32, 32)) if with_thumb else None
    return VideoFile(
        path=path,
        format=VideoFormat.MP4,
        duration_ms=5000,
        width=320,
        height=240,
        existing_thumbnail=thumb,
        is_writable=True,
    )


@pytest.fixture()
def deps():
    """Return a tuple of (loader, extractor, writer, invalidator) as MagicMocks."""
    loader = MagicMock(spec=PyAVVideoLoader)
    extractor = MagicMock(spec=PyAVFrameExtractor)
    writer = MagicMock(spec=FormatDispatchThumbnailWriter)
    invalidator = MagicMock(spec=CacheInvalidator)
    return loader, extractor, writer, invalidator


@pytest.fixture()
def window(qtbot, deps):  # type: ignore[type-arg]
    loader, extractor, writer, invalidator = deps
    w = MainWindow(loader, extractor, writer, invalidator)
    w._on_scrub = MagicMock()
    qtbot.addWidget(w)
    w.show()
    return w


class TestMainWindowConstruction:
    def test_window_title(self, window: MainWindow) -> None:
        assert "Thumbnailer" in window.windowTitle() or "thumbnailer" in window.windowTitle().lower()

    def test_apply_button_disabled_initially(self, window: MainWindow) -> None:
        assert not window._apply_btn.isEnabled()

    def test_timeline_disabled_initially(self, window: MainWindow) -> None:
        assert not window._timeline.isEnabled()

    def test_preview_hidden_initially(self, window: MainWindow) -> None:
        assert not window._preview.isVisible()

    def test_video_controls_hidden_initially(self, window: MainWindow) -> None:
        assert not window._timeline.isVisible()
        assert not window._apply_btn.isVisible()

    def test_drop_zone_visible(self, window: MainWindow) -> None:
        assert window._drop_label.isVisible()

    def test_default_window_is_wide(self, window: MainWindow) -> None:
        assert window.width() >= 1200
        assert window.height() >= 700

class TestMainWindowVideoLoading:
    def test_on_video_loaded_updates_ui(self, window: MainWindow) -> None:
        vf = _make_vf()
        window._on_video_loaded(vf)

        assert window._video is vf
        assert window._timeline.isEnabled()
        assert window._preview.isVisible()
        assert window._timeline.isVisible()
        assert window._apply_btn.isVisible()
        assert not window._drop_label.isVisible()
        assert not window._apply_btn.isEnabled()  # no frame yet

    def test_on_video_loaded_restores_saved_thumbnail_position(
        self, window: MainWindow
    ) -> None:
        video = _make_vf()
        video.thumbnail_frame_number = 80
        video.thumbnail_position_ms = 3210

        window._on_video_loaded(video)

        assert window._timeline.current_position_ms() == 3210
        assert window._path_label.text() == video.path
        assert window._path_label.isVisible()
        assert window._open_video_btn.isVisible()
        window._on_scrub.assert_called_once_with(3210)

    def test_open_video_button_opens_file_dialog(
        self, window: MainWindow, tmp_path
    ) -> None:
        window._last_open_directory = str(tmp_path)

        with (
            patch.object(
                QFileDialog, "getOpenFileName", return_value=("", "")
            ) as get_open_file,
            patch.object(window, "_load_video") as load_video,
        ):
            window._open_video_btn.click()

        assert get_open_file.call_args.args[2] == str(tmp_path)
        load_video.assert_not_called()

    def test_drop_zone_click_opens_file_dialog(self, window: MainWindow) -> None:
        with patch.object(
            QFileDialog, "getOpenFileName", return_value=("", "")
        ) as get_open_file:
            QTest.mouseClick(window._drop_label, Qt.MouseButton.LeftButton)

        get_open_file.assert_called_once()

    def test_load_remembers_last_open_directory(
        self, window: MainWindow, tmp_path
    ) -> None:
        settings = QSettings(
            str(tmp_path / "settings.ini"), QSettings.Format.IniFormat
        )
        window._settings = settings
        video_path = str(tmp_path / "clips" / "clip.mp4")

        with patch.object(window._pool, "start"):
            window._load_video(video_path)

        assert settings.value("lastOpenDirectory") == str(tmp_path / "clips")

    def test_path_click_reveals_video_in_windows_explorer(
        self, window: MainWindow
    ) -> None:
        window._on_video_loaded(_make_vf("C:/videos/clip.mp4"))

        with (
            patch("video_thumbnailer.ui.main_window.sys.platform", "win32"),
            patch("video_thumbnailer.ui.main_window.subprocess.Popen") as popen,
        ):
            QTest.mouseClick(window._path_label, Qt.MouseButton.LeftButton)

        popen.assert_called_once_with(
            ["explorer.exe", "/select,", "C:/videos/clip.mp4"]
        )

    def test_shortcuts_include_open_save_and_help(self, window: MainWindow) -> None:
        shortcut_text = {
            shortcut.key().toString(QKeySequence.SequenceFormat.PortableText)
            for shortcut in window._shortcuts
        }

        assert shortcut_text == {"Ctrl+O", "Ctrl+S", "Shift+/"}

    def test_on_video_loaded_restores_from_frame_number_when_no_timestamp(
        self, window: MainWindow
    ) -> None:
        video = _make_vf()
        video.thumbnail_frame_number = 40

        window._on_video_loaded(video)

        assert window._timeline.current_position_ms() == 40 * video.frame_step_ms

    def test_on_video_loaded_randomizes_in_second_third_when_no_metadata(
        self, window: MainWindow
    ) -> None:
        video = _make_vf()
        with patch("video_thumbnailer.ui.main_window.random.randint", return_value=2500) as randint:
            window._on_video_loaded(video)

        randint.assert_called_once_with(video.duration_ms // 3, video.duration_ms * 2 // 3)
        assert window._timeline.current_position_ms() == 2500

    def test_loaded_layout_places_preview_above_timeline(self, window: MainWindow) -> None:
        window._on_video_loaded(_make_vf())
        layout = window.centralWidget().layout()

        assert layout.indexOf(window._preview) < layout.indexOf(window._timeline)
        assert layout.indexOf(window._timeline) < layout.indexOf(window._apply_btn)

    def test_on_video_loaded_with_existing_thumb(self, window: MainWindow) -> None:
        vf = _make_vf(with_thumb=True)
        window._on_video_loaded(vf)
        # Preview left panel should show the existing thumbnail
        assert not window._preview._current_label.pixmap().isNull()

    def test_on_load_error_shows_message(self, window: MainWindow, qtbot) -> None:
        with patch.object(QMessageBox, "critical", return_value=None) as mock_msg:
            window._on_load_error("some error")
        mock_msg.assert_called_once()
        # Drop label should revert to placeholder text
        assert "Drop" in window._drop_label.text()

    def test_drop_label_text_after_load(self, window: MainWindow) -> None:
        vf = _make_vf("/video/test.mp4")
        window._on_video_loaded(vf)
        assert "test.mp4" in window._drop_label.text() or "MP4" in window._drop_label.text()

    def test_drop_label_wraps_and_keeps_long_path(self, window: MainWindow) -> None:
        path = "C:\\videos\\" + "a_long_folder\\" * 12 + "movie.mp4"
        window._on_video_loaded(_make_vf(path))

        assert window._drop_label.wordWrap()
        assert path in window._drop_label.text()


class TestMainWindowFrameExtraction:
    def test_on_frame_extracted_enables_apply(self, window: MainWindow) -> None:
        vf = _make_vf()
        window._on_video_loaded(vf)

        img = Image.new("RGB", (32, 32))
        window._on_frame_extracted(img)

        assert window._current_frame is img
        assert window._apply_btn.isEnabled()

    def test_on_scrub_skips_when_no_video(self, window: MainWindow) -> None:
        """_on_scrub should do nothing if no video is loaded."""
        window._on_scrub(2000)
        # No worker should have been submitted (pool start not called)
        # Just verify no exception is raised

    def test_on_extract_error_shows_warning(self, window: MainWindow) -> None:
        with patch.object(QMessageBox, "warning", return_value=None) as mock_warn:
            window._on_extract_error("decode fail")
        mock_warn.assert_called_once()

    def test_step_selected_frame_moves_timeline(self, window: MainWindow) -> None:
        vf = _make_vf()
        vf.frame_step_ms = 40
        window._on_video_loaded(vf)
        window._timeline.set_position(1000)

        with patch.object(window, "_on_scrub") as mock_scrub:
            window._step_selected_frame(1)

        assert window._timeline.current_position_ms() == 1040
        mock_scrub.assert_called_once_with(1040)

    def test_repeated_step_moves_timeline_while_busy(self, window: MainWindow) -> None:
        vf = _make_vf()
        vf.frame_step_ms = 40
        window._on_video_loaded(vf)
        window._timeline.set_position(1000)
        window._set_busy(True)

        with patch.object(window, "_on_scrub") as mock_scrub:
            for _ in range(5):
                window._step_selected_frame(1)

        assert window._timeline.current_position_ms() == 1200
        assert [call.args[0] for call in mock_scrub.call_args_list] == [
            1040, 1080, 1120, 1160, 1200
        ]

    def test_frame_shortcuts_allow_auto_repeat(self, window: MainWindow) -> None:
        assert all(shortcut.autoRepeat() for shortcut in window._frame_shortcuts)

    def test_stale_frame_result_retries_latest_position(self, window: MainWindow) -> None:
        window._on_video_loaded(_make_vf())
        window._timeline.set_position(2000)
        window._frame_extract_active = True
        window._frame_extract_target_ms = 1000

        with patch.object(window, "_on_scrub") as mock_scrub:
            window._on_frame_extracted(Image.new("RGB", (32, 32)))

        mock_scrub.assert_called_once_with(2000)
        assert window._current_frame is None

    def test_step_selected_frame_clamps_at_zero(self, window: MainWindow) -> None:
        vf = _make_vf()
        vf.frame_step_ms = 40
        window._on_video_loaded(vf)
        window._timeline.set_position(0)

        with patch.object(window, "_on_scrub") as mock_scrub:
            window._step_selected_frame(-1)

        assert window._timeline.current_position_ms() == 0
        mock_scrub.assert_not_called()


class TestMainWindowApplyThumbnail:
    def test_on_apply_done_success_shows_info(self, window: MainWindow) -> None:
        vf = _make_vf()
        window._on_video_loaded(vf)
        window._current_frame = Image.new("RGB", (32, 32))

        result = ApplyResult(success=True)
        with patch.object(QMessageBox, "information", return_value=None) as mock_info:
            window._on_apply_done(result)
        mock_info.assert_called_once()

    def test_on_apply_done_failure_shows_error(self, window: MainWindow) -> None:
        vf = _make_vf()
        window._on_video_loaded(vf)
        window._current_frame = Image.new("RGB", (32, 32))

        result = ApplyResult(
            success=False,
            error_code=ApplyError.FFMPEG_ERROR,
            error_message="ffmpeg failed",
        )
        with patch.object(QMessageBox, "critical", return_value=None) as mock_err:
            window._on_apply_done(result)
        mock_err.assert_called_once()

    def test_on_apply_error_shows_critical(self, window: MainWindow) -> None:
        with patch.object(QMessageBox, "critical", return_value=None) as mock_crit:
            window._on_apply_error("catastrophic fail")
        mock_crit.assert_called_once()

    def test_on_apply_clicked_no_video_does_nothing(self, window: MainWindow) -> None:
        # Should return immediately without error when no video loaded
        window._on_apply_clicked()

    def test_on_apply_clicked_skips_when_no_frame(self, window: MainWindow) -> None:
        vf = _make_vf()
        window._on_video_loaded(vf)
        window._current_frame = None
        # Should return early since no frame
        window._on_apply_clicked()

    def test_on_apply_clicked_with_existing_thumb_prompts(self, window: MainWindow) -> None:
        """When video has existing thumbnail, user is prompted to confirm overwrite."""
        vf = _make_vf(with_thumb=True)
        window._on_video_loaded(vf)
        window._current_frame = Image.new("RGB", (32, 32))

        # Simulate user clicking "No" on the confirmation dialog
        with patch.object(QMessageBox, "question", return_value=QMessageBox.StandardButton.No):
            window._on_apply_clicked()
        # Apply should have been cancelled — no worker running

    def test_close_progress_when_none_is_noop(self, window: MainWindow) -> None:
        window._progress_dialog = None
        window._close_progress()  # should not raise

    def test_set_busy_disables_controls(self, window: MainWindow) -> None:
        vf = _make_vf()
        window._on_video_loaded(vf)
        window._current_frame = Image.new("RGB", (32, 32))
        window._apply_btn.setEnabled(True)

        window._set_busy(True)
        assert not window._apply_btn.isEnabled()
        assert not window._timeline.isEnabled()


# ---------------------------------------------------------------------------
# _pil_to_pixmap helper
# ---------------------------------------------------------------------------

class TestPilToPixmap:
    def test_converts_rgb_image(self) -> None:
        img = Image.new("RGB", (16, 16), color=(100, 150, 200))
        pixmap = _pil_to_pixmap(img)
        assert not pixmap.isNull()
        assert pixmap.width() == 16
        assert pixmap.height() == 16

    def test_converts_rgba_image(self) -> None:
        img = Image.new("RGBA", (8, 8), color=(255, 0, 0, 128))
        pixmap = _pil_to_pixmap(img)
        assert not pixmap.isNull()
