"""Video file loader using PyAV (libavformat / libavcodec)."""

from __future__ import annotations

import os
from typing import TYPE_CHECKING

import av
from PIL import Image

from video_thumbnailer.exceptions import NoVideoStreamError, UnsupportedFormatError
from video_thumbnailer.models import VideoFile, VideoFormat

if TYPE_CHECKING:
    pass

__all__ = ["PyAVVideoLoader"]

_FORMAT_MAP: dict[str, VideoFormat] = {
    "mp4": VideoFormat.MP4,
    "mov": VideoFormat.MOV,
    "quicktime": VideoFormat.MOV,
    "matroska": VideoFormat.MKV,
    "webm": VideoFormat.WEBM,
    "avi": VideoFormat.AVI,
    "flv": VideoFormat.FLV,
}


def _detect_format(container_format_name: str, path: str) -> VideoFormat:
    """Map a libavformat format name to a VideoFormat enum member.

    For the combined "matroska,webm" format name, distinguishes MKV from WebM
    by file extension.

    Args:
        container_format_name: The ``container.format.name`` string from PyAV.
        path: Filesystem path to the video file (used for extension fallback).

    Returns:
        The corresponding VideoFormat, or VideoFormat.UNSUPPORTED.
    """
    names = [n.strip() for n in container_format_name.lower().split(",")]

    # Combined matroska+webm: distinguish by extension
    if "matroska" in names and "webm" in names:
        ext = os.path.splitext(path)[1].lower().lstrip(".")
        return VideoFormat.WEBM if ext == "webm" else VideoFormat.MKV

    # For the combined mov/mp4 container, use file extension to distinguish
    if "mov" in names and "mp4" in names:
        ext = os.path.splitext(path)[1].lower().lstrip(".")
        if ext == "mp4":
            return VideoFormat.MP4
        if ext in ("mov", "m4v"):
            return VideoFormat.MOV
        # Unknown extension: default to MP4 (most common container for this format)
        return VideoFormat.MP4

    for name in names:
        if name in _FORMAT_MAP:
            return _FORMAT_MAP[name]
    return VideoFormat.UNSUPPORTED


def _extract_existing_thumbnail(
    container: av.container.InputContainer,
) -> Image.Image | None:
    """Extract the embedded cover-art image from the container, if present.

    Handles MP4/MOV ``attached_pic`` streams and MKV/WebM attachment streams.

    Args:
        container: An open PyAV InputContainer.

    Returns:
        A PIL Image (RGB) or None if no cover art is found.
    """
    for stream in container.streams:
        # MP4 / MOV: attached picture flag on a video stream
        attached = bool(stream.disposition & stream.disposition.attached_pic)
        if stream.type == "video" and attached:
            try:
                for packet in container.demux(stream):
                    if packet.size == 0:
                        continue
                    img = Image.open(__import__("io").BytesIO(bytes(packet)))
                    return img.convert("RGB")
            except Exception:  # noqa: BLE001
                continue

        # MKV / WebM: attachment streams
        if stream.type in ("attachment", "data"):
            fname = stream.metadata.get("filename", "").lower()
            if fname in ("cover.jpg", "cover.png", "folder.jpg", "cover.jpeg"):
                try:
                    for packet in container.demux(stream):
                        if packet.size == 0:
                            continue
                        img = Image.open(__import__("io").BytesIO(bytes(packet)))
                        return img.convert("RGB")
                except Exception:  # noqa: BLE001
                    continue

    return None


class PyAVVideoLoader:
    """Load a video file and return a VideoFile domain entity.

    Implements the VideoLoader protocol from contracts/core_interfaces.md.
    """

    def load(self, path: str) -> VideoFile:
        """Open and inspect the video file at ``path``.

        Args:
            path: Absolute or relative path to the video file.

        Returns:
            A populated VideoFile entity.

        Raises:
            FileNotFoundError: if ``path`` does not exist.
            PermissionError: if the file cannot be read.
            UnsupportedFormatError: if the container format is not recognised.
            NoVideoStreamError: if the file contains no video stream.
        """
        abs_path = os.path.abspath(path)
        if not os.path.exists(abs_path):
            raise FileNotFoundError(f"Video file not found: '{abs_path}'")

        container = av.open(abs_path)
        try:
            fmt = _detect_format(container.format.name, abs_path)
            if fmt is VideoFormat.UNSUPPORTED:
                raise UnsupportedFormatError(abs_path)

            video_streams = list(container.streams.video)
            # Filter out attached-picture streams, which are not playable video
            playable = [
                s for s in video_streams
                if not bool(s.disposition & s.disposition.attached_pic)
            ]
            if not playable:
                raise NoVideoStreamError(abs_path)

            stream = playable[0]
            duration_ms = 0
            if stream.duration is not None and stream.time_base is not None:
                duration_ms = int(
                    float(stream.duration) * float(stream.time_base) * 1000
                )
            elif container.duration is not None:
                duration_ms = int(container.duration / 1000)

            width = stream.width or 0
            height = stream.height or 0

            existing_thumbnail = _extract_existing_thumbnail(container)
        finally:
            container.close()

        is_writable = os.access(abs_path, os.W_OK)

        return VideoFile(
            path=abs_path,
            format=fmt,
            duration_ms=duration_ms,
            width=width,
            height=height,
            existing_thumbnail=existing_thumbnail,
            is_writable=is_writable,
        )
