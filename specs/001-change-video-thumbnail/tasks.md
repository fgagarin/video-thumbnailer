---
description: "Task list for feature 001-change-video-thumbnail"
---

# Tasks: Video Thumbnail Changer

**Input**: Design documents from `specs/001-change-video-thumbnail/`
**Prerequisites**: plan.md ✅, spec.md ✅, research.md ✅, data-model.md ✅, contracts/ ✅, quickstart.md ✅

**Tests**: Not TDD-mandated in spec. Unit and integration tests are included per the
constitution (≥80% branch coverage required).

**Organization**: Tasks grouped by user story for independent implementation and testing.

## Format: `[ID] [P?] [Story?] Description`

- **[P]**: Can run in parallel (different files, no blocking dependencies on incomplete tasks)
- **[Story]**: User story this task belongs to ([US1], [US2], [US3])
- Exact file paths included in all task descriptions

---

## Phase 1: Setup (Shared Infrastructure)

**Purpose**: Project scaffolding, dependency manifest, CI, and test fixture generation.
No application logic. All Phase 2+ work depends on this phase completing first.

- [X] T001 Create full project directory tree: `src/video_thumbnailer/`, `src/video_thumbnailer/ui/`, `src/video_thumbnailer/core/`, `src/video_thumbnailer/platform/`, `tests/unit/`, `tests/integration/`, `tests/benchmarks/`; add empty `__init__.py` in each Python package directory; add `.gitkeep` in test leaf directories
- [X] T002 Create `pyproject.toml` at repo root — `[project]` requires-python=">=3.13", dependencies: PySide6>=6.8, av>=12.0, Pillow>=10.3; optional-dependencies.dev: pytest>=8, pytest-qt>=4.4, pytest-cov>=5, pytest-benchmark>=4, ruff>=0.4, mypy>=1.10, pyinstaller>=6.0; `[tool.ruff]` select=["E","F","I"], line-length=88; `[tool.mypy]` strict=true, python_version="3.13"; `[tool.pytest.ini_options]` testpaths=["tests"], addopts="--tb=short"; `[tool.coverage.run]` branch=true, source=["video_thumbnailer"]
- [X] T003 [P] Create `.github/workflows/ci.yml` — four jobs: **lint** (`ruff check src/ tests/`, fails on any output); **typecheck** (`mypy src/`); **test** (`pytest --cov=video_thumbnailer --cov-report=term-missing --cov-fail-under=80`); **benchmark** (`pytest tests/benchmarks/ --benchmark-only --benchmark-max-time=1`); all jobs run on ubuntu-latest, macos-latest, windows-latest; Python 3.13; install deps via `pip install -e ".[dev]"`
- [X] T004 [P] Create `tests/generate_fixtures.py` — Python script: use `subprocess.run(["ffmpeg", ...])` to generate six 10-second 320×240 H.264 test videos in `tests/fixtures/`: `sample.mp4`, `sample.mov`, `sample.mkv`, `sample.avi`, `sample.webm`, `sample.flv`; also generate `sample_with_thumb.mp4` (MP4 with an embedded JPEG cover frame) and `sample_with_thumb.mkv` (MKV with attachment cover); script is idempotent (skip if files exist); document `python tests/generate_fixtures.py` in a module-level docstring

**Checkpoint**: Repo structure exists; `uv venv && uv pip sync pyproject.toml --extra dev` installs all deps; `ruff check src/` and `mypy src/` run without file-not-found errors

---

## Phase 2: Foundational (Blocking Prerequisites)

**Purpose**: Core domain entities, exceptions, and platform factory that ALL user stories
depend on. No user story work begins until this phase is complete.

**⚠️ CRITICAL**: Models and exceptions must be stable before any core or UI module is written.

- [X] T005 Create `src/video_thumbnailer/models.py`
- [X] T006 [P] Create `src/video_thumbnailer/exceptions.py`
- [X] T007 Create `src/video_thumbnailer/platform/__init__.py`
- [X] T008 [P] Create `tests/conftest.py`
- [X] T009 Create `src/video_thumbnailer/__init__.py`

**Checkpoint**: `python -m video_thumbnailer` opens a blank Qt window; `mypy src/` and `ruff check src/` report zero errors; `pytest tests/unit/` (no unit tests yet) exits 0

---

## Phase 3: User Story 1 — Apply Custom Frame as Video Thumbnail (Priority: P1) 🎯 MVP

**Goal**: Full end-to-end flow: drag MP4/MOV video → scrub timeline → see frame preview →
press Apply → thumbnail embedded → file manager refreshed.

**Independent Test**: Launch app, drag `tests/fixtures/sample.mp4`, move scrubber to
10-second mark, press Apply Thumbnail, verify cover art embedded with `av.open()`.

### Implementation for User Story 1

- [X] T010 [P] [US1] Create `src/video_thumbnailer/core/atomic_write.py`
- [X] T011 [P] [US1] Create `src/video_thumbnailer/core/video_loader.py`
- [X] T012 [P] [US1] Create `src/video_thumbnailer/core/frame_extractor.py`
- [X] T013 [US1] Create `src/video_thumbnailer/core/thumbnail_writer.py`
- [X] T014 [P] [US1] Create `src/video_thumbnailer/platform/cache_linux.py`
- [X] T015 [P] [US1] Create `src/video_thumbnailer/platform/cache_macos.py`
- [X] T016 [P] [US1] Create `src/video_thumbnailer/platform/cache_windows.py`
- [X] T017 [P] [US1] Create `src/video_thumbnailer/ui/timeline_widget.py`
- [X] T018 [US1] Create `src/video_thumbnailer/ui/worker.py`
- [X] T019 [US1] Create `src/video_thumbnailer/ui/main_window.py`
- [X] T020 [US1] Update `src/video_thumbnailer/app.py` to wire real dependencies

### Tests for User Story 1

- [X] T021 [P] [US1] Create `tests/unit/test_video_loader.py` — use `conftest.py` fixtures; test: `load()` on `sample.mp4` returns VideoFile with format=MP4, duration_ms>0, width=320, height=240; `load()` on `sample_with_thumb.mp4` returns existing_thumbnail not None; `load()` on `sample.mp4` (no cover art) returns existing_thumbnail=None; `load()` on a `.txt` file raises UnsupportedFormatError; `load()` on a non-existent path raises FileNotFoundError; `is_writable=False` for `read_only_video`; test MOV format detection
- [X] T022 [P] [US1] Create `tests/unit/test_frame_extractor.py` — test: extract at offset_ms=0 returns RGB PIL Image; extract at mid-duration returns image; extract at duration_ms (boundary) returns image without raising; extract with offset_ms=-1 raises ValueError; extract with offset_ms > duration_ms raises ValueError; returned image mode == "RGB"; image dimensions match video stream dimensions
- [X] T023 [P] [US1] Create `tests/unit/test_thumbnail_writer.py` — test `FormatDispatchThumbnailWriter.write()`: success on writable MP4 returns `ApplyResult(success=True)`; returns `ApplyResult(success=False, error_code=FILE_NOT_WRITABLE)` for read-only file (use `read_only_video` fixture); original file bytes unchanged on write failure (compare before/after using `open(..., "rb").read()`); returns `ApplyResult(success=False, error_code=UNSUPPORTED_FORMAT)` for FLV VideoFile (format mismatch in this phase); thumbnail is scaled to ≤640×360 (verify by re-opening output with `av.open()`)
- [X] T024 [P] [US1] Create `tests/unit/test_atomic_write.py` — test: `atomic_replace` success: `write_fn` writes "hello" to tmp_path, verify target_path contains "hello"; `write_fn` raises in the middle, target_path byte-content is identical to before the call; temp file does not exist after failure (cleanup verified); temp file is in same directory as target_path
- [X] T025 [P] [US1] Create `tests/unit/test_cache_invalidator.py` — test `LinuxCacheInvalidator` (run unconditionally): correct XDG thumb path computed from md5 of `file://` URI; PNG written to `~/.cache/thumbnails/large/<hash>.png` with `Thumb::URI` and `Thumb::MTime` PNG metadata chunks; dbus-send failure (mocked subprocess raising CalledProcessError) logs warning but does not raise; test `MacOSCacheInvalidator` and `WindowsCacheInvalidator` do not raise even when subprocess tools are unavailable (mock subprocess to raise FileNotFoundError)
- [X] T026 [US1] Create `tests/integration/test_mp4_workflow.py` — full pipeline test: copy `sample.mp4` to `tmp_path`; instantiate loader, extractor, writer, linux invalidator; `video = loader.load(path)`; `frame = extractor.extract(video, TimelinePosition(offset_ms=5000))`; `result = writer.write(video, frame.to_image() if hasattr(frame, "to_image") else frame)`; assert `result.success == True`; re-open modified file with `av.open(path)`; iterate streams looking for `stream.type == "video" and stream.disposition.attached_pic`; assert attached_pic stream found; same test for `.mov` copy; assert `elapsed_ms < 3000`
- [X] T027 [US1] Create `tests/integration/test_atomic_robustness.py` — simulate interrupted write: monkeypatch `os.replace` to raise `OSError("simulated crash")`; call `writer.write(video, thumbnail)`; assert `result.success == False`; assert `pathlib.Path(video.path).read_bytes() == original_bytes` (file content unchanged); assert no `.vt_*.tmp` files exist in video's directory after the call

**Checkpoint**: `pytest tests/unit/ tests/integration/test_mp4_workflow.py tests/integration/test_atomic_robustness.py` all pass; `python -m video_thumbnailer` launches, accepts MP4 drag-drop, shows timeline scrubber and frame preview, applies thumbnail, file manager shows updated icon

---

## Phase 4: User Story 2 — Preview Before Committing (Priority: P2)

**Goal**: Extend the UI with a side-by-side preview showing the current embedded thumbnail
(or a placeholder) alongside the candidate frame selected on the timeline.

**Independent Test**: Load `sample_with_thumb.mp4` — both panels must be populated before
pressing Apply. Load `sample.mp4` (no thumb) — left panel shows "No current thumbnail"
placeholder text.

### Implementation for User Story 2

- [X] T028 [P] [US2] Create `src/video_thumbnailer/ui/preview_widget.py` — implement `PreviewWidget(QWidget)`: layout is `QHBoxLayout` with two `QFrame` panels side by side; left panel labelled "Current Thumbnail" contains a `QLabel` (`current_label`); right panel labelled "Selected Frame" contains a `QLabel` (`candidate_label`); `set_current_thumbnail(image: PIL.Image.Image | None)`: if None show italic grey "No current thumbnail" text; if image, convert PIL→QPixmap via `ImageQt.toqpixmap(image)` and `pixmap.scaled(240, 135, Qt.KeepAspectRatio, Qt.SmoothTransformation)`, set on `current_label`; `set_candidate_frame(image: PIL.Image.Image)`: convert and display on `candidate_label`; `clear()`: reset both panels to placeholder state; `sizeHint()` returns `QSize(540, 165)`; add `__all__ = ["PreviewWidget"]`
- [X] T029 [US2] Update `src/video_thumbnailer/ui/main_window.py` to integrate `PreviewWidget`: replace the single candidate-frame `QLabel` with a `PreviewWidget` instance; after `VideoLoadWorker` succeeds: call `self._preview.set_current_thumbnail(video.existing_thumbnail)`; after `FrameExtractWorker` succeeds: call `self._preview.set_candidate_frame(pil_image)`; after `ApplyWorker` succeeds: update `video.existing_thumbnail` with the applied frame and call `self._preview.set_current_thumbnail(applied_frame)`; ensure PreviewWidget is visible when a video is loaded and hidden (or placeholder-only) in the no-video state

### Tests for User Story 2

- [X] T030 [P] [US2] Create `tests/unit/test_preview_widget.py` — use `pytest-qt` `qtbot` fixture; test: both panels visible after construction; `set_current_thumbnail(None)` shows placeholder text in left panel; `set_current_thumbnail(pil_image)` shows pixmap in left label (pixmap not null); `set_candidate_frame(pil_image)` shows pixmap in right label; `clear()` resets both panels; widget `sizeHint()` equals QSize(540, 165)
- [X] T031 [US2] Create `tests/integration/test_preview_workflow.py` — instantiate `PreviewWidget` with `qtbot`; load `sample_with_thumb.mp4` via `PyAVVideoLoader`; call `preview.set_current_thumbnail(video.existing_thumbnail)`; assert left label has a non-null pixmap; scrub to 3 s via `PyAVFrameExtractor`; call `preview.set_candidate_frame(frame_image)`; assert right label has a non-null pixmap; both panels populated simultaneously (no clearing between calls); repeat with `sample.mp4` (no thumb) and assert left label shows placeholder text

**Checkpoint**: Both panels render in the running application; video with embedded thumb shows it on the left immediately after drop; placeholder text shown for videos without thumb

---

## Phase 5: User Story 3 — Multiple Video Format Support (Priority: P3)

**Goal**: MKV, WebM, AVI, and FLV formats handled with the same drag-drop workflow.
Thumbnail embedding uses ffmpeg subprocess for MKV/WebM/AVI; FLV uses XDG cache only.

**Independent Test**: Drag in `sample.mkv` → apply thumbnail → `av.open(out.mkv)` shows
attachment stream. Drag `sample.avi` → apply → file not corrupted. Drag `sample.flv` →
apply → ApplyResult notes XDG cache used.

### Implementation for User Story 3

- [X] T032 [P] [US3] Update `src/video_thumbnailer/core/video_loader.py` — extend `VideoFormat` mapping to handle additional `container.format.name` values: "matroska,webm" → MKV or WEBM (distinguish by file extension); "avi" → AVI; "flv" → FLV; extend `existing_thumbnail` extraction: for MKV/WebM, iterate `container.streams` for `stream.type == "data"` or streams with `stream.metadata.get("filename", "").lower() in ("cover.jpg", "cover.png", "folder.jpg")`; read attachment bytes with `av.open` stream packet demux and decode to PIL Image; return None gracefully if no attachment found; verify all 6 VideoFormat members are reachable from container format strings
- [X] T033 [US3] Update `src/video_thumbnailer/core/thumbnail_writer.py` — extend `FormatDispatchThumbnailWriter.write()` with additional format branches: **MKV/WebM**: save thumbnail to temp JPEG file; call `ffmpeg -y -i {tmp_video} -attach {cover_jpg} -metadata:s:t mimetype=image/jpeg -c copy {tmp_out}` via subprocess; rename tmp_out to original using `os.replace`; wrap entire operation in `atomic_replace`; **AVI**: call `ffmpeg -y -i {tmp_video} -c:v copy -c:a copy {tmp_out}`(best-effort, no cover art); include note in success ApplyResult.error_message "AVI does not support embedded cover art; file manager will generate its own preview"; **FLV**: skip ffmpeg; return `ApplyResult(success=True, error_message="FLV does not support embedded cover art; Linux file manager icon updated via XDG cache")`; all branches still call `invalidator.invalidate()` (called by ApplyWorker, not within writer); all branches handle subprocess CalledProcessError → FFMPEG_ERROR
- [X] T034 [P] [US3] Create `tests/unit/test_format_detection.py` — test `PyAVVideoLoader.load()` on each of the six fixture files: assert correct VideoFormat member for each; test UNSUPPORTED returned (not raised) for a file with unrecognised container name; test format detection is based on container format name, not file extension alone (rename sample.mp4 to sample.xyz, verify it still detects as MP4 by reading container header)
- [X] T035 [US3] Create `tests/integration/test_mkv_workflow.py` — copy `sample.mkv` and `sample.webm` to tmp_path; run full apply pipeline for each; for MKV: re-open output with `av.open()`, find stream where `stream.metadata.get("filename") in ("cover.jpg", "cover.png")`; assert attachment found; check file is playable (duration_ms unchanged); for WebM: same assertion; `elapsed_ms < 5000` for both
- [X] T036 [US3] Create `tests/integration/test_avi_workflow.py` — copy `sample.avi` and `sample.flv` to tmp_path; run full apply pipeline for AVI: assert `result.success == True`, output file has same duration_ms as input (not corrupted), ApplyResult.error_message mentions "AVI does not support embedded cover art"; run pipeline for FLV: assert `result.success == True`, ApplyResult.error_message mentions "FLV does not support embedded cover art"; both: verify original byte content unchanged if write fails (inject error to verify atomicity)

**Checkpoint**: `pytest tests/integration/test_mkv_workflow.py tests/integration/test_avi_workflow.py tests/unit/test_format_detection.py` all pass; all 6 formats drag-drop and apply successfully in the running application

---

## Phase 6: Polish & Cross-Cutting Concerns

**Purpose**: Performance gate, distribution packaging, final quality validation.

- [X] T037 [P] Create `tests/benchmarks/test_frame_extraction_perf.py` — use `pytest-benchmark`; benchmark `PyAVFrameExtractor.extract()` on `sample.mp4` (320×240 SD) at 5 s, 1 VP: assert `benchmark.stats["max"] < 0.5`; benchmark on a 1080p MP4 (generate via ffmpeg `-vf scale=1920:1080`): assert max < 0.5; benchmark on a 4K MP4 (`-vf scale=3840:2160`): assert max < 0.5; benchmark label = "frame_extract_SD/1080p/4K"; document reference hardware in module docstring
- [X] T038 [P] Create `pyinstaller.spec` at repo root — `Analysis()` with `pathex=["."]`, `hiddenimports=["av", "PIL", "PySide6"]`, `datas` collecting PySide6 Qt plugins and av dylibs via `collect_data_files("av")` and `collect_data_files("PySide6")`; `EXE(console=False, name="video-thumbnailer")`; add `Makefile` targets: `make dist-linux`, `make dist-macos`, `make dist-windows` each running `pyinstaller pyinstaller.spec`; document in quickstart.md under "Distribution"
- [X] T039 Run `pytest --cov=video_thumbnailer --cov-report=term-missing --cov-fail-under=80 tests/unit/ tests/integration/`; identify any uncovered branches; add targeted tests to reach ≥80% branch coverage; pay particular attention to exception handling branches in `video_loader.py`, `thumbnail_writer.py`, and all three cache invalidator files
- [X] T040 Run `ruff check src/ tests/` and `mypy --strict src/`; fix all reported errors; common issues to expect: missing return type annotations, `PIL.Image` type imports requiring `from __future__ import annotations`, ctypes `windll` access guarded with `sys.platform` check; ensure zero errors before marking done

**Final Checkpoint**: `pytest --cov-fail-under=80` passes; `ruff check` and `mypy --strict` report zero errors; CI workflow green on all three OS; `python -m video_thumbnailer` launches and completes the full thumbnail-change workflow for all 6 formats

---

## Dependencies

```
Phase 1 (T001–T004)
    └──> Phase 2 (T005–T009)   [needs pyproject.toml + package dirs]
              └──> Phase 3 / US1 (T010–T027)   [needs models.py, exceptions.py, platform factory]
                        ├──> Phase 4 / US2 (T028–T031)   [extends MainWindow from T019]
                        └──> Phase 5 / US3 (T032–T036)   [extends VideoLoader + ThumbnailWriter from T011, T013]
                                    ↑
                        Phase 4 and Phase 5 can run in PARALLEL after Phase 3 completes
                        (US2 = pure UI; US3 = pure core + tests; no shared files modified)

Phase 4 + Phase 5
    └──> Phase 6 (T037–T040)   [benchmarks + packaging + quality gates]
```

## Parallel Execution Examples

**Within Phase 3 (US1) — once T009 is complete**:
```
Parallel batch A:  T010, T011, T012, T014, T015, T016, T017
Parallel batch B:  T018 (after T017), T013 (after T010)
Sequential:        T019 (after T018), T020 (after T019)
Parallel batch C:  T021, T022, T023, T024, T025  (all unit tests can run simultaneously)
Sequential tests:  T026 (after T013), T027 (after T013)
```

**Phase 4 and Phase 5 in parallel (after Phase 3)**:
```
Stream A (US2):  T028, T029 → T030, T031
Stream B (US3):  T032 → T033 → T034, T035, T036
```

## Implementation Strategy

**MVP**: Complete Phase 1 + Phase 2 + Phase 3 (T001–T027). This delivers a fully
working application for MP4 and MOV files — the most common formats. Users can drag,
scrub, preview, and apply thumbnails. All CI gates (lint, type check, coverage, benchmarks)
are functional. Total: 27 tasks.

**Post-MVP increment 1**: Phase 4 / US2 (T028–T031) — adds side-by-side preview. High UX
value; 4 tasks.

**Post-MVP increment 2**: Phase 5 / US3 (T032–T036) — adds MKV, AVI, WebM, FLV. Can start
in parallel with US2 increment. 5 tasks.

**Polish**: Phase 6 (T037–T040) — benchmarks, packaging, final quality gates. 4 tasks.
