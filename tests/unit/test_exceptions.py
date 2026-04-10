"""Unit tests for custom exceptions (T039 coverage boost)."""

from __future__ import annotations

import pytest

from video_thumbnailer.exceptions import (
    ExtractionError,
    NoVideoStreamError,
    UnsupportedFormatError,
    VideoThumbnailerError,
)


class TestUnsupportedFormatError:
    def test_is_video_thumbnailer_error(self) -> None:
        exc = UnsupportedFormatError("/path/to/video.rm")
        assert isinstance(exc, VideoThumbnailerError)

    def test_path_attribute(self) -> None:
        exc = UnsupportedFormatError("/path/to/video.rm")
        assert exc.path == "/path/to/video.rm"

    def test_message_contains_path(self) -> None:
        exc = UnsupportedFormatError("/path/to/video.rm")
        assert "/path/to/video.rm" in str(exc)

    def test_raise_and_catch(self) -> None:
        with pytest.raises(UnsupportedFormatError) as exc_info:
            raise UnsupportedFormatError("/bad.xyz")
        assert exc_info.value.path == "/bad.xyz"


class TestNoVideoStreamError:
    def test_is_video_thumbnailer_error(self) -> None:
        exc = NoVideoStreamError("/path/to/audio.mp3")
        assert isinstance(exc, VideoThumbnailerError)

    def test_path_attribute(self) -> None:
        exc = NoVideoStreamError("/path/to/audio.mp3")
        assert exc.path == "/path/to/audio.mp3"

    def test_message_contains_path(self) -> None:
        exc = NoVideoStreamError("/path/to/audio.mp3")
        assert "/path/to/audio.mp3" in str(exc)

    def test_raise_and_catch(self) -> None:
        with pytest.raises(NoVideoStreamError) as exc_info:
            raise NoVideoStreamError("/audio.mp3")
        assert exc_info.value.path == "/audio.mp3"


class TestExtractionError:
    def test_is_video_thumbnailer_error(self) -> None:
        exc = ExtractionError("/video.mp4", 1000, "codec fail")
        assert isinstance(exc, VideoThumbnailerError)

    def test_attributes(self) -> None:
        exc = ExtractionError("/video.mp4", 2500, "seek error")
        assert exc.path == "/video.mp4"
        assert exc.offset_ms == 2500
        assert exc.cause == "seek error"

    def test_message_contains_details(self) -> None:
        exc = ExtractionError("/video.mp4", 3000, "timeout")
        msg = str(exc)
        assert "3000" in msg
        assert "timeout" in msg
        assert "/video.mp4" in msg

    def test_raise_and_catch(self) -> None:
        with pytest.raises(ExtractionError) as exc_info:
            raise ExtractionError("/vid.mkv", 500, "eof")
        assert exc_info.value.offset_ms == 500
        assert exc_info.value.cause == "eof"
