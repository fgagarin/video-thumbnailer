# video-thumbnailer Development Guidelines

Auto-generated from all feature plans. Last updated: 2026-04-10

## Active Technologies

- Python 3.13 + PySide6 6.x (Qt6 GUI), PyAV 12+ (video decode / frame seek), (001-change-video-thumbnail)

## Project Structure

```text
src/
tests/
```

## Commands

cd src && pytest && ruff check .

## Code Style

Python 3.13: Follow standard conventions

## Recent Changes

- 001-change-video-thumbnail: Added Python 3.13 + PySide6 6.x (Qt6 GUI), PyAV 12+ (video decode / frame seek),

<!-- MANUAL ADDITIONS START -->

## Build, test, lint

Use `uv` (preferred) or an activated `.venv` with `pip install -e ".[dev]"`.

```bash
make sync         # uv sync --extra dev
make test         # pytest tests/unit/ tests/integration/ -q
make coverage     # same, with --cov-fail-under=80 gate (branch coverage)
make lint         # ruff check src/ tests/
make typecheck    # mypy --strict src/
```

Run a single test (examples):

```bash
pytest tests/unit/test_frame_extractor.py::test_extract_at_offset -q
pytest tests/unit/test_frame_extractor.py -k "extract" -q
```

- `tests/unit/` — pure logic, no real video I/O beyond generated fixtures.
- `tests/integration/` — exercises real per-format workflows (`test_mp4_workflow.py`, `test_mkv_workflow.py`, etc.) using ffmpeg-generated fixture videos from `tests/conftest.py` (via `imageio_ffmpeg`); no network access, no committed binary fixtures.
- `tests/benchmarks/` — `pytest-benchmark`; run with `pytest tests/benchmarks/ --benchmark-only` (excluded from `make test`/CI's default `test` job, run separately in CI's `benchmark` job).
- CI (`.github/workflows/ci.yml`) runs lint/typecheck/test/benchmark/dist across ubuntu/macos/windows; on Linux, `QT_QPA_PLATFORM=offscreen` is required for the Qt GUI tests to run headless.
- `ruff` selects `E, F, I` only, line-length 88; `tests/**` is exempt from `E501` (long lines OK in tests). `mypy --strict` applies to `src/` only.

## Architecture

Layered, dependency-injected design — `app.py::main()` constructs concrete implementations and injects them into `MainWindow`; nothing below `ui/` imports Qt:

- **`core/`** — framework-agnostic video I/O, each behind a narrow class implementing a `Protocol` documented in `specs/001-change-video-thumbnail/contracts/core_interfaces.md`:
  - `video_loader.py::PyAVVideoLoader` — opens a file with PyAV, detects `VideoFormat` (container name + extension heuristics for the ambiguous `mov,mp4` and `matroska,webm` PyAV format strings), reads any existing embedded thumbnail.
  - `frame_extractor.py::PyAVFrameExtractor` — seeks/decodes a single frame at a `TimelinePosition` into a PIL `Image`.
  - `thumbnail_writer.py::FormatDispatchThumbnailWriter` — dispatches embedding strategy by `VideoFormat`: MP4/MOV use ffmpeg `-disposition:v:1 attached_pic`; MKV/WebM use ffmpeg `-attach`; AVI re-muxes without cover art; FLV/WebM have no embeddable cover art (XDG cache thumbnail only). All mutation goes through `atomic_write.py::atomic_replace` so the source file is never left corrupt on failure. Never raises for expected failure modes — always returns `ApplyResult(success=False, error_code=ApplyError...)`.
  - `atomic_write.py` — write-to-temp-then-rename helper used by any code that mutates a video file in place.
  - `thumbnail_metadata.py` — reads/writes a sidecar marker recording which frame/position produced the current embedded thumbnail (`VideoFile.thumbnail_frame_number` / `thumbnail_position_ms`).
- **`platform/`** — OS-specific "tell the file manager to refresh its thumbnail cache" implementations behind the `CacheInvalidator` Protocol (`platform/__init__.py`). `get_cache_invalidator()` picks `cache_linux.py` / `cache_macos.py` / `cache_windows.py` by `sys.platform` at runtime. Implementations must never raise; failures are logged as warnings only.
- **`ui/`** — PySide6 widgets. `main_window.py::MainWindow` receives the loader/extractor/writer/invalidator as constructor args (see `app.py`) and owns the `QThreadPool`. Long-running work (load/extract/apply) is never run on the GUI thread — it's wrapped in a `QRunnable` from `worker.py` (`VideoLoadWorker`, `FrameExtractWorker`, `ApplyWorker`), each exposing `signals.finished`/`signals.error` `Signal`s connected back into `MainWindow` slots. `timeline_widget.py` and `filmstrip_widget.py` handle scrubbing/preview UI; `preview_widget.py` renders the current frame.
- **`models.py`** — the only shared vocabulary between layers: `VideoFile`, `VideoFormat`, `TimelinePosition`, `ThumbnailFrame`, `ApplyOperation`/`ApplyResult`/`ApplyStatus`/`ApplyError`. Add new cross-layer data shapes here, not in `core/` or `ui/`.
- **`exceptions.py`** — domain exceptions raised by `core/` (`UnsupportedFormatError`, `NoVideoStreamError`, `ExtractionError`), all deriving from `VideoThumbnailerError`. Prefer these over generic exceptions when a caller needs to distinguish failure kinds; `ApplyResult.error_code` is the equivalent typed-failure mechanism for the write path, which deliberately swallows exceptions instead of raising.

## Conventions

- Every module starts with `from __future__ import annotations` and declares `__all__`.
- Public classes/functions get Google-style docstrings with `Args:`/`Returns:`/`Raises:` sections (see any file in `core/` or `models.py`).
- New video container support requires touching four places: `VideoFormat` enum (`models.py`), format detection (`video_loader.py::_detect_format`), the write-dispatch format sets (`thumbnail_writer.py`), and a fixture generator entry in `tests/conftest.py`/`tests/generate_fixtures.py`.
- Spec-driven feature work lives under `specs/<NNN-feature-name>/` (spec.md, plan.md, tasks.md, contracts/) via the Spec Kit `speckit.*` slash commands/agents in `.github/prompts` and `.github/agents`; this file's header/Active-Technologies/Recent-Changes sections above are auto-regenerated by `.specify/scripts/bash/update-agent-context.sh` — only edit within this MANUAL ADDITIONS block.

<!-- MANUAL ADDITIONS END -->
