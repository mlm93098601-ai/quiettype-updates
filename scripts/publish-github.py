#!/usr/bin/env python3
"""Upload versioned app updates first and atomically replace the signed feed last."""
import argparse
import json
from pathlib import Path
import subprocess
import tempfile
import hashlib
import re
from urllib.parse import urlparse
import xml.etree.ElementTree as ET

PROJECT = Path(__file__).resolve().parent.parent


def run(*args, capture=False):
    return subprocess.run([str(x) for x in args], check=True, capture_output=capture, text=True)


def download(url, destination):
    run('/usr/bin/curl', '--fail', '--silent', '--show-error', '--location',
        '--retry', '3', '--retry-all-errors', '--max-time', '120', '--proto', '=https', '--proto-redir', '=https',
        '--output', destination, url)


def sha(path):
    digest = hashlib.sha256()
    with path.open('rb') as stream:
        for block in iter(lambda: stream.read(8 * 1024 * 1024), b''):
            digest.update(block)
    return digest.hexdigest()

def main():
    p = argparse.ArgumentParser(description=__doc__)
    p.add_argument('--repo', required=True, help='OWNER/REPOSITORY, public GitHub repository with at least one commit')
    p.add_argument('--release-dir', type=Path, required=True)
    a = p.parse_args()
    expected = f'https://github.com/{a.repo}/releases/download/updates/'
    config = json.loads((PROJECT / 'release/config.json').read_text())
    if config['feed_url'] != expected + 'appcast.xml' or config['download_url_prefix'].rstrip('/') + '/' != expected:
        p.error('Repository differs from the permanent URL embedded in the release')
    details = json.loads(run('gh', 'repo', 'view', a.repo, '--json', 'isPrivate', capture=True).stdout)
    if details['isPrivate']:
        p.error('A public repository is required: recipients must download without a token')
    updates = a.release_dir.resolve() / 'updates'
    feed = updates / 'appcast.xml'
    run(PROJECT / 'vendor/Sparkle/bin/sign_update', '--account', config['keychain_account'], '--verify', feed)
    tree = ET.parse(feed)
    urls = [node.attrib['url'] for node in tree.findall('.//enclosure')]
    notes = tree.findall('.//{http://www.andymatuschak.org/xml-namespaces/sparkle}releaseNotesLink')
    urls.extend(node.text.strip() for node in notes if node.text)
    if not urls or any(not url.startswith(expected) for url in urls):
        p.error('Feed archive addresses do not match this repository')
    # Publish every file referenced by the signed feed, including release notes.
    assets = []
    for url in urls:
        file = updates / Path(urlparse(url).path).name
        if not file.is_file() or file.stat().st_size >= 2_000_000_000:
            p.error(f'Missing asset or exceeds GitHub asset size: {file.name}')
        assets.append(file)
    versions = [re.fullmatch(r'QuietType-(\d+\.\d+\.\d+)-mac-arm64\.zip', file.name) for file in assets]
    version = max((match.group(1) for match in versions if match), key=lambda value: tuple(map(int, value.split('.'))))
    latest = json.loads(run('gh', 'api', f'repos/{a.repo}/releases/latest', capture=True).stdout)
    if latest['tag_name'] != 'v' + version or not {'QuietType.zip', 'QuietType-Source.zip'}.issubset({asset['name'] for asset in latest['assets']}):
        p.error('Publish the matching latest release with QuietType.zip and QuietType-Source.zip before updating the feed')
    state = subprocess.run(['gh', 'release', 'view', 'updates', '--repo', a.repo], capture_output=True)
    if state.returncode:
        run('gh', 'release', 'create', 'updates', '--repo', a.repo, '--title', 'QuietType 自动更新', '--notes', '签名更新包，仅供 QuietType 检查更新使用，首次安装请使用开发者提供的完整 DMG')
    existing = json.loads(run('gh', 'release', 'view', 'updates', '--repo', a.repo, '--json', 'assets', capture=True).stdout)['assets']
    names = {asset['name'] for asset in existing}
    for file in assets:
        if file.name not in names:
            run('gh', 'release', 'upload', 'updates', file, '--repo', a.repo)
        # macOS curl uses system TLS trust; avoid a separate Python CA store.
        # Download to a temporary file and hash in chunks, including large assets.
        with tempfile.TemporaryDirectory(dir=PROJECT / 'work') as temporary:
            remote = Path(temporary) / file.name
            download(expected + file.name, remote)
            if sha(remote) != sha(file):
                raise ValueError(f'Published asset checksum mismatch: {file.name}; feed left unchanged')
    run('gh', 'release', 'upload', 'updates', feed, '--repo', a.repo, '--clobber')
    with tempfile.TemporaryDirectory(dir=PROJECT / 'work') as temporary:
        remote = Path(temporary) / 'appcast.xml'
        download(config['feed_url'], remote)
        if remote.read_bytes() != feed.read_bytes():
            raise ValueError('Live update feed differs from the local signed feed')
    print('Live signed update feed verified: ' + config['feed_url'])

if __name__ == '__main__':
    main()
