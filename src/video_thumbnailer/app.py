"""Application entry point for video-thumbnailer."""

from __future__ import annotations

import sys


def main() -> None:
    """Launch the video-thumbnailer GUI application."""
    from PySide6.QtWidgets import QApplication

    from video_thumbnailer.core.frame_extractor import PyAVFrameExtractor
    from video_thumbnailer.core.thumbnail_writer import FormatDispatchThumbnailWriter
    from video_thumbnailer.core.video_loader import PyAVVideoLoader
    from video_thumbnailer.platform import get_cache_invalidator
    from video_thumbnailer.ui.main_window import MainWindow

    app = QApplication(sys.argv)

    loader = PyAVVideoLoader()
    extractor = PyAVFrameExtractor()
    writer = FormatDispatchThumbnailWriter()
    invalidator = get_cache_invalidator()

    window = MainWindow(loader, extractor, writer, invalidator)
    window.show()

    sys.exit(app.exec())


if __name__ == "__main__":
    main()
