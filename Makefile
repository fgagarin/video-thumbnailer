.PHONY: help sync dist-linux dist-macos dist-windows test lint typecheck coverage clean

UV ?= uv

help:
	@echo "video-thumbnailer build targets"
	@echo ""
	@echo "  sync         Install / sync all dependencies via uv"
	@echo "  dist-linux   Build a Linux single-file executable  (run on Linux)"
	@echo "  dist-macos   Build a macOS .app bundle             (run on macOS)"
	@echo "  dist-windows Build a Windows .exe                  (run on Windows)"
	@echo "  NOTE: PyInstaller cannot cross-compile. Each dist target must be"
	@echo "        run on its native OS. Use GitHub Actions for CI builds."
	@echo ""
	@echo "  test         Run unit and integration tests"
	@echo "  lint         Run ruff linter"
	@echo "  typecheck    Run mypy strict type check"
	@echo "  coverage     Run tests with >=80% coverage gate"
	@echo "  clean        Remove build artefacts"

# ── Environment ───────────────────────────────────────────────────────────────

sync:
	$(UV) sync --extra dev

# ── Distribution ─────────────────────────────────────────────────────────────

dist-linux:
	$(UV) run pyinstaller pyinstaller.spec \
		--distpath dist/linux \
		--workpath build/linux \
		--noconfirm

dist-macos:
	$(UV) run pyinstaller pyinstaller.spec \
		--distpath dist/macos \
		--workpath build/macos \
		--noconfirm

dist-windows:
	$(UV) run pyinstaller pyinstaller.spec \
		--distpath dist/windows \
		--workpath build/windows \
		--noconfirm

# ── Quality gates ─────────────────────────────────────────────────────────────

test:
	$(UV) run pytest tests/unit/ tests/integration/ -q

lint:
	$(UV) run ruff check src/ tests/

typecheck:
	$(UV) run mypy --strict src/

coverage:
	$(UV) run pytest --cov=video_thumbnailer \
		--cov-report=term-missing \
		--cov-fail-under=80 \
		tests/unit/ tests/integration/

# ── Housekeeping ──────────────────────────────────────────────────────────────

clean:
	rm -rf build/ dist/ __pycache__ .pytest_cache .mypy_cache \
		src/video_thumbnailer/__pycache__ \
		*.egg-info
