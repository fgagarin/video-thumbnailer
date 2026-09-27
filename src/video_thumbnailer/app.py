"""Application entry point for video-thumbnailer."""

from __future__ import annotations

import ctypes
import sys
from pathlib import Path

from PySide6.QtGui import QIcon


def _resource_path(name: str) -> Path:
    bundle_dir = Path(getattr(sys, "_MEIPASS", Path(__file__).resolve().parents[2]))
    return bundle_dir / name


def _set_windows_app_user_model_id() -> None:
    if sys.platform == "win32":
        ctypes.windll.shell32.SetCurrentProcessExplicitAppUserModelID(
            ctypes.c_wchar_p("VideoThumbnailer.Application")
        )


def main() -> None:
    """Launch the video-thumbnailer GUI application."""
    from PySide6.QtWidgets import QApplication

    from video_thumbnailer.core.frame_extractor import PyAVFrameExtractor
    from video_thumbnailer.core.thumbnail_writer import FormatDispatchThumbnailWriter
    from video_thumbnailer.core.video_loader import PyAVVideoLoader
    from video_thumbnailer.platform import get_cache_invalidator
    from video_thumbnailer.ui.main_window import MainWindow

    _set_windows_app_user_model_id()
    app = QApplication(sys.argv)
    icon = QIcon(str(_resource_path("icon.png")))
    app.setWindowIcon(icon)

    loader = PyAVVideoLoader()
    extractor = PyAVFrameExtractor()
    writer = FormatDispatchThumbnailWriter()
    invalidator = get_cache_invalidator()

    window = MainWindow(loader, extractor, writer, invalidator)
    window.setWindowIcon(icon)
    window.show()

    sys.exit(app.exec())


if __name__ == "__main__":
    main()
