# Developer Quickstart: Video Thumbnail Changer

**Branch**: `001-change-video-thumbnail` | **Updated**: 2026-04-10

---

## Prerequisites

| Tool | Version | Purpose |
|------|---------|---------|
| Python | 3.13+ | Runtime |
| uv | Latest | Dependency management + venv |
| ffmpeg | 6+ (system binary) | MKV/WebM/AVI thumbnail embedding + cache |
| Git | Any | Version control |

**Install uv** (if not already installed):
```bash
curl -LsSf https://astral.sh/uv/install.sh | sh
```

**Install ffmpeg**:
- **Linux**: `sudo apt install ffmpeg` or `sudo dnf install ffmpeg`
- **macOS**: `brew install ffmpeg`
- **Windows**: `winget install --id Gyan.FFmpeg` (or download from ffmpeg.org)

---

## Setup

```bash
# Clone and enter the repo
git clone <repo-url>
cd video-thumbnailer

# Create virtual environment and install all dependencies
uv venv
uv pip sync pyproject.toml --extra dev

# Verify installation
python -c "import av; import PySide6; import PIL; print('OK')"
ffmpeg -version | head -1
```

---

## Run the Application

```bash
python -m video_thumbnailer
```

The application window opens. Drag any supported video file (MP4, MOV, MKV, AVI, WebM,
FLV) onto the window to begin.

---

## Run Tests

```bash
# All tests
pytest

# Unit tests only
pytest tests/unit/

# Integration tests (requires ffmpeg on PATH and sample video files)
pytest tests/integration/

# With coverage report (enforces ≥80% branch coverage)
pytest --cov=video_thumbnailer --cov-report=term-missing --cov-fail-under=80

# Benchmark tests (frame extraction performance gate)
pytest tests/benchmarks/ --benchmark-only
```

### Test fixtures

Integration tests use sample video files stored under `tests/fixtures/`. Generate them
once using:
```bash
python tests/generate_fixtures.py
```

This script creates a 10-second test video in each of the 6 supported formats using
ffmpeg. Files are committed to the repo (they are small: ~50 KB each at low resolution).

---

## Lint and Type Check

```bash
# Linting (zero errors required before merge)
ruff check src/ tests/

# Type checking (strict mode, zero errors required)
mypy src/

# Auto-fix formatting
ruff format src/ tests/
```

---

## Project Structure Reference

```text
src/video_thumbnailer/
├── app.py                    # Entry point: python -m video_thumbnailer
├── ui/                       # PySide6 widgets
│   ├── main_window.py        # MainWindow, drag-drop, layout
│   ├── timeline_widget.py    # Custom scrubber widget
│   └── preview_widget.py     # Side-by-side thumbnail preview
├── core/                     # Business logic (no Qt dependencies)
│   ├── video_loader.py       # Open video, read metadata
│   ├── frame_extractor.py    # PyAV seek + decode
│   └── thumbnail_writer.py   # Embed cover art, atomic write
└── platform/                 # OS-specific cache refresh
    ├── cache_linux.py
    ├── cache_macos.py
    └── cache_windows.py
```

---

## Key Design Decisions (summary)

| Decision | Choice | See |
|----------|--------|-----|
| GUI framework | PySide6 (Qt6) | research.md R-001 |
| Frame extraction | PyAV (FFmpeg bindings) | research.md R-002 |
| Thumbnail embedding | PyAV (MP4/MOV) + ffmpeg subprocess (MKV/WebM/AVI) | research.md R-003 |
| File-manager refresh | XDG cache / qlmanage / SHChangeNotify | research.md R-004 |
| Atomic write | tempfile + os.replace() | research.md R-005 |
| Dependency manager | uv + pyproject.toml | research.md R-006 |

---

## Contributing

1. Branch from `main` using `NNN-kebab-description` naming.
2. Write tests before or alongside implementation (TDD preferred).
3. Run `ruff check`, `mypy src/`, and `pytest --cov` before pushing.
4. All CI gates must be green before requesting review (see `Quality Gates` in constitution).

---

## Distribution

Pre-built single-file executables are produced with [PyInstaller](https://pyinstaller.org).
The spec lives at `pyinstaller.spec` in the repository root.

### Prerequisites

```bash
uv pip install pyinstaller
```

### Build commands

| Platform | Command           | Output path              |
|----------|-------------------|--------------------------|
| Linux    | `make dist-linux`  | `dist/linux/video-thumbnailer` |
| macOS    | `make dist-macos`  | `dist/macos/video-thumbnailer` |
| Windows  | `make dist-windows`| `dist/windows/video-thumbnailer.exe` |

Each target produces a single self-contained binary that bundles the Python
runtime, Qt libraries, PyAV (with the bundled `imageio-ffmpeg` binary), and
Pillow.  No separate Python install is required on the target machine.

### Manual build

```bash
pyinstaller pyinstaller.spec --distpath dist/linux --noconfirm
```
