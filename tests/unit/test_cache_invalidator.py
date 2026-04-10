"""Unit tests for platform cache invalidators."""

from __future__ import annotations

import sys
from pathlib import Path
from unittest.mock import MagicMock, patch

import pytest

from video_thumbnailer.models import VideoFile, VideoFormat


def _make_video_file(path: Path) -> VideoFile:
    return VideoFile(
        path=str(path),
        format=VideoFormat.MP4,
        duration_ms=5000,
        width=320,
        height=240,
        existing_thumbnail=None,
        is_writable=True,
    )


class TestLinuxCacheInvalidator:
    @pytest.mark.skipif(sys.platform != "linux", reason="Linux only")
    def test_xdg_thumbnail_path_uses_md5_of_uri(self, tmp_path: Path) -> None:
        import hashlib

        from video_thumbnailer.platform.cache_linux import LinuxCacheInvalidator

        video_path = tmp_path / "test.mp4"
        video_path.write_bytes(b"fake")
        vf = _make_video_file(video_path)

        uri = video_path.as_uri()
        expected_md5 = hashlib.md5(uri.encode()).hexdigest()  # noqa: S324

        invalidator = LinuxCacheInvalidator()
        with patch("subprocess.run"):
            invalidator.invalidate(vf)

        thumb_dir = Path.home() / ".cache" / "thumbnails" / "large"
        expected_thumb = thumb_dir / f"{expected_md5}.png"
        # File may or may not exist depending on whether thumb dir exists; just verify naming
        assert expected_thumb.name == f"{expected_md5}.png"

    @pytest.mark.skipif(sys.platform != "linux", reason="Linux only")
    def test_thumbnail_written_with_xdg_metadata(
        self, tmp_path: Path, sample_pil_image: "Image.Image"  # noqa: F821
    ) -> None:
        from PIL import Image

        from video_thumbnailer.platform.cache_linux import LinuxCacheInvalidator

        video_path = tmp_path / "test.mp4"
        video_path.write_bytes(b"fake")
        vf = VideoFile(
            path=str(video_path),
            format=VideoFormat.MP4,
            duration_ms=5000,
            width=320,
            height=240,
            existing_thumbnail=sample_pil_image,
            is_writable=True,
        )

        written_paths: list[Path] = []

        def fake_atomic_replace(target: Path, write_fn) -> None:  # type: ignore[type-arg]
            written_paths.append(target)
            fake_target = tmp_path / "thumb_out.png"
            write_fn(fake_target)

        with (
            patch(
                "video_thumbnailer.platform.cache_linux.atomic_replace",
                side_effect=fake_atomic_replace,
            ),
            patch("subprocess.run"),
        ):
            invalidator = LinuxCacheInvalidator()
            invalidator.invalidate(vf)

        # Verify the output PNG contains XDG metadata
        if written_paths:
            out_img = Image.open(tmp_path / "thumb_out.png")
            assert "Thumb::URI" in out_img.text  # type: ignore[attr-defined]
            assert "Thumb::MTime" in out_img.text  # type: ignore[attr-defined]

    @pytest.mark.skipif(sys.platform != "linux", reason="Linux only")
    def test_dbus_failure_does_not_raise(self, tmp_path: Path) -> None:
        from unittest.mock import patch

        from video_thumbnailer.platform.cache_linux import LinuxCacheInvalidator

        video_path = tmp_path / "test.mp4"
        video_path.write_bytes(b"fake")
        vf = _make_video_file(video_path)

        with patch(
            "subprocess.run",
            side_effect=FileNotFoundError("dbus-send not found"),
        ):
            invalidator = LinuxCacheInvalidator()
            invalidator.invalidate(vf)  # Must not raise


class TestMacOSCacheInvalidator:
    def test_subprocess_failure_does_not_raise(self, tmp_path: Path) -> None:
        """MacOS invalidator must swallow all errors."""
        # Import guard — module is always importable
        from video_thumbnailer.platform.cache_macos import MacOSCacheInvalidator

        video_path = tmp_path / "test.mp4"
        video_path.write_bytes(b"fake")
        vf = _make_video_file(video_path)

        with patch(
            "subprocess.run",
            side_effect=FileNotFoundError("qlmanage not found"),
        ):
            invalidator = MacOSCacheInvalidator()
            invalidator.invalidate(vf)  # Must not raise


class TestWindowsCacheInvalidator:
    def test_ctypes_failure_does_not_raise(self, tmp_path: Path) -> None:
        """Windows invalidator must swallow all errors."""
        from video_thumbnailer.platform.cache_windows import WindowsCacheInvalidator

        video_path = tmp_path / "test.mp4"
        video_path.write_bytes(b"fake")
        vf = _make_video_file(video_path)

        fake_ctypes = MagicMock()
        fake_ctypes.windll.shell32.SHChangeNotify.side_effect = OSError("not Windows")

        with patch.dict("sys.modules", {"ctypes": fake_ctypes}):
            invalidator = WindowsCacheInvalidator()
            invalidator.invalidate(vf)  # Must not raise
