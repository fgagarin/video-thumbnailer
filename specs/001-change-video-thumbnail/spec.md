# Feature Specification: Video Thumbnail Changer

**Feature Branch**: `001-change-video-thumbnail`
**Created**: 2026-04-10
**Status**: Draft

## Clarifications

### Session 2026-04-10

- Q: After the user presses Apply, can they undo or revert to the previous thumbnail? → A: No undo — the confirmation dialog is the only safeguard; once applied the change is permanent.
- Q: Should the apply operation use atomic write semantics to prevent file corruption on crash? → A: Yes — write to a temp file in the same directory, then rename to replace the original atomically.
- Q: What visual feedback does the user see during long operations (video loading, frame extraction, apply)? → A: Show a spinner/progress indicator and disable interactive controls during each operation.
- Q: If the user drops a second video while one is already loaded, what happens? → A: Load the new video silently, replacing the current one (no confirmation needed — no destructive change has been committed yet).
- Q: What is the maximum supported video resolution? → A: Up to 4K (3840×2160); 8K and above deferred to a future release.

## User Scenarios & Testing *(mandatory)*

### User Story 1 - Apply Custom Frame as Video Thumbnail (Priority: P1)

A user has a video file whose auto-generated thumbnail gives a poor first impression (e.g.,
a black frame or an irrelevant scene). The user opens the application, drags the video file
onto the window, scrubs the timeline scrubber to a frame that represents the content well,
and presses "Apply Thumbnail". From that point on, the file manager (Windows Explorer, macOS
Finder, or Linux Nautilus) displays the chosen frame as the file's thumbnail icon.

**Why this priority**: This is the entire purpose of the application. All other stories
extend this core flow. A user who completes this story has the full value of the tool.

**Independent Test**: Launch the application, drag in a test MP4 file, move the timeline
scrubber to the 10-second mark, press Apply Thumbnail, then check the file manager — the
thumbnail icon MUST have changed to the selected frame. Can be fully tested without any
other story being implemented.

**Acceptance Scenarios**:

1. **Given** the application is open and no video is loaded, **When** the user drags a
   supported video file onto the window, **Then** the video loads and the timeline scrubber
   becomes interactive, showing the video's total duration.
2. **Given** a video is loaded, **When** the user moves the timeline scrubber to a position,
   **Then** the frame preview area updates to show the exact video frame at that position
   within 500 ms.
3. **Given** a frame is selected, **When** the user presses "Apply Thumbnail", **Then** a
   confirmation dialog appears if the video already contains an embedded thumbnail, asking
   whether to overwrite. There is no undo once applied — the confirmation dialog is the sole
   safeguard.
4. **Given** the user confirms (or no existing thumbnail), **When** the apply operation
   completes, **Then** a success message is shown and the file manager displays the new
   thumbnail within 5 seconds without the user restarting the file manager.
5. **Given** the apply operation fails (e.g., file is read-only, format unsupported),
   **When** the error occurs, **Then** the application displays a clear, human-readable
   error message and the original video file is left unmodified.

---

### User Story 2 - Preview Before Committing (Priority: P2)

Before applying a new thumbnail, a user wants to compare the currently embedded thumbnail
(if any) side-by-side with the frame they have selected, so they can make an informed
decision without trial-and-error.

**Why this priority**: Reduces repeated apply-undo cycles and increases confidence in the
result. Builds on P1 but adds meaningful quality-of-life value that can be independently
demonstrated.

**Independent Test**: Load a video that already has an embedded thumbnail. Move the scrubber
to a candidate frame. The UI MUST show two images side by side — the existing thumbnail and
the candidate frame — before the user presses Apply. Can be verified visually without
completing any apply operation.

**Acceptance Scenarios**:

1. **Given** a video with an existing embedded thumbnail is loaded, **When** the user scrubs
   the timeline, **Then** both the current thumbnail and the candidate frame are visible
   simultaneously in the preview area.
2. **Given** a video with no embedded thumbnail is loaded, **When** the user scrubs the
   timeline, **Then** the "current thumbnail" area shows a placeholder indicating no
   thumbnail exists.
3. **Given** the user is viewing the side-by-side preview, **When** they press Apply
   Thumbnail, **Then** the current thumbnail area updates to the newly applied frame after
   the operation completes.

---

### User Story 3 - Multiple Video Format Support (Priority: P3)

A user works with videos in a variety of container formats (not just MP4) and expects the
application to handle all common formats with the same drag-drop workflow and identical UX.

**Why this priority**: Broadens the tool's usefulness but does not change the core workflow.
The core value is already delivered by P1 with MP4/MOV support; P3 adds coverage for less
common formats.

**Independent Test**: Drag in one video of each supported format (MKV, AVI, WebM, FLV) and
apply a thumbnail for each. Each file MUST succeed and the file manager MUST display the
updated thumbnail. Verified independently from stories P1 and P2.

**Acceptance Scenarios**:

1. **Given** the user drops a file in any supported format (MP4, MOV, MKV, AVI, WebM, FLV),
   **When** the file loads, **Then** the timeline scrubber and frame preview work identically
   to the MP4 flow.
2. **Given** the user drops a file in an unsupported format (e.g., a .txt file renamed to
   .mp4, or an unrecognised container), **When** the drop occurs, **Then** the application
   displays a clear error explaining which formats are supported and does not crash.
3. **Given** the user applies a thumbnail to an MKV or AVI file, **When** the operation
   completes, **Then** the file manager on the same operating system shows the new thumbnail
   icon.

---

### Edge Cases

- What happens when the timeline scrubber is positioned at the very first or last frame?
  Expected: frame extraction succeeds; frames at boundaries are valid selections.
- How does the system handle a video file that is currently open in another application
  (file locked)? Expected: error message; original file untouched.
- What happens when the video has no video stream (audio-only file with a video extension)?
  Expected: clear error; file not modified.
- How does the system handle very short videos (< 1 second)?
  Expected: scrubber still works; any single frame can be selected and applied.
- What happens when the user drops multiple files at once?
  Expected: only the first file is loaded; a user-friendly message explains that one-at-a-time
  operation is required in v1.
- What happens when the user drops a new video while one is already loaded?
  Expected: the new video loads silently, replacing the current one. No confirmation is needed
  because no destructive change has been committed (the apply operation is the only
  irreversible action).
- What if the video file is on a read-only file system or network share?
  Expected: error dialog with actionable advice (copy file locally first).

## Requirements *(mandatory)*

### Functional Requirements

- **FR-001**: System MUST accept supported video files via drag-and-drop onto the application
  window. Dropping a new video while one is already loaded MUST silently replace the current
  video without a confirmation dialog.
- **FR-001a**: System MUST support video files up to 4K resolution (3840×2160); videos at
  higher resolutions are not required to be supported in v1.
- **FR-002**: System MUST display an interactive timeline scrubber showing the total video
  duration after a video is loaded.
- **FR-003**: Users MUST be able to position the timeline scrubber at any point in the video
  and see the corresponding video frame displayed in a preview area within 500 ms.
- **FR-004**: Users MUST be able to apply the currently previewed frame as the video's
  embedded thumbnail by pressing a clearly labelled "Apply Thumbnail" button.
- **FR-005**: System MUST embed the selected frame into the video file's metadata using the
  native mechanism for the file's container format (e.g., cover-art atom for MP4/MOV,
  attachment track for MKV).
- **FR-006**: System MUST display the existing embedded thumbnail alongside the candidate
  frame before the user applies a change, if an existing thumbnail is present.
- **FR-007**: System MUST prompt the user for confirmation before overwriting an existing
  embedded thumbnail.
- **FR-008**: System MUST support the following video container formats: MP4, MOV, MKV, AVI,
  WebM, FLV.
- **FR-009**: System MUST display a clear success or error message after each apply
  operation, including the reason for failure when an error occurs.
- **FR-009a**: System MUST display a spinner and disable interactive controls during all
  long-running operations: initial video loading, timeline scrub frame extraction (when
  extraction exceeds 200 ms), and the apply operation. Controls MUST re-enable as soon as
  the operation completes or fails.
- **FR-010**: System MUST leave the original video file completely unmodified if any part of
  the apply operation fails. The apply operation MUST use an atomic write strategy: write the
  updated file to a temporary file in the same directory, then rename it to replace the
  original. If the process is interrupted at any point before the rename completes, the
  original file MUST remain intact.
- **FR-011**: System MUST trigger a file-manager thumbnail cache refresh on the local
  machine after a successful apply, using the platform's standard invalidation mechanism.
- **FR-012**: System MUST operate without requiring administrator or root privileges on any
  supported operating system.

### Key Entities

- **Video File**: Represents a video file on disk. Key attributes: file path, container
  format, total duration, presence and data of an existing embedded thumbnail, write
  permission status.
- **Timeline Position**: A point in time within a loaded video, expressed as a duration
  offset from the start. Used to identify which frame to extract and preview.
- **Thumbnail Frame**: A single still image extracted from the video at a selected timeline
  position. Key attributes: image data, pixel dimensions, source timeline position.
- **Apply Operation**: The act of embedding a Thumbnail Frame into a Video File's metadata
  and updating the file-manager cache. Has a result: success or failure with reason.

## Success Criteria *(mandatory)*

### Measurable Outcomes

- **SC-001**: A first-time user can successfully change a video thumbnail within 60 seconds
  of launching the application, with no prior instructions.
- **SC-002**: The updated thumbnail is visible in the file manager on the same machine within
  5 seconds of a successful apply operation, without restarting the file manager.
- **SC-003**: Frame extraction and preview update happens within 500 ms of the user stopping
  the timeline scrubber.
- **SC-004**: The application correctly completes the thumbnail-change workflow for all 6
  supported formats without errors on standard test files for each format, including at least
  one 4K (3840×2160) test file.
- **SC-005**: When an error occurs (read-only file, unsupported format, locked file), 100% of
  error messages include the specific cause and a suggested corrective action.
- **SC-006**: The applied thumbnail is visible in Windows Explorer, macOS Finder, and Linux
  Nautilus without the user taking any additional steps beyond using the application.

## Assumptions

- The application modifies the source video file in place by updating its embedded metadata;
  a confirmation dialog is shown to the user before overwriting an existing thumbnail.
- The application is a desktop GUI application; no command-line interface is in scope for v1.
- Cross-platform support for Windows, macOS, and Linux is required from the first release,
  as explicitly stated in the requirements.
- The thumbnail to be embedded is a JPEG image scaled to a reasonable preview size
  (maximum 640 × 360 pixels) to keep file size impact minimal; exact size is an
  implementation detail.
- File-manager cache invalidation is attempted on a best-effort basis using each platform's
  standard mechanism (e.g., `touch` + inotify on Linux, Explorer thumbnail cache flush on
  Windows, Finder `qlmanage -r cache` on macOS); if invalidation fails, the thumbnail will
  still be visible after the user navigates away and back.
- Video files up to 2 GB are in scope; files larger than 2 GB are deferred to a future
  release.
- The application does not create backup copies of the original file; users are responsible
  for maintaining their own backups before using the tool. There is no in-session or
  persistent undo/revert capability; the confirmation dialog before overwriting an existing
  thumbnail is the only safeguard.
- The apply operation uses an atomic write (write temp → rename) to prevent file corruption
  on process crash or power loss during write.
- Dropping a new video while one is already loaded silently replaces the current video.
- The application displays a spinner and disables controls during video loading, frame
  extraction, and the apply operation.
- Maximum supported video resolution is 4K (3840×2160); 8K and above are deferred.
- One video can be loaded at a time in v1; batch processing of multiple files is out of
  scope.
