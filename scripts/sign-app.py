#!/usr/bin/env python3
"""Reuse one private local signing identity so updates retain their code identity."""
import hashlib
import os
from pathlib import Path
import secrets
import subprocess
import sys
import tempfile

ROOT = Path.home() / "Library/Application Support/com.ai-quick-input.assistant/local-signing"
KEYCHAIN = ROOT / "QuietType.keychain-db"
PASSWORD = ROOT / "keychain-password"
CERTIFICATE = ROOT / "certificate.der"
PROJECT = Path(__file__).resolve().parent.parent


def run(*args):
    result = subprocess.run(args, capture_output=True, text=True)
    if result.returncode:
        # Never include command arguments: Security receives a generated keychain password.
        raise RuntimeError(result.stderr.strip() or "本地签名命令失败")
    return result.stdout + result.stderr


def identity():
    ROOT.mkdir(mode=0o700, parents=True, exist_ok=True)
    if not KEYCHAIN.exists():
        if PASSWORD.exists() or CERTIFICATE.exists():
            raise RuntimeError("本地签名资料不完整，请修复原身份，不要重新生成已授权的身份")
        password = secrets.token_hex(32)
        fd = os.open(PASSWORD, os.O_WRONLY | os.O_CREAT | os.O_EXCL, 0o600)
        with os.fdopen(fd, "w") as file:
            file.write(password)
        with tempfile.TemporaryDirectory(dir=ROOT) as temporary:
            folder = Path(temporary)
            config = folder / "openssl.cnf"
            config.write_text("""[req]
prompt = no
distinguished_name = subject
x509_extensions = extensions
[subject]
CN = QuietType Local Development
O = QuietType
[extensions]
basicConstraints = critical,CA:FALSE
keyUsage = critical,digitalSignature
extendedKeyUsage = critical,codeSigning
""")
            key, pem, p12 = [folder / name for name in ("private.pem", "certificate.pem", "identity.p12")]
            run("/usr/bin/openssl", "req", "-new", "-newkey", "rsa:2048", "-nodes", "-x509",
                "-sha256", "-days", "3650", "-config", str(config), "-keyout", str(key), "-out", str(pem))
            run("/usr/bin/openssl", "x509", "-in", str(pem), "-outform", "DER", "-out", str(CERTIFICATE))
            run("/usr/bin/openssl", "pkcs12", "-export", "-inkey", str(key), "-in", str(pem),
                "-out", str(p12), "-passout", "file:" + str(PASSWORD))
            # Keep the user's normal keychain search list unchanged; signing uses this keychain explicitly.
            import shlex
            previous = shlex.split(run("/usr/bin/security", "list-keychains", "-d", "user"))
            try:
                run("/usr/bin/security", "create-keychain", "-p", password, str(KEYCHAIN))
                run("/usr/bin/security", "import", str(p12), "-k", str(KEYCHAIN), "-P", password,
                    "-x", "-T", "/usr/bin/codesign")
                run("/usr/bin/security", "set-key-partition-list", "-S", "apple-tool:,codesign:",
                    "-s", "-k", password, str(KEYCHAIN))
            finally:
                run("/usr/bin/security", "list-keychains", "-d", "user", "-s", *previous)
    if not PASSWORD.is_file() or not CERTIFICATE.is_file():
        raise RuntimeError("本地签名资料不完整，请保留并修复原身份")
    run("/usr/bin/security", "unlock-keychain", "-p", PASSWORD.read_text(), str(KEYCHAIN))
    return hashlib.sha1(CERTIFICATE.read_bytes()).hexdigest().upper()


def sign_contents(path):
    fingerprint = identity()
    # Resources contain portable Python/Ollama executables and extension modules.
    # Sign every native file first, then nested bundles from the inside out.
    native_files = []
    magic = {b"\xfe\xed\xfa\xce", b"\xce\xfa\xed\xfe", b"\xfe\xed\xfa\xcf", b"\xcf\xfa\xed\xfe", b"\xca\xfe\xba\xbe", b"\xbe\xba\xfe\xca"}
    for file in path.rglob("*"):
        if file.is_file() and not file.is_symlink():
            with file.open("rb") as stream:
                if stream.read(4) in magic:
                    native_files.append(file)
    for file in native_files:
        run("/usr/bin/codesign", "--force", "--sign", fingerprint, "--keychain", str(KEYCHAIN),
            "--timestamp=none", str(file))
    bundles = sorted((file for file in path.rglob("*") if file.is_dir() and not file.is_symlink()
                      and file.suffix in {".app", ".xpc", ".framework"}), key=lambda file: len(file.parts), reverse=True)
    for bundle in bundles:
        run("/usr/bin/codesign", "--force", "--sign", fingerprint, "--keychain", str(KEYCHAIN),
            "--timestamp=none", str(bundle))
    runtime = path if (path / 'runtime-version.json').is_file() else path / "Contents/Resources/quiettype-runtime"
    manifest = runtime / "SHA256.json"
    if manifest.is_file():
        import json
        checksums = json.loads(manifest.read_text())
        for name in checksums:
            digest = hashlib.sha256()
            with (runtime / name).open("rb") as stream:
                for block in iter(lambda: stream.read(8 * 1024 * 1024), b""):
                    digest.update(block)
            checksums[name] = digest.hexdigest()
        manifest.write_text(json.dumps(checksums, indent=2))
    return fingerprint


def sign(path):
    fingerprint = sign_contents(path)
    run("/usr/bin/codesign", "--force", "--sign", fingerprint, "--keychain", str(KEYCHAIN),
        "--identifier", "com.ai-quick-input.assistant", "--entitlements", str(PROJECT / "src-tauri/Entitlements.plist"),
        "--timestamp=none", str(path))
    run("/usr/bin/codesign", "--verify", "--deep", "--strict", str(path))
    requirement = run("/usr/bin/codesign", "-d", "-r-", str(path))
    if "cdhash " in requirement or fingerprint.lower() not in requirement.lower():
        raise RuntimeError("签名没有绑定固定证书，停止更新")
    print(requirement.strip())


if __name__ == "__main__":
    try:
        sign(Path(sys.argv[1]).resolve())
    except (RuntimeError, IndexError) as error:
        sys.exit(str(error))
