"""Benchmark tests for frame extraction performance (T037).

Reference hardware: AMD Ryzen 5 (desktop), 32 GB RAM, NVMe SSD.
Measured baseline (single-threaded, cold PyAV open):
    SD  (320×240):   ~5–20 ms
    HD  (1920×1080): ~15–60 ms
    4K  (3840×2160): ~40–150 ms

The 500 ms assertion gives a generous upper bound that should pass on any
reasonably modern development machine without a GPU decoder.
"""

from __future__ import annotations

import subprocess
from pathlib import Path

import imageio_ffmpeg
import pytest
from PIL import Image

from video_thumbnailer.core.frame_extractor import PyAVFrameExtractor
from video_thumbnailer.core.video_loader import PyAVVideoLoader
from video_thumbnailer.models import TimelinePosition

_FFMPEG = imageio_ffmpeg.get_ffmpeg_exe()


def _make_mp4(output: Path, width: int, height: int, duration: int = 5) -> None:
    subprocess.run(
        [
            _FFMPEG, "-y",
            "-f", "lavfi",
            "-i", f"testsrc=size={width}x{height}:rate=25:duration={duration}",
            "-f", "lavfi",
            "-i", f"sine=frequency=440:duration={duration}",
            "-c:v", "libx264", "-c:a", "aac", "-pix_fmt", "yuv420p",
            str(output),
        ],
        check=True,
        capture_output=True,
    )


# Fixtures ──────────────────────────────────────────────────────────────────

@pytest.fixture(scope="module")
def sd_mp4(tmp_path_factory: pytest.TempPathFactory) -> Path:
    """320×240 SD test video, reused across benchmarks."""
    p = tmp_path_factory.mktemp("bench") / "sample_sd.mp4"
    _make_mp4(p, 320, 240)
    return p


@pytest.fixture(scope="module")
def hd_mp4(tmp_path_factory: pytest.TempPathFactory) -> Path:
    """1920×1080 HD test video."""
    p = tmp_path_factory.mktemp("bench") / "sample_hd.mp4"
    _make_mp4(p, 1920, 1080)
    return p


@pytest.fixture(scope="module")
def uhd_mp4(tmp_path_factory: pytest.TempPathFactory) -> Path:
    """3840×2160 4K test video."""
    p = tmp_path_factory.mktemp("bench") / "sample_4k.mp4"
    _make_mp4(p, 3840, 2160)
    return p


# Benchmarks ────────────────────────────────────────────────────────────────

def test_frame_extract_sd(benchmark: pytest.fixture, sd_mp4: Path) -> None:  # type: ignore[type-arg]
    """Benchmark: frame extraction from 320×240 SD MP4.

    Assertion: worst-case (max) single run under 500 ms.
    """
    loader = PyAVVideoLoader()
    extractor = PyAVFrameExtractor()
    video = loader.load(str(sd_mp4))
    pos = TimelinePosition(offset_ms=2500)

    result: Image.Image = benchmark(extractor.extract, video, pos)

    assert result is not None
    assert benchmark.stats["max"] < 0.5, (
        f"frame_extract_SD max={benchmark.stats['max']:.3f}s exceeds 0.5s limit"
    )


def test_frame_extract_1080p(benchmark: pytest.fixture, hd_mp4: Path) -> None:  # type: ignore[type-arg]
    """Benchmark: frame extraction from 1920×1080 HD MP4.

    Assertion: worst-case (max) single run under 500 ms.
    """
    loader = PyAVVideoLoader()
    extractor = PyAVFrameExtractor()
    video = loader.load(str(hd_mp4))
    pos = TimelinePosition(offset_ms=2500)

    result: Image.Image = benchmark(extractor.extract, video, pos)

    assert result is not None
    assert benchmark.stats["max"] < 0.5, (
        f"frame_extract_1080p max={benchmark.stats['max']:.3f}s exceeds 0.5s limit"
    )


def test_frame_extract_4k(benchmark: pytest.fixture, uhd_mp4: Path) -> None:  # type: ignore[type-arg]
    """Benchmark: frame extraction from 3840×2160 4K MP4.

    Assertion: worst-case (max) single run under 500 ms.
    """
    loader = PyAVVideoLoader()
    extractor = PyAVFrameExtractor()
    video = loader.load(str(uhd_mp4))
    pos = TimelinePosition(offset_ms=2500)

    result: Image.Image = benchmark(extractor.extract, video, pos)

    assert result is not None
    assert benchmark.stats["max"] < 0.5, (
        f"frame_extract_4K max={benchmark.stats['max']:.3f}s exceeds 0.5s limit"
    )
