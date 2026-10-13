#!/usr/bin/env python3
"""Check in-place metadata updates using tiny plist fixtures, never an app."""
import importlib.util
import json
import plistlib
from pathlib import Path
import tempfile

PROJECT = Path(__file__).resolve().parent.parent
spec = importlib.util.spec_from_file_location('bundle_metadata', PROJECT / 'scripts/sync-bundle-metadata.py')
metadata = importlib.util.module_from_spec(spec)
spec.loader.exec_module(metadata)
config = json.loads((PROJECT / 'src-tauri/tauri.conf.json').read_text())
with tempfile.TemporaryDirectory(prefix='metadata-check-', dir=PROJECT / 'work') as temporary:
    path = Path(temporary) / 'Info.plist'
    original = {'CFBundleIdentifier': config['identifier'], 'CFBundleVersion': '0.0.0',
                'LSMinimumSystemVersion': '13.0', 'SUFeedURL': 'https://example.com/appcast.xml',
                'SUPublicEDKey': 'existing-key', 'SUEnableAutomaticChecks': False}
    path.write_bytes(plistlib.dumps(original))
    metadata.sync(path)
    updated = plistlib.loads(path.read_bytes())
    assert updated['CFBundleVersion'] == updated['CFBundleShortVersionString'] == config['version']
    assert updated['LSMinimumSystemVersion'] == config['bundle']['macOS']['minimumSystemVersion']
    assert all(updated[key] == original[key] for key in original if key.startswith('SU'))
    original['CFBundleIdentifier'] = 'another.app'
    path.write_bytes(plistlib.dumps(original))
    before = path.read_bytes()
    try:
        metadata.sync(path)
        raise AssertionError('Foreign bundle identity accepted')
    except ValueError:
        assert path.read_bytes() == before
print('Bundle version/system metadata, Sparkle preservation and identity checks passed')
