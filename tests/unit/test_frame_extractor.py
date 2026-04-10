"""Unit tests for PyAVFrameExtractor."""

from __future__ import annotations

from pathlib import Path

import pytest
from PIL import Image

from video_thumbnailer.core.frame_extractor import PyAVFrameExtractor
from video_thumbnailer.core.video_loader import PyAVVideoLoader
from video_thumbnailer.models import TimelinePosition


@pytest.fixture()
def loader() -> PyAVVideoLoader:
    return PyAVVideoLoader()


@pytest.fixture()
def extractor() -> PyAVFrameExtractor:
    return PyAVFrameExtractor()


class TestFrameExtractor:
    def test_extract_at_zero_returns_rgb_image(
        self,
        loader: PyAVVideoLoader,
        extractor: PyAVFrameExtractor,
        sample_video: Path,
    ) -> None:
        vf = loader.load(str(sample_video))
        img = extractor.extract(vf, TimelinePosition(offset_ms=0))
        assert isinstance(img, Image.Image)
        assert img.mode == "RGB"

    def test_extract_at_mid_duration(
        self,
        loader: PyAVVideoLoader,
        extractor: PyAVFrameExtractor,
        sample_video: Path,
    ) -> None:
        vf = loader.load(str(sample_video))
        mid = vf.duration_ms // 2
        img = extractor.extract(vf, TimelinePosition(offset_ms=mid))
        assert img is not None
        assert img.mode == "RGB"

    def test_extract_at_boundary(
        self,
        loader: PyAVVideoLoader,
        extractor: PyAVFrameExtractor,
        sample_video: Path,
    ) -> None:
        vf = loader.load(str(sample_video))
        img = extractor.extract(vf, TimelinePosition(offset_ms=vf.duration_ms))
        assert img is not None

    def test_extract_negative_offset_raises(
        self,
        loader: PyAVVideoLoader,
        extractor: PyAVFrameExtractor,
        sample_video: Path,
    ) -> None:
        vf = loader.load(str(sample_video))
        with pytest.raises(ValueError):
            extractor.extract(vf, TimelinePosition(offset_ms=-1))

    def test_extract_beyond_duration_raises(
        self,
        loader: PyAVVideoLoader,
        extractor: PyAVFrameExtractor,
        sample_video: Path,
    ) -> None:
        vf = loader.load(str(sample_video))
        with pytest.raises(ValueError):
            extractor.extract(vf, TimelinePosition(offset_ms=vf.duration_ms + 1))

    def test_returned_image_dimensions_match_stream(
        self,
        loader: PyAVVideoLoader,
        extractor: PyAVFrameExtractor,
        sample_video: Path,
    ) -> None:
        vf = loader.load(str(sample_video))
        img = extractor.extract(vf, TimelinePosition(offset_ms=0))
        assert img.width == vf.width
        assert img.height == vf.height
