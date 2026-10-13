#!/usr/bin/env python3
"""Refresh source metadata while preserving installed Sparkle configuration."""
import json
import plistlib
from pathlib import Path
import sys

PROJECT = Path(__file__).resolve().parent.parent


def sync(path):
    config = json.loads((PROJECT / 'src-tauri/tauri.conf.json').read_text())
    info = plistlib.loads(path.read_bytes())
    if info.get('CFBundleIdentifier') != config['identifier']:
        raise ValueError('Bundle identity differs; existing application was preserved')
    info.update(plistlib.loads((PROJECT / 'src-tauri/Info.plist').read_bytes()))
    info.update(CFBundleVersion=config['version'],
                CFBundleShortVersionString=config['version'],
                LSMinimumSystemVersion=config['bundle']['macOS']['minimumSystemVersion'])
    path.write_bytes(plistlib.dumps(info))


if __name__ == '__main__':
    sync(Path(sys.argv[1]))
