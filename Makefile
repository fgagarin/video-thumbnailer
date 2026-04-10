.PHONY: help dist-linux dist-macos dist-windows test lint typecheck coverage clean

PYTHON ?= python3
PYINSTALLER ?= pyinstaller

help:
	@echo "video-thumbnailer build targets"
	@echo ""
	@echo "  dist-linux   Build a Linux single-file executable"
	@echo "  dist-macos   Build a macOS .app bundle"
	@echo "  dist-windows Build a Windows .exe"
	@echo ""
	@echo "  test         Run unit and integration tests"
	@echo "  lint         Run ruff linter"
	@echo "  typecheck    Run mypy strict type check"
	@echo "  coverage     Run tests with >=80% coverage gate"
	@echo "  clean        Remove build artefacts"

# ── Distribution ─────────────────────────────────────────────────────────────

dist-linux:
	$(PYINSTALLER) pyinstaller.spec \
		--distpath dist/linux \
		--workpath build/linux \
		--noconfirm

dist-macos:
	$(PYINSTALLER) pyinstaller.spec \
		--distpath dist/macos \
		--workpath build/macos \
		--noconfirm

dist-windows:
	$(PYINSTALLER) pyinstaller.spec \
		--distpath dist/windows \
		--workpath build/windows \
		--noconfirm

# ── Quality gates ─────────────────────────────────────────────────────────────

test:
	cd src && pytest ../tests/ -q

lint:
	ruff check src/ tests/

typecheck:
	mypy --strict src/

coverage:
	pytest --cov=video_thumbnailer \
		--cov-report=term-missing \
		--cov-fail-under=80 \
		tests/unit/ tests/integration/

# ── Housekeeping ──────────────────────────────────────────────────────────────

clean:
	rm -rf build/ dist/ __pycache__ .pytest_cache .mypy_cache \
		src/video_thumbnailer/__pycache__ \
		*.egg-info
