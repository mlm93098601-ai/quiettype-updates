import hashlib
import importlib.util
import json
from pathlib import Path
import tarfile
import tempfile

PROJECT = Path(__file__).resolve().parent.parent
spec = importlib.util.spec_from_file_location('package_runtime', PROJECT / 'scripts/package-runtime.py')
module = importlib.util.module_from_spec(spec); spec.loader.exec_module(module)
with tempfile.TemporaryDirectory(dir=PROJECT / 'work') as temporary:
    root = Path(temporary)
    source = root / 'source'; source.mkdir()
    (source / 'runtime-version.json').write_text('{"version":1}')
    (source / 'content').write_bytes(b'portable runtime fixture')
    (source / 'alias').symlink_to('content')
    manifest = module.package(source, root / 'output', 'https://github.com/owner/repo/releases/download/runtime-v1', 31)
    archive = root / 'output/QuietType-Runtime-v1.tar.gz'
    combined = b''.join((root / 'output' / part['name']).read_bytes() for part in manifest['parts'])
    assert combined == archive.read_bytes()
    assert hashlib.sha256(combined).hexdigest() == manifest['sha256']
    assert all(hashlib.sha256((root / 'output' / p['name']).read_bytes()).hexdigest() == p['sha256'] for p in manifest['parts'])
    with tarfile.open(archive) as tar:
        assert tar.extractfile('./content').read() == b'portable runtime fixture'
        assert tar.getmember('./alias').linkname == 'content'
    (source / 'outside').symlink_to(root.parent)
    try:
        module.package(source, root / 'unsafe', 'https://github.com/owner/repo/releases/download/runtime-v1')
        raise AssertionError('External link accepted')
    except ValueError:
        assert not (root / 'unsafe').exists()
print('Runtime archive content, internal links, split hashes and external-link refusal passed')
