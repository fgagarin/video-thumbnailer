# Implementation Plan: Video Thumbnail Changer

**Branch**: `001-change-video-thumbnail` | **Date**: 2026-04-10 | **Spec**: [spec.md](spec.md)
**Input**: Feature specification from `/specs/001-change-video-thumbnail/spec.md`

## Summary

Allow users to change the thumbnail icon displayed by Windows Explorer, macOS Finder, and
Linux Nautilus for any video file. The user drags a video file onto the application window,
scrubs an interactive timeline to find the desired frame, previews it against the current
thumbnail side-by-side, then presses "Apply Thumbnail" to embed the selected frame as
cover art and trigger a file-manager cache refresh.

**Technical approach** (from research): Python 3.13 desktop GUI using PySide6 (Qt6),
frame extraction via PyAV (FFmpeg bindings), thumbnail embedding via PyAV for MP4/MOV
and ffmpeg subprocess for MKV/WebM/AVI, atomically safe writes via same-directory temp
file + `os.replace()`, and platform-specific XDG cache writes (Linux), qlmanage
(macOS), and SHChangeNotify (Windows) for file-manager refresh.

## Technical Context

**Language/Version**: Python 3.13
**Primary Dependencies**: PySide6 6.x (Qt6 GUI), PyAV 12+ (video decode / frame seek),
Pillow 10+ (image resize / convert), ffmpeg binary (thumbnail embedding for MKV/WebM/AVI)
**Storage**: Local filesystem only — video files modified in-place via atomic write; XDG
thumbnail cache written on Linux (`~/.cache/thumbnails/`)
**Testing**: pytest 8+, pytest-qt 4+, pytest-cov (≥80% branch), pytest-benchmark
**Target Platform**: Windows 10+, macOS 12+, Linux (X11 and Wayland via Qt6)
**Project Type**: Desktop GUI application
**Performance Goals**: ≤500 ms frame extraction and preview update; ≤5 s end-to-end apply
and file-manager refresh; ≥10 thumbnails/second batch throughput baseline
**Constraints**: ≤512 MB peak per-process memory; no admin/root privileges required; atomic
write must not corrupt files up to 2 GB on crash; 4K (3840×2160) resolution must be supported
**Scale/Scope**: Single video file per session; cross-platform; 6 supported container formats
(MP4, MOV, MKV, AVI, WebM, FLV)

## Constitution Check

*GATE: Must pass before Phase 0 research. Re-check after Phase 1 design.*

| Principle | Gate | Status | Evidence |
|-----------|------|--------|---------|
| I. Code Quality | ruff + mypy (strict) enforced in CI; files ≤300 lines | ✅ PASS | pyproject.toml configures ruff + mypy; module split enforced in project structure |
| II. Testing Standards | ≥80% branch coverage; integration tests per format; TDD workflow | ✅ PASS | pytest-cov, pytest-qt, per-format integration test files in scope |
| III. UX Consistency | Error messages human-readable with cause + fix; predictable apply/cancel UX | ✅ PASS | All FR-009/009a addressed; CLI principle is N/A (GUI-only app per spec) |
| IV. Performance | ≤500 ms frame extraction; ≤512 MB peak memory; benchmark gate in CI | ✅ PASS | PyAV seek+decode targets 50–300 ms; benchmark tests under tests/benchmarks/ |

**Post-design re-check**: All gates still pass after Phase 1 design. No violations to track.

## Project Structure

### Documentation (this feature)

```text
specs/[###-feature]/
├── plan.md              # This file (/speckit.plan command output)
├── research.md          # Phase 0 output (/speckit.plan command)
├── data-model.md        # Phase 1 output (/speckit.plan command)
├── quickstart.md        # Phase 1 output (/speckit.plan command)
├── contracts/           # Phase 1 output (/speckit.plan command)
└── tasks.md             # Phase 2 output (/speckit.tasks command - NOT created by /speckit.plan)
```

## Project Structure

### Documentation (this feature)

```text
specs/001-change-video-thumbnail/
├── plan.md              # This file
├── research.md          # Technology decisions with rationale
├── data-model.md        # Entity definitions
├── quickstart.md        # Developer onboarding
├── contracts/           # Internal module interface contracts
│   └── core_interfaces.md
└── tasks.md             # Phase 2 output (/speckit.tasks command)
```

### Source Code (repository root)

```text
src/
└── video_thumbnailer/
    ├── __init__.py
    ├── app.py                         # QApplication entry point; main() function
    ├── ui/
    │   ├── __init__.py
    │   ├── main_window.py             # MainWindow: drag-drop zone, layout orchestration
    │   ├── timeline_widget.py         # TimelineWidget: custom QWidget scrubber
    │   └── preview_widget.py          # PreviewWidget: side-by-side current vs candidate
    ├── core/
    │   ├── __init__.py
    │   ├── video_loader.py            # VideoLoader: open container, read duration + existing thumb
    │   ├── frame_extractor.py         # FrameExtractor: PyAV seek + decode → PIL Image
    │   └── thumbnail_writer.py        # ThumbnailWriter: embed cover art; atomic write
    └── platform/
        ├── __init__.py
        ├── cache_linux.py             # XDG thumbnail spec write + D-Bus Nautilus refresh
        ├── cache_macos.py             # qlmanage -r cache + touch + mdimport
        └── cache_windows.py           # SHChangeNotify via ctypes

tests/
├── conftest.py                        # Shared fixtures (sample video files per format)
├── unit/
│   ├── test_video_loader.py
│   ├── test_frame_extractor.py
│   ├── test_thumbnail_writer.py
│   └── test_cache_invalidator.py
├── integration/
│   ├── test_mp4_workflow.py           # Full apply pipeline: MP4 / MOV
│   ├── test_mkv_workflow.py           # Full apply pipeline: MKV / WebM
│   ├── test_avi_workflow.py           # Full apply pipeline: AVI / FLV
│   └── test_atomic_write.py           # Crash-safety verification
└── benchmarks/
    └── test_frame_extraction_perf.py  # ≤500 ms gate per format

pyproject.toml                         # PEP 621 project metadata + dependencies
uv.lock                                # Deterministic lock file (uv)
```

**Structure Decision**: Single-project layout. All source under `src/video_thumbnailer/`
(src-layout prevents accidental imports from project root). UI, core business logic, and
platform adapters are separated into distinct subpackages to enforce single-responsibility
and keep files under the 300-line limit imposed by the constitution.

## Complexity Tracking

No constitution violations. No complexity exceptions required.
