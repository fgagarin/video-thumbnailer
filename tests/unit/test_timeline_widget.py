"""Unit tests for TimelineWidget (T039 coverage boost)."""

from __future__ import annotations

import pytest
from PySide6.QtCore import QPoint, Qt
from PySide6.QtTest import QTest

from video_thumbnailer.ui.timeline_widget import TimelineWidget


@pytest.fixture()
def widget(qtbot) -> TimelineWidget:  # type: ignore[type-arg]
    w = TimelineWidget()
    qtbot.addWidget(w)
    w.resize(300, 50)
    w.show()
    return w


class TestTimelineWidget:
    def test_initial_state(self, widget: TimelineWidget) -> None:
        assert widget._duration_ms == 0
        assert widget._position_ms == 0
        assert widget._dragging is False

    def test_set_duration_resets_position(self, widget: TimelineWidget) -> None:
        widget.set_duration(60_000)
        widget.set_position(30_000)
        widget.set_duration(10_000)
        assert widget._duration_ms == 10_000
        assert widget._position_ms == 0

    def test_set_duration_clamps_negative(self, widget: TimelineWidget) -> None:
        widget.set_duration(-1000)
        assert widget._duration_ms == 0

    def test_set_position_clamps_to_zero(self, widget: TimelineWidget) -> None:
        widget.set_duration(5000)
        widget.set_position(-100)
        assert widget._position_ms == 0

    def test_set_position_clamps_to_duration(self, widget: TimelineWidget) -> None:
        widget.set_duration(5000)
        widget.set_position(99999)
        assert widget._position_ms == 5000

    def test_set_position_midpoint(self, widget: TimelineWidget) -> None:
        widget.set_duration(10_000)
        widget.set_position(5_000)
        assert widget._position_ms == 5_000

    def test_size_hint(self, widget: TimelineWidget) -> None:
        hint = widget.sizeHint()
        assert hint.width() == 300
        assert hint.height() == 40

    def test_position_changed_signal_on_mouse_release(
        self, widget: TimelineWidget, qtbot
    ) -> None:
        widget.set_duration(10_000)
        received: list[int] = []
        widget.positionChanged.connect(received.append)

        # Simulate click at centre → position = ~5000ms
        centre_x = widget.width() // 2
        centre_y = widget.height() // 2
        QTest.mousePress(widget, Qt.MouseButton.LeftButton, pos=QPoint(centre_x, centre_y))
        QTest.mouseRelease(widget, Qt.MouseButton.LeftButton, pos=QPoint(centre_x, centre_y))

        assert len(received) == 1
        assert 3000 <= received[0] <= 7000  # rough range for centre click

    def test_no_signal_when_duration_zero(
        self, widget: TimelineWidget, qtbot
    ) -> None:
        received: list[int] = []
        widget.positionChanged.connect(received.append)

        QTest.mousePress(widget, Qt.MouseButton.LeftButton, pos=QPoint(50, 20))
        QTest.mouseRelease(widget, Qt.MouseButton.LeftButton, pos=QPoint(50, 20))

        # Duration is 0; no position change should be emitted
        assert received == []

    def test_paint_event_does_not_raise(self, widget: TimelineWidget, qtbot) -> None:
        widget.set_duration(5000)
        widget.set_position(2500)
        widget.update()
        qtbot.wait(50)  # Let Qt process paint event

    def test_paint_event_zero_duration_does_not_raise(
        self, widget: TimelineWidget, qtbot
    ) -> None:
        widget.update()
        qtbot.wait(50)

    def test_mouse_move_during_drag_updates_position(
        self, widget: TimelineWidget, qtbot
    ) -> None:
        widget.set_duration(10_000)

        QTest.mousePress(widget, Qt.MouseButton.LeftButton, pos=QPoint(0, 20))
        QTest.mouseMove(widget, pos=QPoint(widget.width(), 20))
        assert widget._dragging is True
        assert widget._position_ms >= 9000  # close to end

        QTest.mouseRelease(widget, Qt.MouseButton.LeftButton, pos=QPoint(widget.width(), 20))
        assert widget._dragging is False

    def test_emit_position_emits_signal(self, widget: TimelineWidget) -> None:
        widget.set_duration(5000)
        widget._position_ms = 2500
        received: list[int] = []
        widget.positionChanged.connect(received.append)
        widget._emit_position()
        assert received == [2500]
