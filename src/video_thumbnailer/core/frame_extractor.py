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

            target_seconds = position.offset_ms / 1000
            best_frame: av.video.frame.VideoFrame | None = None
            best_distance = float("inf")
            previous_frame: av.video.frame.VideoFrame | None = None
            previous_time: float | None = None
            reached_target = False
            for packet in container.demux(stream):
                for f in packet.decode():
                    frame_time = f.time
                    if (
                        frame_time is None
                        and f.pts is not None
                        and f.time_base is not None
                    ):
                        frame_time = float(f.pts * f.time_base)
                    if frame_time is None:
                        if best_frame is None:
                            best_frame = f
                        continue

                    distance = abs(frame_time - target_seconds)
                    if distance < best_distance:
                        best_frame = f
                        best_distance = distance

                    if frame_time >= target_seconds:
                        if (
                            previous_frame is not None
                            and previous_time is not None
                        ):
                            if (
                                target_seconds - previous_time
                                <= frame_time - target_seconds
                            ):
                                best_frame = previous_frame
                        reached_target = True
                        break

                    previous_frame = f
                    previous_time = frame_time
                if reached_target:
                    break

            if best_frame is None:
                raise ExtractionError(
                    video.path, position.offset_ms, "no frame decoded after seek"
                )

            image: Image.Image = best_frame.to_image().convert("RGB")  # type: ignore[no-untyped-call]
            return image
        except (ValueError, ExtractionError):
            raise
        except Exception as exc:  # noqa: BLE001
            raise ExtractionError(video.path, position.offset_ms, str(exc)) from exc
        finally:
            if container is not None:
                container.close()
