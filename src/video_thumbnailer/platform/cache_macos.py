"""macOS Quick Look / Finder thumbnail cache invalidator."""

from __future__ import annotations

import logging
import os
import subprocess
from typing import TYPE_CHECKING

if TYPE_CHECKING:
    from video_thumbnailer.models import VideoFile

logger = logging.getLogger(__name__)

__all__ = ["MacOSCacheInvalidator"]


class MacOSCacheInvalidator:
    """Clear the Quick Look cache and re-index the video file in Spotlight."""

    def invalidate(self, video: VideoFile) -> None:
        """Trigger a Finder / Quick Look thumbnail refresh for ``video.path``.

        Failures are logged but never raised.

        Args:
            video: The updated video file.
        """
        try:
            subprocess.run(
                ["qlmanage", "-r", "cache"],
                timeout=10,
                capture_output=True,
                check=False,
            )
        except Exception as exc:  # noqa: BLE001
            logger.warning("qlmanage failed: %s", exc)

        try:
            os.utime(video.path, None)
        except Exception as exc:  # noqa: BLE001
            logger.warning("os.utime failed for '%s': %s", video.path, exc)

        try:
            subprocess.run(
                ["mdimport", video.path],
                timeout=10,
                capture_output=True,
                check=False,
            )
        except Exception as exc:  # noqa: BLE001
            logger.warning("mdimport failed for '%s': %s", video.path, exc)
