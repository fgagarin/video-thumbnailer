"""Unit tests for platform/__init__.py: get_cache_invalidator (T039 coverage boost)."""

from __future__ import annotations

import sys
from unittest.mock import MagicMock, patch

import pytest


class TestGetCacheInvalidator:
    def test_linux_returns_linux_invalidator(self) -> None:
        if sys.platform != "linux":
            pytest.skip("Linux-only test")
        from video_thumbnailer.platform import get_cache_invalidator
        from video_thumbnailer.platform.cache_linux import LinuxCacheInvalidator

        result = get_cache_invalidator()
        assert isinstance(result, LinuxCacheInvalidator)

    def test_darwin_returns_macos_invalidator(self) -> None:
        mock_macos = MagicMock()
        mock_macos_class = MagicMock(return_value=mock_macos)

        with patch.dict(
            "sys.modules",
            {"video_thumbnailer.platform.cache_macos": MagicMock(MacOSCacheInvalidator=mock_macos_class)},
        ):
            with patch("sys.platform", "darwin"):
                # Re-import to pick up patched sys.platform
                import importlib

                import video_thumbnailer.platform as platform_mod
                importlib.reload(platform_mod)
                result = platform_mod.get_cache_invalidator()
                assert result is mock_macos

        # Restore the module
        import importlib

        import video_thumbnailer.platform as platform_mod
        importlib.reload(platform_mod)

    def test_win32_returns_windows_invalidator(self) -> None:
        mock_win = MagicMock()
        mock_win_class = MagicMock(return_value=mock_win)

        with patch.dict(
            "sys.modules",
            {"video_thumbnailer.platform.cache_windows": MagicMock(WindowsCacheInvalidator=mock_win_class)},
        ):
            with patch("sys.platform", "win32"):
                import importlib

                import video_thumbnailer.platform as platform_mod
                importlib.reload(platform_mod)
                result = platform_mod.get_cache_invalidator()
                assert result is mock_win

        # Restore
        import video_thumbnailer.platform as platform_mod
        importlib.reload(platform_mod)

    def test_unsupported_platform_raises(self) -> None:
        with patch("sys.platform", "freebsd"):
            import importlib

            import video_thumbnailer.platform as platform_mod
            importlib.reload(platform_mod)
            with pytest.raises(RuntimeError, match="Unsupported platform"):
                platform_mod.get_cache_invalidator()

        # Restore
        import video_thumbnailer.platform as platform_mod
        importlib.reload(platform_mod)
