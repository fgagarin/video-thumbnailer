"""Unit tests for atomic_replace."""

from __future__ import annotations

import glob
from pathlib import Path

import pytest

from video_thumbnailer.core.atomic_write import atomic_replace


class TestAtomicReplace:
    def test_success_writes_content(self, tmp_path: Path) -> None:
        target = tmp_path / "target.dat"
        target.write_bytes(b"original")

        def write_fn(p: Path) -> None:
            p.write_bytes(b"updated")

        atomic_replace(target, write_fn)
        assert target.read_bytes() == b"updated"

    def test_failure_leaves_target_unchanged(self, tmp_path: Path) -> None:
        target = tmp_path / "target.dat"
        target.write_bytes(b"original")

        def write_fn(p: Path) -> None:
            raise RuntimeError("disk full simulation")

        with pytest.raises(RuntimeError, match="disk full"):
            atomic_replace(target, write_fn)

        assert target.read_bytes() == b"original"

    def test_failure_cleans_up_temp(self, tmp_path: Path) -> None:
        target = tmp_path / "target.dat"
        target.write_bytes(b"original")

        def write_fn(p: Path) -> None:
            p.write_bytes(b"partial")
            raise RuntimeError("interleaved failure")

        with pytest.raises(RuntimeError):
            atomic_replace(target, write_fn)

        leftover = glob.glob(str(tmp_path / ".vt_*.tmp"))
        assert leftover == [], f"Temp files not cleaned up: {leftover}"

    def test_temp_in_same_directory(self, tmp_path: Path) -> None:
        target = tmp_path / "target.dat"
        target.write_bytes(b"original")

        seen_tmp_dir: list[Path] = []

        def write_fn(p: Path) -> None:
            seen_tmp_dir.append(p.parent)
            p.write_bytes(b"updated")

        atomic_replace(target, write_fn)
        assert seen_tmp_dir[0] == tmp_path

    def test_creates_target_if_missing(self, tmp_path: Path) -> None:
        target = tmp_path / "new_file.dat"
        assert not target.exists()

        def write_fn(p: Path) -> None:
            p.write_bytes(b"brand new")

        atomic_replace(target, write_fn)
        assert target.read_bytes() == b"brand new"
