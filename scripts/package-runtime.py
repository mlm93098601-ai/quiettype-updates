#!/usr/bin/env python3
"""Package the existing offline runtime once; later app releases reuse it."""
import argparse
import hashlib
import importlib.util
import json
from pathlib import Path
import subprocess

PROJECT = Path(__file__).resolve().parent.parent


def package(runtime, output, base_url, chunk_bytes=1_000_000_000):
    root = runtime.resolve()
    expanded = 0
    for path in root.rglob('*'):
        if path.is_symlink() and not path.resolve().is_relative_to(root):
            raise ValueError('Runtime contains an external link')
        if path.is_file() and not path.is_symlink():
            expanded += path.stat().st_size
    if json.loads((root / 'runtime-version.json').read_text())['version'] != 1:
        raise ValueError('Unsupported runtime version')
    output.mkdir(parents=True, exist_ok=False)
    archive = output / 'QuietType-Runtime-v1.tar.gz'
    with archive.open('wb') as destination:
        tar = subprocess.Popen(['/usr/bin/tar', '-cf', '-', '--no-xattrs', '-C', str(root), '.'], stdout=subprocess.PIPE)
        gzip = subprocess.Popen(['/usr/bin/gzip', '-1'], stdin=tar.stdout, stdout=destination)
        tar.stdout.close()
        gzip_status, tar_status = gzip.wait(), tar.wait()
        if gzip_status or tar_status:
            raise RuntimeError('Runtime archive failed')
    whole, parts = hashlib.sha256(), []
    with archive.open('rb') as source:
        index = 1
        while block := source.read(min(chunk_bytes, 8 * 1024 * 1024)):
            name = archive.name + f'.part{index:02}'
            digest, count = hashlib.sha256(), 0
            with (output / name).open('wb') as target:
                while block:
                    target.write(block); digest.update(block); whole.update(block); count += len(block)
                    if count == chunk_bytes:
                        break
                    block = source.read(min(chunk_bytes - count, 8 * 1024 * 1024))
            parts.append(dict(name=name, bytes=count, sha256=digest.hexdigest(), url=base_url.rstrip('/') + '/' + name))
            print(f'Prepared {name}: {count:,} bytes', flush=True)
            index += 1
    manifest = dict(version=1, bytes=archive.stat().st_size, expandedBytes=expanded,
                    sha256=whole.hexdigest(), parts=parts)
    (output / 'runtime-download.json').write_text(json.dumps(manifest, indent=2) + '\n')
    return manifest


if __name__ == '__main__':
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument('--runtime', type=Path, required=True)
    parser.add_argument('--output', type=Path, required=True)
    args = parser.parse_args()
    spec = importlib.util.spec_from_file_location('sign_app', PROJECT / 'scripts/sign-app.py')
    signing = importlib.util.module_from_spec(spec); spec.loader.exec_module(signing)
    signing.sign_contents(args.runtime)
    config = json.loads((PROJECT / 'release/config.json').read_text())
    base = config['download_url_prefix'].rstrip('/').rsplit('/', 1)[0] + '/runtime-v1'
    package(args.runtime, args.output, base)
