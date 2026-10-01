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
from typing import Callable

import av
import imageio_ffmpeg  # type: ignore[import-untyped]
from PIL import Image

from video_thumbnailer.core.atomic_write import atomic_replace
from video_thumbnailer.core.thumbnail_metadata import write_thumbnail_source
from video_thumbnailer.models import ApplyError, ApplyResult, VideoFile, VideoFormat

__all__ = ["FormatDispatchThumbnailWriter"]

logger = logging.getLogger(__name__)

_MAX_THUMB_W = 640
_MAX_THUMB_H = 360
_JPEG_QUALITY = 90
_FFMPEG_CREATION_FLAGS = subprocess.CREATE_NO_WINDOW if os.name == "nt" else 0

_MP4_MOV_FORMATS = frozenset({VideoFormat.MP4, VideoFormat.MOV})
_MKV_FORMATS = frozenset({VideoFormat.MKV})
_WEBM_FORMATS = frozenset({VideoFormat.WEBM})
_AVI_FORMATS = frozenset({VideoFormat.AVI})
_FLV_FORMATS = frozenset({VideoFormat.FLV})
_NO_EMBED_FORMATS = _WEBM_FORMATS | _FLV_FORMATS
_ALL_SUPPORTED = (
    _MP4_MOV_FORMATS | _MKV_FORMATS | _WEBM_FORMATS | _AVI_FORMATS | _FLV_FORMATS
)

_MP4_COVER_FORMAT_JPEG = 13
_AV1_CODEC_ID = av.Codec("av1", "r").id


class _SaveStages:
    def __init__(self, callback: Callable[[str, float | None], None] | None) -> None:
        self._callback = callback
        self._name: str | None = None
        self._started = 0.0

    def start(self, name: str) -> None:
        self.finish()
        self._name = name
        self._started = time.monotonic()
        if self._callback is not None:
            self._callback(name, None)

    def finish(self) -> None:
        if self._name is not None and self._callback is not None:
            self._callback(self._name, time.monotonic() - self._started)
        self._name = None


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

    def write(
        self,
        video: VideoFile,
        thumbnail: Image.Image,
        *,
        position_ms: int | None = None,
        on_progress: Callable[[str, float | None], None] | None = None,
    ) -> ApplyResult:
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

        stages = _SaveStages(on_progress)
        stages.start("Preparing thumbnail")

        # Preserve the selected frame for zero-frame preview fallbacks.
        frame_buf = io.BytesIO()
        thumbnail.copy().convert("RGB").save(frame_buf, format="PNG")
        frame_bytes = frame_buf.getvalue()

        # Scale and encode thumbnail to JPEG
        cover_image = thumbnail.copy()
        cover_image.thumbnail((_MAX_THUMB_W, _MAX_THUMB_H), Image.Resampling.LANCZOS)

        jpeg_buf = io.BytesIO()
        cover_image.convert("RGB").save(jpeg_buf, format="JPEG", quality=_JPEG_QUALITY)
        jpeg_bytes = jpeg_buf.getvalue()

        # FLV / WebM: no container embedding — return success immediately
        if video.format in _NO_EMBED_FORMATS:
            stages.finish()
            elapsed_ms = int((time.monotonic() - start) * 1000)
            return ApplyResult(
                success=True,
                elapsed_ms=elapsed_ms,
                error_message=(
                    f"{video.format.name} does not support embedded cover art; "
                    "Linux file manager icon updated via XDG cache"
                ),
            )

        duration_shift_ms = 0
        try:
            if video.format in _MP4_MOV_FORMATS:
                duration_shift_ms = self._embed_mp4_mov(
                    video.path,
                    jpeg_bytes,
                    frame_bytes,
                    stages=stages,
                    position_ms=position_ms,
                    frame_number=(
                        round(position_ms / max(1, video.frame_step_ms))
                        if position_ms is not None
                        else None
                    ),
                    frame_step_ms=video.frame_step_ms,
                    write_mp4_covr=video.format is VideoFormat.MP4,
                )
            elif video.format in _MKV_FORMATS:
                self._embed_mkv_webm(video.path, jpeg_bytes, stages)
            elif video.format in _AVI_FORMATS:
                self._remux_avi(video.path, stages)
                stages.finish()
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
            stages.finish()
            return ApplyResult(
                success=False,
                error_code=ApplyError.FILE_NOT_WRITABLE,
                error_message=f"Permission denied writing '{video.path}': {exc}",
            )
        except subprocess.CalledProcessError as exc:
            stages.finish()
            stderr = exc.stderr.decode(errors="replace") if exc.stderr else "(none)"
            return ApplyResult(
                success=False,
                error_code=ApplyError.FFMPEG_ERROR,
                error_message=(
                    f"ffmpeg failed (exit {exc.returncode}). stderr: {stderr}"
                ),
            )
        except OSError as exc:
            stages.finish()
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
            stages.finish()
            return ApplyResult(
                success=False,
                error_code=ApplyError.UNEXPECTED,
                error_message=f"Unexpected error: {exc}",
            )

        stages.finish()
        elapsed_ms = int((time.monotonic() - start) * 1000)
        return ApplyResult(
            success=True, elapsed_ms=elapsed_ms, duration_shift_ms=duration_shift_ms
        )

    # ------------------------------------------------------------------
    # Format-specific embedding helpers
    # ------------------------------------------------------------------

    def reencode_reason(self, video: VideoFile) -> str | None:
        """Return the known reason MP4/MOV saving will re-encode the video."""
        if video.format not in _MP4_MOV_FORMATS:
            return None
        if Path(video.path).suffix.lower() != ".mp4":
            return "The fast copy path supports .mp4 files only."
        _, reason = self._concat_decision(video.path)
        return reason

    def _embed_mp4_mov(
        self,
        video_path: str,
        jpeg_bytes: bytes,
        frame_bytes: bytes,
        *,
        stages: _SaveStages,
        position_ms: int | None,
        frame_number: int | None,
        frame_step_ms: int,
        write_mp4_covr: bool,
    ) -> int:
        """Embed a preview and cover art into an MP4 or MOV file.

        Returns:
            Milliseconds the concat fast path shifted every original frame
            forward by prepending a new frame; 0 when the full re-encode path
            was used instead (frame positions are unchanged in that case).
        """
        ffmpeg_exe = imageio_ffmpeg.get_ffmpeg_exe()
        video_dir = os.path.dirname(os.path.abspath(video_path))
        video_ext = os.path.splitext(video_path)[1].lower() or ".mp4"
        stages.start("Inspecting video streams")
        if video_ext == ".mp4":
            concat_params, _ = self._concat_decision(video_path)
        else:
            logger.debug(
                "concat fast path skipped for '%s': extension %r is not .mp4",
                video_path,
                video_ext,
            )
            concat_params = None
        stages.start("Preparing temporary files")
        cover_fd, cover_path = tempfile.mkstemp(
            dir=video_dir, prefix=".vt_cover_", suffix=".jpg"
        )
        frame_fd, frame_path = tempfile.mkstemp(
            dir=video_dir, prefix=".vt_frame_", suffix=".png"
        )
        preview_fd, preview_path = tempfile.mkstemp(
            dir=video_dir, prefix=".vt_preview_", suffix=".mp4"
        )
        concat_fd, concat_path = tempfile.mkstemp(
            dir=video_dir, prefix=".vt_concat_", suffix=".txt"
        )
        os.close(preview_fd)
        os.close(concat_fd)
        try:
            with os.fdopen(cover_fd, "wb") as f:
                f.write(jpeg_bytes)
            with os.fdopen(frame_fd, "wb") as f:
                f.write(frame_bytes)

            shift_ms = 0

            def _ffmpeg_write_fn(tmp_video_path: Path) -> None:
                nonlocal shift_ms
                output_format = "mp4"
                if concat_params is not None and self._write_concat(
                    video_path,
                    tmp_video_path,
                    frame_path,
                    cover_path,
                    preview_path,
                    concat_path,
                    output_format,
                    concat_params,
                    stages,
                ):
                    # The concat path prepends a new frame, pushing every original
                    # frame (and the one the user selected) forward in time.
                    shift_ms = round(concat_params[5] * 1000)
                    shifted_position_ms = (
                        position_ms + shift_ms if position_ms is not None else None
                    )
                    shifted_frame_number = (
                        round(shifted_position_ms / max(1, frame_step_ms))
                        if shifted_position_ms is not None
                        else frame_number
                    )
                    if write_mp4_covr:
                        stages.start("Writing cover art")
                        self._write_mp4_covr_tag(tmp_video_path, jpeg_bytes)
                    stages.start("Writing frame metadata")
                    self._write_source_metadata(
                        tmp_video_path, shifted_frame_number, shifted_position_ms
                    )
                    return

                logger.info(
                    "Full re-encode required for '%s' (concat fast path unavailable)",
                    video_path,
                )
                stages.start("Encoding video")
                subprocess.run(
                    [
                        ffmpeg_exe, "-y",
                        "-i", video_path,
                        "-loop", "1", "-i", frame_path,
                        "-i", cover_path,
                        "-filter_complex",
                        "[1:v][0:v]scale2ref=flags=lanczos[poster][base];"
                        "[base][poster]overlay=shortest=1:enable='eq(n,0)'[v]",
                        "-map", "[v]",
                        "-map", "0:a?",
                        "-map", "0:s?",
                        "-map", "2:v",
                        "-map_metadata", "0",
                        "-map_chapters", "0",
                        "-c:v:0", "libx264",
                        "-preset:v:0", "veryfast",
                        "-crf:v:0", "23",
                        "-pix_fmt:v:0", "yuv420p",
                        "-c:v:1", "copy",
                        "-disposition:v:1", "attached_pic",
                        "-c:a", "copy",
                        "-c:s", "copy",
                        "-f", output_format,
                        str(tmp_video_path),
                    ],
                    check=True,
                    capture_output=True,
                    creationflags=_FFMPEG_CREATION_FLAGS,
                )
                if write_mp4_covr:
                    stages.start("Writing cover art")
                    self._write_mp4_covr_tag(tmp_video_path, jpeg_bytes)
                stages.start("Writing frame metadata")
                self._write_source_metadata(
                    tmp_video_path, frame_number, position_ms
                )

            atomic_replace(video_path, _ffmpeg_write_fn, copy_existing=False)
            stages.start("Cleaning up temporary files")
        finally:
            for temp_path in (cover_path, frame_path, preview_path, concat_path):
                try:
                    os.unlink(temp_path)
                except FileNotFoundError:
                    pass
        return shift_ms

    @staticmethod
    def _concat_decision(
        video_path: str,
    ) -> tuple[tuple[str, int, int, str, str, float] | None, str | None]:
        with av.open(video_path) as container:
            playable = [
                stream
                for stream in container.streams.video
                if not bool(stream.disposition & stream.disposition.attached_pic)
            ]
            if len(playable) != 1:
                logger.debug(
                    "concat fast path skipped for '%s': found %d playable video "
                    "stream(s), expected exactly 1",
                    video_path,
                    len(playable),
                )
                return (
                    None,
                    f"Expected one playable video stream; found {len(playable)}.",
                )
            if any(
                bool(stream.disposition & stream.disposition.attached_pic)
                for stream in container.streams.video
            ):
                logger.debug(
                    "concat fast path skipped for '%s': file already has an "
                    "attached-picture video stream",
                    video_path,
                )
                return None, "An attached-picture video stream is already present."

            stream = playable[0]
            codec = stream.codec_context
            profile = (codec.profile or "").lower()
            h264_profiles = {
                "baseline": "baseline",
                "constrained baseline": "baseline",
                "main": "main",
                "high": "high",
            }
            is_av1 = codec.codec.id == _AV1_CODEC_ID
            # average_rate need not equal base_rate: the concat demuxer's explicit
            # segment duration (plus a matching -itsoffset) keeps A/V in sync
            # independent of whether the source stream is CFR or VFR.
            frame_rate = stream.average_rate
            reasons: list[str] = []
            if codec.name != "h264" and not is_av1:
                reasons.append(
                    f"video codec is {codec.codec.long_name or codec.name} "
                    "(fast copy supports H.264 and AV1 only)"
                )
            if (is_av1 and profile != "main") or (
                not is_av1 and profile not in h264_profiles
            ):
                reasons.append(f"profile is {(codec.profile or '(none)')!r}")
            if codec.format is None or codec.format.name != "yuv420p":
                pix_fmt = codec.format.name if codec.format is not None else None
                reasons.append(f"pixel format is {pix_fmt!r}, not yuv420p")
            if stream.width <= 0 or stream.height <= 0:
                reasons.append(f"invalid dimensions {stream.width}x{stream.height}")
            if frame_rate is None or float(frame_rate) <= 0:
                reasons.append(f"invalid frame rate {frame_rate!r}")
            if reasons:
                logger.debug(
                    "concat fast path skipped for '%s': %s",
                    video_path,
                    "; ".join(reasons),
                )
                return None, "; ".join(reasons) + "."

            assert frame_rate is not None
            return (
                (
                    "av1" if is_av1 else "h264",
                    stream.width,
                    stream.height,
                    "main" if is_av1 else h264_profiles[profile],
                    str(frame_rate),
                    1 / float(frame_rate),
                ),
                None,
            )

    @staticmethod
    def _concat_quote(path: str) -> str:
        """Quote a path for an FFmpeg concat demuxer manifest."""
        return "'" + Path(path).as_posix().replace("'", "\\'") + "'"

    def _write_concat(
        self,
        video_path: str,
        output_path: Path,
        frame_path: str,
        cover_path: str,
        preview_path: str,
        concat_path: str,
        output_format: str,
        params: tuple[str, int, int, str, str, float],
        stages: _SaveStages,
    ) -> bool:
        """Prepend one matching frame and stream-copy the original packets."""
        ffmpeg_exe = imageio_ffmpeg.get_ffmpeg_exe()
        codec_name, width, height, profile, frame_rate, frame_duration = params
        duration_text = f"{frame_duration:.12g}"
        encoder_args = (
            ["-c:v", "libaom-av1", "-cpu-used", "8", "-row-mt", "1",
             "-threads", "4", "-profile:v", "0", "-crf", "23", "-b:v", "0",
             "-pix_fmt", "yuv420p"]
            if codec_name == "av1"
            else ["-c:v", "libx264", "-profile:v", profile, "-pix_fmt", "yuv420p"]
        )

        stages.start("Encoding preview frame")
        subprocess.run(
            [
                ffmpeg_exe, "-y",
                "-loop", "1", "-framerate", frame_rate, "-i", frame_path,
                "-frames:v", "1",
                "-vf", f"scale={width}:{height}:flags=lanczos",
                "-an", "-sn",
                *encoder_args,
                "-f", "mp4",
                preview_path,
            ],
            check=True,
            capture_output=True,
            creationflags=_FFMPEG_CREATION_FLAGS,
        )
        stages.start("Checking encoded frame")
        with av.open(preview_path) as preview:
            preview_stream = preview.streams.video[0]
            preview_codec = preview_stream.codec_context
            if (
                (preview_codec.codec.id != _AV1_CODEC_ID if codec_name == "av1"
                 else preview_codec.name != "h264")
                or (preview_codec.profile or "").lower() != profile
                or preview_codec.format is None
                or preview_codec.format.name != "yuv420p"
                or preview_stream.width != width
                or preview_stream.height != height
                or preview_stream.average_rate is None
                or abs(float(preview_stream.average_rate) - 1 / frame_duration) > 0.001
            ):
                logger.debug(
                    "concat fast path skipped for '%s': encoded preview frame "
                    "did not match source params (codec=%s, profile=%s, "
                    "pix_fmt=%s, size=%dx%d, rate=%s; expected profile=%s, "
                    "size=%dx%d, rate=%s)",
                    video_path,
                    preview_codec.name,
                    preview_codec.profile,
                    preview_codec.format.name if preview_codec.format else None,
                    preview_stream.width,
                    preview_stream.height,
                    preview_stream.average_rate,
                    profile,
                    width,
                    height,
                    1 / frame_duration,
                )
                return False

        manifest = (
            f"file {self._concat_quote(preview_path)}\n"
            f"duration {duration_text}\n"
            f"file {self._concat_quote(video_path)}\n"
        )
        Path(concat_path).write_text(manifest, encoding="utf-8")
        stages.start("Copying video streams")
        subprocess.run(
            [
                ffmpeg_exe, "-y",
                "-f", "concat", "-safe", "0", "-i", concat_path,
                "-itsoffset", duration_text, "-i", video_path,
                "-i", cover_path,
                "-map", "0:v:0",
                "-map", "1:a?",
                "-map", "1:s?",
                "-map", "2:v:0",
                "-map_metadata", "1",
                "-map_chapters", "1",
                "-c", "copy",
                "-disposition:v:1", "attached_pic",
                "-f", output_format,
                str(output_path),
            ],
            check=True,
            capture_output=True,
            creationflags=_FFMPEG_CREATION_FLAGS,
        )
        return True

    def _write_mp4_covr_tag(self, video_path: Path, jpeg_bytes: bytes) -> None:
        """Write JPEG cover art into the MP4 ``covr`` tag for Telegram clients."""
        from mutagen.mp4 import MP4, MP4Cover

        tags = MP4(str(video_path))
        tags.tags = tags.tags or {}
        tags.tags["covr"] = [MP4Cover(jpeg_bytes, imageformat=_MP4_COVER_FORMAT_JPEG)]
        tags.save()

    @staticmethod
    def _write_source_metadata(
        video_path: Path, frame_number: int | None, position_ms: int | None
    ) -> None:
        if frame_number is not None and position_ms is not None:
            write_thumbnail_source(video_path, frame_number, position_ms)

    def _embed_mkv_webm(
        self, video_path: str, jpeg_bytes: bytes, stages: _SaveStages
    ) -> None:
        """Embed cover art into an MKV or WebM file using ffmpeg -attach."""
        stages.start("Preparing temporary files")
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
                    stages.start("Attaching cover art")
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
                        creationflags=_FFMPEG_CREATION_FLAGS,
                    )
                    os.replace(out_path, tmp_video_path)
                except Exception:
                    try:
                        os.unlink(out_path)
                    except FileNotFoundError:
                        pass
                    raise

            atomic_replace(video_path, _mkv_write_fn)
            stages.start("Cleaning up temporary files")
        finally:
            try:
                os.unlink(cover_path)
            except FileNotFoundError:
                pass

    def _remux_avi(self, video_path: str, stages: _SaveStages) -> None:
        """Re-mux an AVI file in-place (no cover art; preserves all streams)."""
        stages.start("Preparing temporary files")
        ffmpeg_exe = imageio_ffmpeg.get_ffmpeg_exe()
        video_dir = os.path.dirname(os.path.abspath(video_path))

        def _avi_write_fn(tmp_video_path: Path) -> None:
            out_fd, out_path = tempfile.mkstemp(
                dir=video_dir, prefix=".vt_out_", suffix=".avi"
            )
            os.close(out_fd)
            try:
                stages.start("Remuxing video")
                subprocess.run(
                    [
                        ffmpeg_exe, "-y",
                        "-i", str(tmp_video_path),
                        "-c", "copy",
                        out_path,
                    ],
                    check=True,
                    capture_output=True,
                    creationflags=_FFMPEG_CREATION_FLAGS,
                )
                os.replace(out_path, tmp_video_path)
            except Exception:
                try:
                    os.unlink(out_path)
                except FileNotFoundError:
                    pass
                raise

        atomic_replace(video_path, _avi_write_fn)
