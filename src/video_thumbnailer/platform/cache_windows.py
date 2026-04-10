"""Windows Explorer SHChangeNotify thumbnail cache invalidator."""

from __future__ import annotations

import logging
import os
import sys
from typing import TYPE_CHECKING

if TYPE_CHECKING:
    from video_thumbnailer.models import VideoFile

logger = logging.getLogger(__name__)

__all__ = ["WindowsCacheInvalidator"]

# SHChangeNotify constants
_SHCNE_UPDATEITEM: int = 0x00000001
_SHCNF_PATHW: int = 0x00000005


class WindowsCacheInvalidator:
    """Notify Windows Explorer to refresh the thumbnail for a file."""

    def invalidate(self, video: VideoFile) -> None:
        """Call SHChangeNotify so Explorer drops its cached thumbnail.

        Failures are logged but never raised.

        Args:
            video: The updated video file.
        """
        if sys.platform != "win32":
            logger.warning(
                "WindowsCacheInvalidator called on non-Windows platform (%s); skipping.",  # noqa: E501
                sys.platform,
            )
            return

        try:
            import ctypes

            abs_path = os.path.abspath(video.path)
            ctypes.windll.shell32.SHChangeNotify(
                _SHCNE_UPDATEITEM,
                _SHCNF_PATHW,
                abs_path,
                None,
            )
        except (AttributeError, OSError) as exc:
            logger.warning("SHChangeNotify failed for '%s': %s", video.path, exc)
