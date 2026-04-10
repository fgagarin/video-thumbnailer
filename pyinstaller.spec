# -*- mode: python ; coding: utf-8 -*-
"""PyInstaller spec for video-thumbnailer distribution builds.

Usage:
    pyinstaller pyinstaller.spec

Or via Makefile targets:
    make dist-linux
    make dist-macos
    make dist-windows
"""

import sys
from PyInstaller.utils.hooks import collect_data_files

# Collect runtime data files for PyAV (ffmpeg dylibs) and PySide6 (Qt plugins)
datas = []
datas += collect_data_files("av")
datas += collect_data_files("PySide6")

block_cipher = None

a = Analysis(
    ["src/video_thumbnailer/__main__.py"],
    pathex=["."],
    binaries=[],
    datas=datas,
    hiddenimports=[
        "av",
        "av.codec",
        "av.container",
        "av.stream",
        "PIL",
        "PIL.Image",
        "PIL.JpegImagePlugin",
        "PySide6",
        "PySide6.QtWidgets",
        "PySide6.QtCore",
        "PySide6.QtGui",
        "imageio_ffmpeg",
        "video_thumbnailer",
        "video_thumbnailer.core",
        "video_thumbnailer.platform",
        "video_thumbnailer.ui",
    ],
    hookspath=[],
    runtime_hooks=[],
    excludes=[],
    win_no_prefer_redirects=False,
    win_private_assemblies=False,
    cipher=block_cipher,
    noarchive=False,
)

pyz = PYZ(a.pure, a.zipped_data, cipher=block_cipher)

exe = EXE(
    pyz,
    a.scripts,
    a.binaries,
    a.zipfiles,
    a.datas,
    [],
    name="video-thumbnailer",
    debug=False,
    bootloader_ignore_signals=False,
    strip=False,
    upx=True,
    upx_exclude=[],
    runtime_tmpdir=None,
    console=False,
    disable_windowed_traceback=False,
    target_arch=None,
    codesign_identity=None,
    entitlements_file=None,
)
