#!/usr/bin/env python3
"""Verify installer splitting, current-version links and executable ZIP entry."""
import hashlib
import importlib.util
import json
from pathlib import Path
import subprocess
import tempfile
import zipfile

PROJECT = Path(__file__).resolve().parent.parent
spec = importlib.util.spec_from_file_location('installer_downloader', PROJECT / 'scripts/installer-downloader.py')
installer = importlib.util.module_from_spec(spec)
spec.loader.exec_module(installer)
with tempfile.TemporaryDirectory(prefix='installer-check-', dir=PROJECT / 'work') as temporary:
    root = Path(temporary)
    dmg = root / 'fixture.dmg'
    dmg.write_bytes(b'tiny installer fixture, no app or models')
    entry = installer.make_installer(dmg, root / 'output', '0.2.8',
                                     'https://github.com/owner/repo/releases/download/v0.2.8', chunk_bytes=9)
    manifest = json.loads((entry.parent / 'checksums.json').read_text())
    combined = b''.join((entry.parent / part['name']).read_bytes() for part in manifest['parts'])
    assert len(manifest['parts']) > 1 and combined == dmg.read_bytes()
    assert hashlib.sha256(combined).hexdigest() == manifest['sha256']
    assert all(hashlib.sha256((entry.parent / part['name']).read_bytes()).hexdigest() == part['sha256']
               for part in manifest['parts'])
    with zipfile.ZipFile(entry) as archive:
        command = next(item for item in archive.infolist() if item.filename.endswith('.command'))
        assert command.external_attr >> 16 & 0o111
        script = archive.read(command).decode()
        assert '/download/v0.2.8/' in script and '0.2.0' not in script and '--continue-at -' in script
        subprocess.run(['bash', '-n'], input=script, text=True, check=True)
    try:
        installer.make_installer(dmg, root / 'unsafe', '0.2.8', 'http://unsafe.example')
        raise AssertionError('Insecure download URL accepted')
    except ValueError:
        assert not (root / 'unsafe').exists()
print('Installer split hashes, versioned links, resume support and ZIP execution bits passed')
