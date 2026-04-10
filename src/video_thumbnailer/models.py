"""Domain model types for video-thumbnailer.

Defines all shared dataclasses and enums used across the core, UI, and platform layers.
"""

from __future__ import annotations

from dataclasses import dataclass, field
from enum import Enum
from typing import TYPE_CHECKING

if TYPE_CHECKING:
    from PIL import Image as PILImage

__all__ = [
    "VideoFormat",
    "VideoFile",
    "TimelinePosition",
    "ThumbnailFrame",
    "ApplyStatus",
    "ApplyError",
    "ApplyResult",
    "ApplyOperation",
]


class VideoFormat(Enum):
    """Supported video container formats."""

    MP4 = "mp4"
    MOV = "mov"
    MKV = "mkv"
    AVI = "avi"
    WEBM = "webm"
    FLV = "flv"
    UNSUPPORTED = "unsupported"


@dataclass
class VideoFile:
    """Represents a video file loaded into the application for thumbnail editing.

    Attributes:
        path: Absolute filesystem path to the video file.
        format: Detected container format.
        duration_ms: Total video duration in milliseconds (>0).
        width: Video stream pixel width (>0).
        height: Video stream pixel height (>0).
        existing_thumbnail: Current embedded cover-art image, or None if absent.
        is_writable: Whether the file is writable by the current user.
    """

    path: str
    format: VideoFormat
    duration_ms: int
    width: int
    height: int
    existing_thumbnail: PILImage.Image | None
    is_writable: bool


@dataclass
class TimelinePosition:
    """A point in time within the loaded video.

    Attributes:
        offset_ms: Millisecond offset from the start (0 <= offset_ms <= duration_ms).
    """

    offset_ms: int

    def fraction(self, duration_ms: int) -> float:
        """Return the relative position in [0.0, 1.0].

        Args:
            duration_ms: Total duration of the video in milliseconds.

        Returns:
            Ratio of offset_ms to duration_ms; 0.0 when duration_ms is 0.
        """
        if duration_ms <= 0:
            return 0.0
        return self.offset_ms / duration_ms


@dataclass
class ThumbnailFrame:
    """A single still image extracted from a VideoFile at a given TimelinePosition.

    Attributes:
        image: Decoded video frame as an RGB PIL Image.
        source_position: The timeline position from which this frame was extracted.
        width: Pixel width.
        height: Pixel height.
    """

    image: PILImage.Image
    source_position: TimelinePosition
    width: int
    height: int


class ApplyStatus(Enum):
    """Lifecycle state of an ApplyOperation."""

    PENDING = "pending"
    IN_PROGRESS = "in_progress"
    SUCCEEDED = "succeeded"
    FAILED = "failed"


class ApplyError(Enum):
    """Machine-readable failure reason for an ApplyOperation."""

    FILE_NOT_WRITABLE = "file_not_writable"
    FILE_LOCKED = "file_locked"
    UNSUPPORTED_FORMAT = "unsupported_format"
    NO_VIDEO_STREAM = "no_video_stream"
    FFMPEG_ERROR = "ffmpeg_error"
    DISK_FULL = "disk_full"
    UNEXPECTED = "unexpected"


@dataclass
class ApplyResult:
    """The outcome of a completed ApplyOperation.

    Attributes:
        success: True if thumbnail was embedded and cache refreshed.
        error_code: Machine-readable failure reason; None on success.
        error_message: Human-readable failure description; None on success.
        elapsed_ms: Total wall-clock duration of the operation in milliseconds.
    """

    success: bool
    error_code: ApplyError | None = None
    error_message: str | None = None
    elapsed_ms: int = 0


@dataclass
class ApplyOperation:
    """Represents a single invocation of the 'Apply Thumbnail' action.

    Attributes:
        video_file: The video file to be modified.
        selected_position: The timeline position of the frame to embed.
        status: Current status of the operation.
        result: Populated when status is SUCCEEDED or FAILED.
    """

    video_file: VideoFile
    selected_position: TimelinePosition
    status: ApplyStatus = field(default=ApplyStatus.PENDING)
    result: ApplyResult | None = None
