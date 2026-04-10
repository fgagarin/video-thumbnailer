"""Linux XDG thumbnail cache invalidator.

Writes a Freedesktop-spec PNG thumbnail to ~/.cache/thumbnails/large/ and
optionally sends a D-Bus signal to notifiy file managers.

Reference: https://specifications.freedesktop.org/thumbnail-spec/thumbnail-spec-latest.html
"""

from __future__ import annotations

import hashlib
import io
import logging
import os
import subprocess
from pathlib import Path
from typing import TYPE_CHECKING

from PIL import Image, PngImagePlugin

from video_thumbnailer.core.atomic_write import atomic_replace

if TYPE_CHECKING:
    from video_thumbnailer.models import VideoFile

logger = logging.getLogger(__name__)

__all__ = ["LinuxCacheInvalidator"]

_MAX_THUMB_SIZE = 256


class LinuxCacheInvalidator:
    """Write an XDG thumbnail entry and notify file managers via D-Bus."""

    def invalidate(self, video: VideoFile) -> None:
        """Refresh the system thumbnail cache for ``video.path``.

        Writes to ``~/.cache/thumbnails/large/<md5>.png`` and attempts to notify
        the file manager via D-Bus. Failures are logged but never raised.

        Args:
            video: The updated video file. ``existing_thumbnail`` should reflect
                   the newly applied frame so it is embedded in the XDG PNG.
        """
        try:
            self._write_xdg_thumbnail(video)
        except Exception as exc:  # noqa: BLE001
            logger.warning(
                "Failed to write XDG thumbnail for '%s': %s", video.path, exc
            )

        try:
            self._notify_dbus(video.path)
        except Exception as exc:  # noqa: BLE001
            logger.warning("D-Bus notification failed for '%s': %s", video.path, exc)

    # ------------------------------------------------------------------
    # Private helpers
    # ------------------------------------------------------------------

    def _write_xdg_thumbnail(self, video: VideoFile) -> None:
        abs_path = os.path.abspath(video.path)
        file_uri = "file://" + abs_path
        md5_hex = hashlib.md5(file_uri.encode()).hexdigest()

        thumb_dir = os.path.expanduser("~/.cache/thumbnails/large")
        os.makedirs(thumb_dir, exist_ok=True)

        dest_path = os.path.join(thumb_dir, f"{md5_hex}.png")

        # Use the newly applied thumbnail or a 1×1 placeholder
        if video.existing_thumbnail is not None:
            img = video.existing_thumbnail.copy()
        else:
            img = Image.new("RGB", (1, 1), color=(128, 128, 128))

        img.thumbnail((_MAX_THUMB_SIZE, _MAX_THUMB_SIZE), Image.Resampling.LANCZOS)
        img = img.convert("RGBA")  # PNG with transparency

        stat = os.stat(abs_path)
        pnginfo = PngImagePlugin.PngInfo()
        pnginfo.add_text("Thumb::URI", file_uri)
        pnginfo.add_text("Thumb::MTime", str(int(stat.st_mtime)))
        pnginfo.add_text("Thumb::Size", str(stat.st_size))

        buf = io.BytesIO()
        img.save(buf, format="PNG", pnginfo=pnginfo)
        png_bytes = buf.getvalue()

        def _write_png(tmp: Path) -> None:
            tmp.write_bytes(png_bytes)

        atomic_replace(dest_path, _write_png)

    def _notify_dbus(self, path: str) -> None:
        abs_path = os.path.abspath(path)
        file_uri = "file://" + abs_path
        subprocess.run(
            [
                "dbus-send",
                "--session",
                "--dest=org.freedesktop.FileManager1",
                "--type=method_call",
                "/org/freedesktop/FileManager1",
                "org.freedesktop.FileManager1.ShowItems",
                f"array:string:{file_uri}",
                "string:",
            ],
            timeout=3,
            capture_output=True,
            check=True,
        )
