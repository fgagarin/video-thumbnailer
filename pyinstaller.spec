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
datas += [("icon.png", ".")]

app_icon = None
if sys.platform == "win32":
    app_icon = "icon.ico"
elif sys.platform == "darwin":
    app_icon = "icon.icns"

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

# macOS .app bundles must be built in onedir mode (a single-file EXE cannot be
# turned into a .app bundle — PyInstaller deprecates/errors on that combination).
# Linux and Windows keep the single-file onefile EXE.
_onedir = sys.platform == "darwin"

exe = EXE(
    pyz,
    a.scripts,
    [] if _onedir else a.binaries,
    [] if _onedir else a.zipfiles,
    [] if _onedir else a.datas,
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
    icon=app_icon,
    exclude_binaries=_onedir,
)

if _onedir:
    coll = COLLECT(
        exe,
        a.binaries,
        a.zipfiles,
        a.datas,
        strip=False,
        upx=True,
        upx_exclude=[],
        name="video-thumbnailer",
    )

    app = BUNDLE(
        coll,
        name="video-thumbnailer.app",
        icon=app_icon,
        bundle_identifier="com.video-thumbnailer.app",
        info_plist={
            "CFBundleName": "Video Thumbnailer",
            "CFBundleDisplayName": "Video Thumbnailer",
            "CFBundleShortVersionString": "0.1.0",
            "CFBundleVersion": "0.1.0",
            "NSHighResolutionCapable": True,
            "LSBackgroundOnly": False,
            "LSUIElement": False,
            "NSHumanReadableCopyright": "video-thumbnailer",
        },
    )
