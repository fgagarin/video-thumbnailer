"""Atomic file replacement utility.

Guarantees that if any step fails, the original file at ``target_path`` remains
completely unmodified.

Atomicity guarantee:
    The temp file is created in the *same directory* as ``target_path``, ensuring both
    reside on the same filesystem mount. ``os.replace()`` then maps to ``rename(2)`` on
    POSIX (atomic) and ``ReplaceFileW`` on Windows (also atomic). If the process is
    killed between the write and the rename, the original file is left intact.

Same-filesystem requirement:
    If the temp file were created on a different filesystem (e.g. ``/tmp`` when the
    target is on a network share), the rename would degrade to a copy+delete, which is
    not atomic. Using the target's own directory prevents this.
"""

from __future__ import annotations

import os
import shutil
import tempfile
from collections.abc import Callable
from pathlib import Path

__all__ = ["atomic_replace"]


def atomic_replace(target_path: Path | str, write_fn: Callable[[Path], None]) -> None:
    """Atomically replace ``target_path`` using ``write_fn``.

    Procedure:
        1. Create a temp file in the same directory as ``target_path``.
        2. If ``target_path`` exists, copy it into the temp file (preserves metadata).
        3. Call ``write_fn(tmp_path)`` to write/modify the temp file.
        4. Call ``os.replace(tmp_path, target_path)`` to atomically rename.

    On any exception in steps 2–4, the temp file is deleted and the exception is
    re-raised. ``target_path`` is never modified when an exception occurs.

    Args:
        target_path: Absolute or relative path to the file to be replaced or created.
        write_fn: Callable that receives the temp file ``Path`` and writes it in-place.
            May raise any exception; the original file will be left intact.

    Raises:
        Any exception raised by ``write_fn`` or by OS operations.
    """
    abs_target = Path(target_path).resolve()
    directory = abs_target.parent

    with tempfile.NamedTemporaryFile(
        dir=directory, prefix=".vt_", suffix=".tmp", delete=False
    ) as tmp:
        tmp_path = Path(tmp.name)

    try:
        if abs_target.exists():
            shutil.copy2(abs_target, tmp_path)
        write_fn(tmp_path)
        os.replace(tmp_path, abs_target)
    except Exception:
        try:
            tmp_path.unlink(missing_ok=True)
        except OSError:
            pass
        raise
