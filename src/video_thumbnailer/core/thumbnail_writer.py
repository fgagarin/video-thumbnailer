"""Thumbnail writer: embeds cover-art into video files using format-appropriate tools.

Phase 1 supports MP4 and MOV only. MKV, WebM, AVI, and FLV support is added in T033.
"""

from __future__ import annotations

import errno
import io
import logging
import os
import subprocess
import tempfile
import time
from pathlib import Path

import imageio_ffmpeg  # type: ignore[import-untyped]
from PIL import Image

from video_thumbnailer.core.atomic_write import atomic_replace
from video_thumbnailer.models import ApplyError, ApplyResult, VideoFile, VideoFormat

__all__ = ["FormatDispatchThumbnailWriter"]

logger = logging.getLogger(__name__)

_MAX_THUMB_W = 640
_MAX_THUMB_H = 360
_JPEG_QUALITY = 90

_MP4_MOV_FORMATS = frozenset({VideoFormat.MP4, VideoFormat.MOV})
_MKV_FORMATS = frozenset({VideoFormat.MKV})
_WEBM_FORMATS = frozenset({VideoFormat.WEBM})
_AVI_FORMATS = frozenset({VideoFormat.AVI})
_FLV_FORMATS = frozenset({VideoFormat.FLV})
_NO_EMBED_FORMATS = _WEBM_FORMATS | _FLV_FORMATS
_ALL_SUPPORTED = (
    _MP4_MOV_FORMATS | _MKV_FORMATS | _WEBM_FORMATS | _AVI_FORMATS | _FLV_FORMATS
)


class FormatDispatchThumbnailWriter:
    """Embed a thumbnail into a video file using the format-appropriate mechanism.

    Implements the ThumbnailWriter protocol from contracts/core_interfaces.md.

    Supported formats:
        MP4, MOV — via ffmpeg ``-disposition:v:1 attached_pic`` strategy.
        MKV, WebM — via ffmpeg ``-attach`` strategy.
        AVI — re-mux without cover art (AVI does not support embedded thumbnails).
        FLV — XDG cache only (FLV does not support embedded thumbnails).

    All mux operations use atomic_replace to guarantee the original file is never
    left in a corrupt state on failure.
    """

    def write(self, video: VideoFile, thumbnail: Image.Image) -> ApplyResult:
        """Embed ``thumbnail`` as cover art in ``video``.

        Args:
            video: The video file to modify.
            thumbnail: PIL Image to use as cover art (will be resized/compressed).

        Returns:
            ApplyResult with success=True on success, or success=False with an
            error_code and human-readable error_message on any failure.
            Never raises — all errors are encoded in the result.
        """
        start = time.monotonic()

        if not video.is_writable:
            return ApplyResult(
                success=False,
                error_code=ApplyError.FILE_NOT_WRITABLE,
                error_message=(
                    f"'{video.path}' is not writable. "
                    "Copy the file to a writable location and try again."
                ),
            )

        if video.format not in _ALL_SUPPORTED:
            return ApplyResult(
                success=False,
                error_code=ApplyError.UNSUPPORTED_FORMAT,
                error_message=(
                    f"Format {video.format.name} is not supported. "
                    "Supported formats: MP4, MOV."
                ),
            )

        # Scale and encode thumbnail to JPEG
        cover_image = thumbnail.copy()
        cover_image.thumbnail((_MAX_THUMB_W, _MAX_THUMB_H), Image.Resampling.LANCZOS)

        jpeg_buf = io.BytesIO()
        cover_image.convert("RGB").save(jpeg_buf, format="JPEG", quality=_JPEG_QUALITY)
        jpeg_bytes = jpeg_buf.getvalue()

        # FLV / WebM: no container embedding — return success immediately
        if video.format in _NO_EMBED_FORMATS:
            elapsed_ms = int((time.monotonic() - start) * 1000)
            return ApplyResult(
                success=True,
                elapsed_ms=elapsed_ms,
                error_message=(
                    f"{video.format.name} does not support embedded cover art; "
                    "Linux file manager icon updated via XDG cache"
                ),
            )

        try:
            if video.format in _MP4_MOV_FORMATS:
                self._embed_mp4_mov(video.path, jpeg_bytes)
            elif video.format in _MKV_FORMATS:
                self._embed_mkv_webm(video.path, jpeg_bytes)
            elif video.format in _AVI_FORMATS:
                self._remux_avi(video.path)
                elapsed_ms = int((time.monotonic() - start) * 1000)
                return ApplyResult(
                    success=True,
                    elapsed_ms=elapsed_ms,
                    error_message=(
                        "AVI does not support embedded cover art; "
                        "file manager will generate its own preview"
                    ),
                )
        except PermissionError as exc:
            return ApplyResult(
                success=False,
                error_code=ApplyError.FILE_NOT_WRITABLE,
                error_message=f"Permission denied writing '{video.path}': {exc}",
            )
        except subprocess.CalledProcessError as exc:
            stderr = exc.stderr.decode(errors="replace") if exc.stderr else "(none)"
            return ApplyResult(
                success=False,
                error_code=ApplyError.FFMPEG_ERROR,
                error_message=(
                    f"ffmpeg failed (exit {exc.returncode}). stderr: {stderr}"
                ),
            )
        except OSError as exc:
            if exc.errno == errno.ENOSPC:
                return ApplyResult(
                    success=False,
                    error_code=ApplyError.DISK_FULL,
                    error_message=(
                        "Not enough disk space to write the updated file."
                        " Free space and retry."
                    ),
                )
            return ApplyResult(
                success=False,
                error_code=ApplyError.UNEXPECTED,
                error_message=f"Unexpected OS error: {exc}",
            )
        except Exception as exc:  # noqa: BLE001
            return ApplyResult(
                success=False,
                error_code=ApplyError.UNEXPECTED,
                error_message=f"Unexpected error: {exc}",
            )

        elapsed_ms = int((time.monotonic() - start) * 1000)
        return ApplyResult(success=True, elapsed_ms=elapsed_ms)

    # ------------------------------------------------------------------
    # Format-specific embedding helpers
    # ------------------------------------------------------------------

    def _embed_mp4_mov(self, video_path: str, jpeg_bytes: bytes) -> None:
        """Embed cover art into an MP4 or MOV file using ffmpeg subprocess."""
        ffmpeg_exe = imageio_ffmpeg.get_ffmpeg_exe()
        # Write the JPEG bytes to a temp file so ffmpeg can read them
        video_dir = os.path.dirname(os.path.abspath(video_path))
        # Preserve the video extension so ffmpeg can auto-detect the output format
        video_ext = os.path.splitext(video_path)[1].lower() or ".mp4"
        cover_fd, cover_path = tempfile.mkstemp(
            dir=video_dir, prefix=".vt_cover_", suffix=".jpg"
        )
        try:
            with os.fdopen(cover_fd, "wb") as f:
                f.write(jpeg_bytes)

            def _ffmpeg_write_fn(tmp_video_path: Path) -> None:
                out_fd, out_path = tempfile.mkstemp(
                    dir=video_dir, prefix=".vt_out_", suffix=video_ext
                )
                os.close(out_fd)
                try:
                    subprocess.run(
                        [
                            ffmpeg_exe, "-y",
                            "-i", str(tmp_video_path),
                            "-i", cover_path,
                            "-map", "0",
                            "-map", "1",
                            "-c", "copy",
                            "-disposition:v:1", "attached_pic",
                            "-f", "mp4",
                            out_path,
                        ],
                        check=True,
                        capture_output=True,
                    )
                    os.replace(out_path, tmp_video_path)
                except Exception:
                    try:
                        os.unlink(out_path)
                    except FileNotFoundError:
                        pass
                    raise

            atomic_replace(video_path, _ffmpeg_write_fn)
        finally:
            try:
                os.unlink(cover_path)
            except FileNotFoundError:
                pass

    def _embed_mkv_webm(self, video_path: str, jpeg_bytes: bytes) -> None:
        """Embed cover art into an MKV or WebM file using ffmpeg -attach."""
        ffmpeg_exe = imageio_ffmpeg.get_ffmpeg_exe()
        video_dir = os.path.dirname(os.path.abspath(video_path))
        video_ext = os.path.splitext(video_path)[1].lower() or ".mkv"

        cover_fd, cover_path = tempfile.mkstemp(
            dir=video_dir, prefix=".vt_cover_", suffix=".jpg"
        )
        try:
            with os.fdopen(cover_fd, "wb") as f:
                f.write(jpeg_bytes)

            def _mkv_write_fn(tmp_video_path: Path) -> None:
                out_fd, out_path = tempfile.mkstemp(
                    dir=video_dir, prefix=".vt_out_", suffix=video_ext
                )
                os.close(out_fd)
                try:
                    subprocess.run(
                        [
                            ffmpeg_exe, "-y",
                            "-i", str(tmp_video_path),
                            "-attach", cover_path,
                            "-metadata:s:t", "mimetype=image/jpeg",
                            "-metadata:s:t", "filename=cover.jpg",
                            "-c", "copy",
                            out_path,
                        ],
                        check=True,
                        capture_output=True,
                    )
                    os.replace(out_path, tmp_video_path)
                except Exception:
                    try:
                        os.unlink(out_path)
                    except FileNotFoundError:
                        pass
                    raise

            atomic_replace(video_path, _mkv_write_fn)
        finally:
            try:
                os.unlink(cover_path)
            except FileNotFoundError:
                pass

    def _remux_avi(self, video_path: str) -> None:
        """Re-mux an AVI file in-place (no cover art; preserves all streams)."""
        ffmpeg_exe = imageio_ffmpeg.get_ffmpeg_exe()
        video_dir = os.path.dirname(os.path.abspath(video_path))

        def _avi_write_fn(tmp_video_path: Path) -> None:
            out_fd, out_path = tempfile.mkstemp(
                dir=video_dir, prefix=".vt_out_", suffix=".avi"
            )
            os.close(out_fd)
            try:
                subprocess.run(
                    [
                        ffmpeg_exe, "-y",
                        "-i", str(tmp_video_path),
                        "-c", "copy",
                        out_path,
                    ],
                    check=True,
                    capture_output=True,
                )
                os.replace(out_path, tmp_video_path)
            except Exception:
                try:
                    os.unlink(out_path)
                except FileNotFoundError:
                    pass
                raise

        atomic_replace(video_path, _avi_write_fn)
