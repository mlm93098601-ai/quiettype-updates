#!/usr/bin/env python3
"""Create the first-install download entry from an already packaged full DMG."""
import hashlib
import json
from pathlib import Path
import re
import shlex
from urllib.parse import urlparse
import zipfile


def make_installer(dmg, output, version, base_url, chunk_bytes=400_000_000):
    url = urlparse(base_url)
    if not re.fullmatch(r'(0|[1-9]\d*)\.(0|[1-9]\d*)\.(0|[1-9]\d*)', version):
        raise ValueError('Installer version must be X.Y.Z')
    if url.scheme != 'https' or not url.hostname or url.username or url.password or url.query or url.fragment:
        raise ValueError('Installer download URL must be HTTPS without credentials')
    if chunk_bytes <= 0 or dmg.stat().st_size == 0:
        raise ValueError('Installer DMG and chunk size must be nonempty')
    output.mkdir(parents=True, exist_ok=False)
    filename = f'QuietType-{version}-full-mac-arm64.dmg'
    whole = hashlib.sha256()
    parts = []
    with dmg.open('rb') as source:
        index = 1
        while True:
            name = f'{filename}.part{index:02}'
            part = output / name
            digest, written = hashlib.sha256(), 0
            with part.open('wb') as target:
                while written < chunk_bytes:
                    block = source.read(min(8 * 1024 * 1024, chunk_bytes - written))
                    if not block:
                        break
                    target.write(block)
                    digest.update(block)
                    whole.update(block)
                    written += len(block)
            if not written:
                part.unlink()
                break
            parts.append({'name': name, 'bytes': written, 'sha256': digest.hexdigest()})
            index += 1
    size, checksum = dmg.stat().st_size, whole.hexdigest()
    script = '''#!/bin/bash
set -euo pipefail
trap 'code=$?; if [ "$code" -ne 0 ]; then echo "下载未完成，重新双击可继续；若无法连接 GitHub，请检查网络"; read -r -p "按回车关闭…" _ || true; fi' EXIT
echo 'QuietType VERSION 完整安装包，Apple 芯片 Mac、macOS 26 或更新，建议预留 30 GB'
if [ "$(/usr/bin/uname -m)" != arm64 ]; then echo '需要 Apple 芯片 Mac'; exit 1; fi
os_version=$(/usr/bin/sw_vers -productVersion)
if [ "${os_version%%.*}" -lt 26 ]; then echo '需要 macOS 26 或更新'; exit 1; fi
download_dir="$HOME/Downloads/QuietType-VERSION"
/bin/mkdir -p "$download_dir"
cd "$download_dir"
dmg=FILENAME
valid_file() {
    [ -f "$1" ] && [ "$(/usr/bin/stat -f %z "$1")" = "$2" ] &&
        [ "$(/usr/bin/shasum -a 256 "$1" | /usr/bin/cut -d ' ' -f 1)" = "$3" ]
}
if valid_file "$dmg" SIZE CHECKSUM; then /usr/bin/open "$download_dir/$dmg"; exit 0; fi
download_part() {
    name="$1"; size="$2"; checksum="$3"; url="$4"
    echo "正在下载 $name"
    if valid_file "$name" "$size" "$checksum"; then return; fi
    partial="$name.download"
    if ! valid_file "$partial" "$size" "$checksum"; then
        if [ -f "$partial" ] && [ "$(/usr/bin/stat -f %z "$partial")" -ge "$size" ]; then /bin/rm -f "$partial"; fi
        /usr/bin/curl --fail --location --retry 3 --connect-timeout 30 --proto '=https' --proto-redir '=https' \\
            --continue-at - --output "$partial" "$url"
    fi
    if ! valid_file "$partial" "$size" "$checksum"; then /bin/rm -f "$partial"; echo '分段校验失败，请重试'; exit 1; fi
    /bin/mv -f "$partial" "$name"
}
CALLS
echo '正在合并并校验完整安装包…'
/bin/cat PARTS > "$dmg.download"
if ! valid_file "$dmg.download" SIZE CHECKSUM; then echo '完整安装包校验失败，请重试'; exit 1; fi
/usr/bin/hdiutil verify "$dmg.download"
/bin/mv -f "$dmg.download" "$dmg"
/bin/rm -f PARTS
echo '请将 QuietType 拖入 Applications；首次受阻时在 系统设置 → 隐私与安全 中选择 仍要打开'
/usr/bin/open "$download_dir/$dmg"
'''
    calls = '\n'.join('download_part ' + ' '.join(shlex.quote(str(value)) for value in
                     [part['name'], part['bytes'], part['sha256'], base_url.rstrip('/') + '/' + part['name']])
                     for part in parts)
    replacements = {'VERSION': version, 'FILENAME': shlex.quote(filename), 'SIZE': str(size),
                    'CHECKSUM': shlex.quote(checksum),
                    'PARTS': ' '.join(shlex.quote(part['name']) for part in parts), 'CALLS': calls}
    for key, value in replacements.items():
        script = script.replace(key, value)
    instructions = (f'QuietType {version} 首次安装\n\n解压后双击“双击下载完整安装包.command”，'
                    '下载器会续传、校验并打开完整安装包，无需另装工具。\n'
                    '需要 Apple 芯片 Mac、macOS 26 或更新，建议 30 GB 可用空间。\n'
                    '将 QuietType 拖入 Applications，允许麦克风和辅助功能，并把 Fn/地球键设为“不执行任何操作”。\n'
                    '首次准备本地语音时请等待界面提示；后续只需在设置中更新程序。\n'
                    '当前为固定本地签名版本，尚未经过 Apple 公证；首次受阻时到系统设置的隐私与安全中选择“仍要打开”。\n')
    entry = output / 'QuietType-Installer.zip'
    with zipfile.ZipFile(entry, 'w', zipfile.ZIP_DEFLATED) as archive:
        command = zipfile.ZipInfo(f'QuietType-{version}-下载器/双击下载完整安装包.command')
        command.create_system = 3
        command.external_attr = 0o100755 << 16
        archive.writestr(command, script)
        archive.writestr(f'QuietType-{version}-下载器/先读我.txt', instructions)
    manifest = {'version': version, 'dmg': filename, 'bytes': size, 'sha256': checksum, 'parts': parts}
    (output / 'checksums.json').write_text(json.dumps(manifest, indent=2) + '\n')
    (output / '安装说明.txt').write_text(instructions)
    return entry
