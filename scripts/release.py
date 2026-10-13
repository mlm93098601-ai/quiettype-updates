#!/usr/bin/env python3
"""Build, verify and package QuietType. Does not upload or change the installed app."""
import argparse
import base64
import hashlib
import importlib.util
import json
import os
from pathlib import Path
import plistlib
import re
import shutil
import subprocess
import tempfile
from urllib.parse import urlparse

PROJECT = Path(__file__).resolve().parent.parent
SPARKLE = PROJECT / 'vendor/Sparkle/bin'


def run(*args, **kwargs):
    subprocess.run([str(x) for x in args], check=True, **kwargs)


def https(value):
    if not value:
        raise ValueError('A permanent HTTPS update address is required for a release; use --preview for an installation preview')
    url = urlparse(value)
    if url.scheme != 'https' or not url.hostname or url.username or url.password or url.fragment or url.hostname.endswith(('.example', '.invalid')) or url.hostname in ['example.com', 'localhost', '127.0.0.1']:
        raise ValueError('Use an actual HTTPS address without credentials or placeholders')
    return value


def config(path, preview):
    data = json.loads(path.read_text())
    if len(base64.b64decode(data['public_key'], validate=True)) != 32:
        raise ValueError('Invalid Ed25519 public key')
    if not preview:
        https(data['feed_url'])
        https(data['download_url_prefix'])
    actual = subprocess.check_output([SPARKLE / 'generate_keys', '--account', data['keychain_account'], '-p'], text=True).strip()
    if actual != data['public_key']:
        raise ValueError('Keychain signing key differs from the public key; do not generate a replacement key')
    return data


def digest(file):
    h = hashlib.sha256()
    with file.open('rb') as f:
        for block in iter(lambda: f.read(8 * 1024 * 1024), b''):
            h.update(block)
    return h.hexdigest()


def sign(app):
    run('python3', PROJECT / 'scripts/sign-app.py', app)


def metadata(app, data, version, preview):
    file = app / 'Contents/Info.plist'
    info = plistlib.loads(file.read_bytes())
    info.update(CFBundleVersion=version, CFBundleShortVersionString=version,
                LSMinimumSystemVersion=data['minimum_system_version'],
                SUPublicEDKey=data['public_key'], SUEnableAutomaticChecks=True,
                SUScheduledCheckInterval=86400, SUAutomaticallyUpdate=False,
                SUEnableSystemProfiling=False, SUVerifyUpdateBeforeExtraction=True,
                SURequireSignedFeed=True, SUSignedFeedFailureExpirationInterval=0)
    if preview:
        info.pop('SUFeedURL', None)
    else:
        info['SUFeedURL'] = data['feed_url']
    file.write_bytes(plistlib.dumps(info))


def main():
    p = argparse.ArgumentParser(description=__doc__)
    p.add_argument('--version', required=True)
    p.add_argument('--config', type=Path, default=PROJECT / 'release/config.json')
    p.add_argument('--output', required=True, type=Path, help='Release directory; keep past update archives here for delta generation')
    p.add_argument('--bootstrap-runtime', type=Path, help='Include a prepared runtime in a first-install DMG')
    p.add_argument('--preview', action='store_true', help='Installation preview with networking disabled')
    p.add_argument('--skip-build', action='store_true', help='Reuse a just-built current app')
    a = p.parse_args()
    if not re.fullmatch(r'(0|[1-9]\d*)\.(0|[1-9]\d*)\.(0|[1-9]\d*)', a.version):
        p.error('Version must be an increasing X.Y.Z release version')
    data = config(a.config, a.preview)
    out = a.output.resolve()
    out.mkdir(parents=True, exist_ok=True)
    name = f'QuietType-{a.version}-mac-arm64' + ('-preview' if a.preview else '')
    updates = out / ('preview-updates' if a.preview else 'updates')
    updates.mkdir(exist_ok=True)
    archive, dmg = updates / (name + '.zip'), out / (name + '.dmg')
    if archive.exists() or dmg.exists():
        p.error('Version already packaged; existing releases are preserved')
    for old in out.glob('QuietType-*-mac-arm64*.json'):
        old_data = json.loads(old.read_text())
        if tuple(map(int, old_data['version'].split('.'))) >= tuple(map(int, a.version.split('.'))):
            p.error('Version must be higher than existing releases')
    if a.bootstrap_runtime:
        runtime = a.bootstrap_runtime.resolve()
        for item in ['runtime-version.json', 'python/bin/python3.13', '.venv/bin/python', 'silero-vad/model.safetensors', 'Qwen3-ASR-1.7B-bf16/model.safetensors', 'ollama-runtime/ollama', 'ollama-runtime/llama-server', 'editor-models/manifests/registry.ollama.ai/library/qwen3.5/4b']:
            if not (runtime / item).is_file():
                p.error(f'Incomplete bootstrap runtime: {item}')
    env = {**os.environ, 'CARGO_NET_OFFLINE': 'true'}
    with tempfile.TemporaryDirectory(prefix='release-', dir=PROJECT / 'work') as temporary:
        stage = Path(temporary)
        override = stage / 'tauri-version.json'
        override.write_text(json.dumps({'version': a.version}))
        if not a.skip_build:
            run('bash', PROJECT / 'scripts/build-app.sh', '--config', override, cwd=PROJECT, env=env)
        app = stage / 'update/QuietType.app'
        run('/usr/bin/ditto', PROJECT / 'src-tauri/target/release/bundle/macos/QuietType.app', app)
        if (app / 'Contents/Resources/quiettype-runtime').exists():
            raise ValueError('Build input unexpectedly includes a runtime; build a clean app first')
        metadata(app, data, a.version, a.preview)
        sign(app)
        # Small update archives contain the app only. Models live in Application Support.
        run('/usr/bin/ditto', '-c', '-k', '--sequesterRsrc', '--keepParent', app, archive)
        signature = subprocess.check_output([SPARKLE / 'sign_update', '--account', data['keychain_account'], '-p', archive], text=True).strip()
        (updates / (archive.name + '.sig')).write_text(signature + '\n')
        run(SPARKLE / 'sign_update', '--account', data['keychain_account'], '--verify', archive, signature)
        installer = stage / 'installer'
        installer.mkdir()
        install_app = installer / 'QuietType.app'
        run('/usr/bin/ditto', app, install_app)
        if a.bootstrap_runtime:
            run('/usr/bin/ditto', runtime, install_app / 'Contents/Resources/quiettype-runtime')
            sign(install_app)
        (installer / 'Applications').symlink_to('/Applications')
        runtime_note = ('首次启动会复制离线模型，请预留至少 25 GB 可用空间并等待\n'
                        if a.bootstrap_runtime else
                        '首次打开会自动下载约 6.8 GB 本地语音文件，支持断点续传，建议预留 30 GB 空间；已有模型不会重复下载。无需终端或下载器。\n')
        (installer / '安装说明.txt').write_text('QuietType\nApple 芯片 · macOS 26 或更高版本\n\n1. 把 QuietType 拖到 Applications\n2. 从应用程序打开；首次受阻时进入 系统设置 → 隐私与安全 → 仍要打开\n3. 允许麦克风与辅助功能，Fn/地球键设为“不执行任何操作”\n\n' + runtime_note + '语音识别和文字整理在本机进行\n\n这是小范围朋友使用的本地签名版本，没有 Apple 公证\n' + ('此预览包未配置联网更新，请勿当作正式发布包\n' if a.preview else '设置 → 软件更新 可检查更新，默认每天检查，有更新时确认安装\n'), encoding='utf-8')
        run('hdiutil', 'create', '-quiet', '-fs', 'HFS+', '-volname', 'QuietType', '-srcfolder', installer, '-format', 'UDZO', '-imagekey', 'zlib-level=1', dmg)
        run('hdiutil', 'verify', dmg)
    if not a.preview:
        run(SPARKLE / 'generate_appcast', '--account', data['keychain_account'], '--download-url-prefix', data['download_url_prefix'].rstrip('/') + '/', '--maximum-deltas', '3', '--delta-compression', 'lz4', '-o', updates / 'appcast.xml', updates)
        run(SPARKLE / 'sign_update', '--account', data['keychain_account'], '--verify', updates / 'appcast.xml')
    # GitHub's permanent latest-download link needs the same asset name in every release.
    direct = out / 'QuietType.zip'
    shutil.copy2(archive, direct)
    if a.bootstrap_runtime and not a.preview:
        spec = importlib.util.spec_from_file_location('installer_downloader', PROJECT / 'scripts/installer-downloader.py')
        downloader = importlib.util.module_from_spec(spec)
        spec.loader.exec_module(downloader)
        base_url = data['download_url_prefix'].rstrip('/').rsplit('/', 1)[0] + '/v' + a.version
        entry = downloader.make_installer(dmg, out / ('first-install-' + a.version), a.version, base_url)
        print('First-install entry: ' + str(entry))
    result = {'version': a.version, 'preview': a.preview, 'apple_notarized': False, 'bootstrap': bool(a.bootstrap_runtime), 'update_url': None if a.preview else data['feed_url'], 'files': {str(file.relative_to(out)): {'bytes': file.stat().st_size, 'sha256': digest(file)} for file in [archive, dmg, direct]}}
    (out / (name + '.json')).write_text(json.dumps(result, indent=2, ensure_ascii=False))
    print(json.dumps(result, indent=2, ensure_ascii=False))

if __name__ == '__main__':
    main()
