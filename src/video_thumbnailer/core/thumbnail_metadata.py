"""Private container metadata for restoring the selected thumbnail source."""

from __future__ import annotations

from pathlib import Path

from mutagen.mp4 import MP4, MP4FreeForm

FRAME_NUMBER_TAG = "----:com.apple.iTunes:video-thumbnailer-frame-number"
POSITION_MS_TAG = "----:com.apple.iTunes:video-thumbnailer-position-ms"


def read_thumbnail_source(path: str) -> tuple[int | None, int | None]:
    """Read the saved frame number and timeline position from an MP4 container."""
    try:
        tags = MP4(path).tags or {}
    except Exception:  # noqa: BLE001
        return None, None
    return _read_integer(tags.get(FRAME_NUMBER_TAG)), _read_integer(
        tags.get(POSITION_MS_TAG)
    )


def write_thumbnail_source(
    path: Path, frame_number: int, position_ms: int
) -> None:
    """Save the selected frame source as MP4 freeform metadata."""
    tags = MP4(str(path))
    tags.tags = tags.tags or {}
    tags.tags[FRAME_NUMBER_TAG] = [MP4FreeForm(str(frame_number).encode("ascii"))]
    tags.tags[POSITION_MS_TAG] = [MP4FreeForm(str(position_ms).encode("ascii"))]
    tags.save()


def _read_integer(values: list[bytes] | None) -> int | None:
    if not values:
        return None
    try:
        value = int(bytes(values[0]).decode("ascii"))
    except (TypeError, ValueError, UnicodeDecodeError):
        return None
    return value if value >= 0 else None