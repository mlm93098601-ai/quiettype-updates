#!/usr/bin/env python3
"""Trust-boundary smoke checks; no microphone, installation or network mutation."""
import importlib.util
from pathlib import Path
import subprocess
import tempfile

PROJECT = Path(__file__).resolve().parent.parent
spec = importlib.util.spec_from_file_location('quiet_release', PROJECT / 'scripts/release.py')
release = importlib.util.module_from_spec(spec)
spec.loader.exec_module(release)
for url in [None, 'http://example.org/appcast.xml', 'https://user:secret@example.org/a', 'https://example.com/a', 'https://localhost/a', 'https://test.invalid/a']:
    try:
        release.https(url)
        raise AssertionError('Unsafe feed address accepted')
    except ValueError:
        pass
assert release.https('https://github.com/owner/repo/releases/download/updates/appcast.xml')
release.config(PROJECT / 'release/config.json', True)
with tempfile.TemporaryDirectory(dir=PROJECT / 'work') as temporary:
    file = Path(temporary) / 'archive.bin'
    file.write_bytes(b'QuietType updater verification fixture')
    tool = PROJECT / 'vendor/Sparkle/bin/sign_update'
    signature = subprocess.check_output([tool, '--account', 'QuietType', '-p', file], text=True).strip()
    subprocess.run([tool, '--account', 'QuietType', '--verify', file, signature], check=True)
    file.write_bytes(file.read_bytes() + b'tampered')
    result = subprocess.run([tool, '--account', 'QuietType', '--verify', file, signature], capture_output=True)
    assert result.returncode != 0, 'Tampered update accepted'
print('Feed URL validation, persistent signing-key match and tamper rejection passed')
