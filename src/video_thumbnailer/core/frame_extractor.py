"""Frame extractor using PyAV (libavformat / libavcodec)."""

from __future__ import annotations

import av
from PIL import Image

from video_thumbnailer.exceptions import ExtractionError
from video_thumbnailer.models import TimelinePosition, VideoFile

__all__ = ["PyAVFrameExtractor"]


class PyAVFrameExtractor:
    """Extract a single video frame at a specified timestamp using PyAV.

    Implements the FrameExtractor protocol from contracts/core_interfaces.md.
    """

    def extract(self, video: VideoFile, position: TimelinePosition) -> Image.Image:
        """Decode and return the video frame nearest to ``position.offset_ms``.

        Args:
            video: The loaded video file to extract a frame from.
            position: The timeline position specifying the desired timestamp.

        Returns:
            A PIL Image in RGB mode.

        Raises:
            ValueError: if ``position.offset_ms`` is outside [0, video.duration_ms].
            ExtractionError: if the frame cannot be decoded.
        """
        if not (0 <= position.offset_ms <= video.duration_ms):
            raise ValueError(
                f"position.offset_ms={position.offset_ms} is outside "
                f"[0, {video.duration_ms}] for '{video.path}'"
            )

        container: av.container.InputContainer | None = None
        try:
            container = av.open(video.path)
            # Find the first non-attached-picture video stream
            playable = [
                s for s in container.streams.video
                if not bool(s.disposition & s.disposition.attached_pic)
            ]
            if not playable:
                raise ExtractionError(
                    video.path, position.offset_ms, "no playable video stream"
                )

            stream = playable[0]
            # Seek to the target timestamp.  offset is in AV_TIME_BASE units
            # (microseconds) when no stream is given, which is what we want.
            target_us = int(position.offset_ms * 1000)
            container.seek(target_us)

            frame: av.video.frame.VideoFrame | None = None
            for packet in container.demux(stream):
                for f in packet.decode():
                    frame = f
                    break
                if frame is not None:
                    break

            if frame is None:
                raise ExtractionError(
                    video.path, position.offset_ms, "no frame decoded after seek"
                )

            image: Image.Image = frame.to_image().convert("RGB")  # type: ignore[no-untyped-call]
            return image
        except (ValueError, ExtractionError):
            raise
        except Exception as exc:  # noqa: BLE001
            raise ExtractionError(video.path, position.offset_ms, str(exc)) from exc
        finally:
            if container is not None:
                container.close()
