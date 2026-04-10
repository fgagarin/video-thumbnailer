"""Platform package for video-thumbnailer.

Provides the CacheInvalidator protocol and a factory function that returns the
platform-appropriate implementation at runtime.
"""

from __future__ import annotations

import sys
from typing import TYPE_CHECKING, Protocol

if TYPE_CHECKING:
    from video_thumbnailer.models import VideoFile

__all__ = [
    "CacheInvalidator",
    "get_cache_invalidator",
]


class CacheInvalidator(Protocol):
    """Notify the host OS file manager to refresh the thumbnail for a given file.

    Implementations must never raise — failures are logged as warnings only.
    """

    def invalidate(self, video: VideoFile) -> None:
        """Trigger a file-manager thumbnail cache refresh for ``video.path``.

        Args:
            video: VideoFile whose path should be refreshed; ``existing_thumbnail``
                   should already reflect the newly applied frame so that the Linux
                   XDG PNG writer can use it.

        Returns:
            None. Cache invalidation is best-effort; failures are logged but never
            propagated to the caller.
        """
        ...


def get_cache_invalidator() -> CacheInvalidator:
    """Return the CacheInvalidator appropriate for the current platform.

    Returns:
        A CacheInvalidator instance for Linux, macOS, or Windows.

    Raises:
        RuntimeError: If the current platform is not supported.
    """
    if sys.platform == "linux":
        from video_thumbnailer.platform.cache_linux import LinuxCacheInvalidator

        return LinuxCacheInvalidator()
    if sys.platform == "darwin":
        from video_thumbnailer.platform.cache_macos import MacOSCacheInvalidator

        return MacOSCacheInvalidator()
    if sys.platform == "win32":
        from video_thumbnailer.platform.cache_windows import WindowsCacheInvalidator

        return WindowsCacheInvalidator()
    raise RuntimeError(f"Unsupported platform: {sys.platform}")
