"""Tests for lazy adaptive filmstrip thumbnails."""

from __future__ import annotations

from unittest.mock import MagicMock

import pytest
from PIL import Image
from PySide6.QtCore import QPoint, Qt
from PySide6.QtTest import QTest

from video_thumbnailer.models import VideoFile, VideoFormat
from video_thumbnailer.ui.filmstrip_widget import FilmstripWidget


@pytest.fixture()
def filmstrip(qtbot) -> FilmstripWidget:  # type: ignore[type-arg]
    widget = FilmstripWidget()
    qtbot.addWidget(widget)
    widget.resize(700, widget.sizeHint().height())
    widget.show()
    return widget


def _video(duration_ms: int = 60_000) -> VideoFile:
    return VideoFile(
        path="/fake/video.mp4",
        format=VideoFormat.MP4,
        duration_ms=duration_ms,
        width=1920,
        height=1080,
        existing_thumbnail=None,
        is_writable=True,
        frame_step_ms=40,
    )


class TestFilmstripWidget:
    def test_samples_cover_video_duration_evenly(self, filmstrip: FilmstripWidget) -> None:
        filmstrip._dispatch_queue = MagicMock()
        filmstrip.set_video(_video(), MagicMock())

        samples = [position for _, position in filmstrip._positions]

        assert len(samples) == (filmstrip.width() + 4) // 132
        assert samples[0] == 0
        assert samples[-1] == 60_000
        assert samples == sorted(samples)

        tiles = [
            filmstrip._tile_rect(index)
            for index in range(len(filmstrip._positions))
        ]
        assert tiles[0].x() == 0
        assert tiles[-1].x() + tiles[-1].width() == filmstrip.width()
        assert all(
            current.x() == previous.x() + previous.width()
            for previous, current in zip(tiles, tiles[1:])
        )

    def test_resize_changes_visible_sample_count(self, filmstrip: FilmstripWidget) -> None:
        filmstrip._dispatch_queue = MagicMock()
        filmstrip.set_video(_video(), MagicMock())
        wide_count = len(filmstrip._positions)

        filmstrip.resize(400, filmstrip.height())

        assert len(filmstrip._positions) < wide_count

    def test_click_emits_the_tile_timestamp(
        self, filmstrip: FilmstripWidget
    ) -> None:
        filmstrip._dispatch_queue = MagicMock()
        filmstrip.set_video(_video(), MagicMock())
        received: list[int] = []
        filmstrip.positionSelected.connect(received.append)
        expected = filmstrip._positions[1][1]

        QTest.mouseClick(
            filmstrip,
            Qt.MouseButton.LeftButton,
            pos=QPoint(132 + 10, filmstrip.height() // 2),
        )

        assert received == [expected]

    def test_thumbnail_requests_are_bounded_and_fill_as_ready(
        self, filmstrip: FilmstripWidget, qtbot
    ) -> None:
        extractor = MagicMock()
        extractor.extract.return_value = Image.new("RGB", (320, 180), "blue")
        filmstrip.set_video(_video(), extractor)

        assert len(filmstrip._in_flight) <= 3
        qtbot.waitUntil(
            lambda: len(filmstrip._frames) == len(filmstrip._positions),
            timeout=10_000,
        )
        assert extractor.extract.call_count == len(filmstrip._positions)

    def test_resize_requests_new_samples_lazily(
        self, filmstrip: FilmstripWidget, qtbot
    ) -> None:
        extractor = MagicMock()
        extractor.extract.return_value = Image.new("RGB", (320, 180), "blue")
        filmstrip.set_video(_video(), extractor)
        qtbot.waitUntil(
            lambda: len(filmstrip._frames) == len(filmstrip._positions),
            timeout=10_000,
        )
        initial_positions = set(filmstrip._frames)

        filmstrip.resize(1200, filmstrip.height())

        assert len(filmstrip._positions) > len(initial_positions)
        qtbot.waitUntil(
            lambda: all(
                frame_index in filmstrip._frames
                for frame_index, _ in filmstrip._positions
            ),
            timeout=10_000,
        )
        assert set(filmstrip._frames) > initial_positions
