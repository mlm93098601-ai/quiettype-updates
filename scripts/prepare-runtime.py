#!/usr/bin/env python3
"""Prepare a relocatable, offline Apple Silicon runtime; no user data or keys."""
import argparse
import hashlib
import json
from pathlib import Path
import shutil
import subprocess


def copy(source, target):
    if source.is_dir():
        shutil.copytree(source, target, symlinks=True, ignore=shutil.ignore_patterns('__pycache__', '*.pyc', '.DS_Store', '.cache'))
    else:
        target.parent.mkdir(parents=True, exist_ok=True)
        shutil.copy2(source, target)


def apple_silicon_only(root):
    removed = 0
    for file in root.rglob("*"):
        if not file.is_file() or file.is_symlink():
            continue
        with file.open("rb") as stream:
            if stream.read(4) not in {b"\xfe\xed\xfa\xce", b"\xce\xfa\xed\xfe", b"\xfe\xed\xfa\xcf", b"\xcf\xfa\xed\xfe", b"\xca\xfe\xba\xbe", b"\xbe\xba\xfe\xca"}:
                continue
        result = subprocess.run(["lipo", "-archs", str(file)], capture_output=True, text=True, check=True)
        architectures = result.stdout.split()
        before = file.stat().st_size
        if "arm64" not in architectures:
            file.unlink()
        elif len(architectures) > 1:
            temporary = file.with_name(file.name + ".arm64")
            subprocess.run(["lipo", str(file), "-thin", "arm64", "-output", str(temporary)], check=True)
            temporary.chmod(file.stat().st_mode & 0o777)
            temporary.replace(file)
        removed += before - (file.stat().st_size if file.exists() else 0)
    print(f"Removed Intel-only runtime slices: {removed:,} bytes")


def main():
    p = argparse.ArgumentParser(description=__doc__)
    p.add_argument('--models', required=True, type=Path)
    p.add_argument('--python', required=True, type=Path, help='Relocatable CPython 3.13 installation root')
    p.add_argument('--ollama', required=True, type=Path, help='Ollama.app/Contents/Resources')
    p.add_argument('--output', required=True, type=Path)
    a = p.parse_args()
    if a.output.exists():
        p.error('Output must be a new directory; existing runtime is preserved')
    out = a.output
    out.mkdir(parents=True)
    copy(a.python, out / 'python')
    copy(a.models / '.venv/lib/python3.13/site-packages', out / '.venv/lib/python3.13/site-packages')
    (out / '.venv/bin').mkdir(parents=True)
    (out / '.venv/bin/python').symlink_to('../../python/bin/python3.13')
    for name in ['Qwen3-ASR-1.7B-bf16', 'silero-vad']:
        copy(a.models / name, out / name)
    # Include only qwen3.5:4b and the exact blobs referenced by its manifest.
    manifest = Path.home() / '.ollama/models/manifests/registry.ollama.ai/library/qwen3.5/4b'
    model = json.loads(manifest.read_text())
    copy(manifest, out / 'editor-models/manifests/registry.ollama.ai/library/qwen3.5/4b')
    for layer in [model['config'], *model['layers']]:
        name = layer['digest'].replace(':', '-')
        copy(Path.home() / '.ollama/models/blobs' / name, out / 'editor-models/blobs' / name)
    dest = out / 'ollama-runtime'
    dest.mkdir()
    for file in a.ollama.iterdir():
        if file.name in ['ollama', 'llama-server'] or file.suffix in ['.dylib', '.so'] or file.name.startswith('mlx_metal_') or 'LICENSE' in file.name or 'NOTICE' in file.name:
            copy(file, dest / file.name)
    apple_silicon_only(out)
    # Runtime copied into Application Support by the app; updates reuse this version.
    (out / 'runtime-version.json').write_text(json.dumps({'version': 1, 'python': '3.13', 'editor': 'qwen3.5:4b', 'minimum_macos': '26.0'}, indent=2))
    files = {}
    for file in sorted(out.rglob('*')):
        if file.is_file() and not file.is_symlink():
            digest = hashlib.sha256()
            with file.open('rb') as stream:
                for block in iter(lambda: stream.read(8 * 1024 * 1024), b''):
                    digest.update(block)
            files[str(file.relative_to(out))] = digest.hexdigest()
    (out / 'SHA256.json').write_text(json.dumps(files, indent=2))
    print(f'Prepared offline runtime: {out}')

if __name__ == '__main__':
    main()
