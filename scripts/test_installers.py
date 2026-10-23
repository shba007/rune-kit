"""Offline installer acceptance tests; never modifies the real user PATH or data.
Run: python scripts/test_installers.py --binary target/debug/rune-kit.exe
Windows exercises PowerShell 5.1 and 7 when available; Git Bash exercises POSIX flow.
"""
import argparse
import hashlib
import io
import os
from pathlib import Path
import shutil
import subprocess
import tarfile
import tempfile
import unittest
import zipfile

ROOT = Path(__file__).resolve().parents[1]
parser = argparse.ArgumentParser()
parser.add_argument('--binary', type=Path, required=True)
options, remaining = parser.parse_known_args()
BINARY = options.binary.resolve()
VERSION = subprocess.check_output([str(BINARY), '--version'], text=True).strip().split()[1]
TAG = 'v' + VERSION
POWERSHELL = shutil.which('pwsh') or shutil.which('powershell')
BASH = str(Path(os.environ.get('ProgramFiles', 'C:/Program Files')) / 'Git/bin/bash.exe') if os.name == 'nt' else shutil.which('sh')
if BASH and not Path(BASH).is_file():
    BASH = None


class Installers(unittest.TestCase):
    @classmethod
    def setUpClass(cls):
        cls.sandbox = tempfile.TemporaryDirectory(prefix='rune-kit-install-tests-')
        cls.base = Path(cls.sandbox.name)
        cls.zip = cls.base / 'fixture.zip'
        with zipfile.ZipFile(cls.zip, 'w', zipfile.ZIP_DEFLATED, compresslevel=1) as archive:
            archive.write(BINARY, 'rune-kit.exe')
        cls.legacy = cls.base / 'legacy.zip'
        with zipfile.ZipFile(cls.legacy, 'w', zipfile.ZIP_DEFLATED, compresslevel=1) as archive:
            archive.write(BINARY, 'rune.exe')
        cls.tar = cls.base / 'fixture.tar.gz'
        with tarfile.open(cls.tar, 'w:gz') as archive:
            data = f'#!/bin/sh\nprintf "rune-kit {VERSION}\\n"\n'.encode()
            member = tarfile.TarInfo('rune-kit')
            member.mode = 0o755
            member.size = len(data)
            archive.addfile(member, io.BytesIO(data))

    @classmethod
    def tearDownClass(cls):
        cls.sandbox.cleanup()

    def setUp(self):
        self.temp = tempfile.TemporaryDirectory(prefix='case-', dir=self.base)
        self.addCleanup(self.temp.cleanup)
        self.root = Path(self.temp.name)
        self.mirror = self.root / 'release files'
        self.mirror.mkdir()
        self.install = self.root / "user's bin"
        self.env = os.environ.copy()
        self.env.update(RUNE_KIT_VERSION=TAG, RUNE_KIT_BASE_URL=self.mirror.as_posix(),
                        RUNE_KIT_INSTALL_DIR=self.install.as_posix(), RUNE_KIT_NO_MODIFY_PATH='1',
                        RUNE_KIT_TARGET='x86_64-unknown-linux-gnu')

    def package(self, source, name):
        target = self.mirror / name
        shutil.copyfile(source, target)
        digest = hashlib.sha256(target.read_bytes()).hexdigest()
        target.with_name(name + '.sha256').write_text(f'{digest}  {name}\n', encoding='ascii')
        return target

    def run_installer(self, windows=False, shell=None, success=True):
        command = ([shell or POWERSHELL, '-NoProfile', '-NonInteractive', '-File', str(ROOT/'install.ps1')]
                   if windows else [BASH, str(ROOT/'install.sh')])
        process = subprocess.run(command, env=self.env, text=True, capture_output=True, timeout=120)
        if success:
            self.assertEqual(process.returncode, 0, process.stdout + process.stderr)
        else:
            self.assertNotEqual(process.returncode, 0, process.stdout + process.stderr)
        return process

    @unittest.skipUnless(POWERSHELL and os.name == 'nt', 'PowerShell on Windows required')
    def test_windows_install_and_reinstall_canonical_binary(self):
        self.package(self.zip, f'rune-kit-{TAG}-x86_64-pc-windows-msvc.zip')
        self.run_installer(windows=True)
        self.run_installer(windows=True)
        installed = self.install / 'rune-kit.exe'
        self.assertEqual(installed.read_bytes(), BINARY.read_bytes())
        self.assertFalse((self.install/'rune.exe').exists())
        self.assertEqual(sorted(p.name for p in self.install.iterdir()), ['rune-kit.exe'])

    @unittest.skipUnless(shutil.which('powershell') and os.name == 'nt', 'Windows PowerShell 5.1 required')
    def test_windows_powershell_51(self):
        self.package(self.zip, f'rune-kit-{TAG}-x86_64-pc-windows-msvc.zip')
        self.run_installer(windows=True, shell=shutil.which('powershell'))
        self.assertTrue((self.install/'rune-kit.exe').is_file())

    @unittest.skipUnless(POWERSHELL and os.name == 'nt', 'PowerShell on Windows required')
    def test_windows_legacy_archive_installs_only_new_command(self):
        self.package(self.legacy, f'rune-{TAG}-x86_64-pc-windows-msvc.zip')
        self.run_installer(windows=True)
        self.assertTrue((self.install/'rune-kit.exe').is_file())
        self.assertFalse((self.install/'rune.exe').exists())

    @unittest.skipUnless(POWERSHELL and os.name == 'nt', 'PowerShell on Windows required')
    def test_windows_bad_checksum_preserves_existing_install(self):
        target = self.package(self.zip, f'rune-kit-{TAG}-x86_64-pc-windows-msvc.zip')
        target.with_name(target.name+'.sha256').write_text('0'*64, encoding='ascii')
        self.install.mkdir()
        (self.install/'rune-kit.exe').write_bytes(b'previous installation')
        process = self.run_installer(windows=True, success=False)
        self.assertIn('Checksum mismatch', process.stdout + process.stderr)
        self.assertEqual((self.install/'rune-kit.exe').read_bytes(), b'previous installation')

    @unittest.skipUnless(POWERSHELL and os.name == 'nt', 'PowerShell on Windows required')
    def test_windows_missing_checksum_does_not_install(self):
        target = self.package(self.zip, f'rune-kit-{TAG}-x86_64-pc-windows-msvc.zip')
        target.with_name(target.name+'.sha256').unlink()
        self.run_installer(windows=True, success=False)
        self.assertFalse(self.install.exists())

    @unittest.skipUnless(BASH, 'POSIX shell or Git Bash required')
    def test_posix_default_shared_root_and_path_once(self):
        self.package(self.tar, f'rune-kit-{TAG}-x86_64-unknown-linux-gnu.tar.gz')
        home = self.root / "home with spaces"
        shared = home / 'rune'
        shared.mkdir(parents=True)
        (shared/'rune-infer').write_bytes(b'infer')
        (shared/'infer').mkdir()
        (shared/'infer/config').write_bytes(b'configuration')
        self.env.pop('RUNE_KIT_INSTALL_DIR')
        self.env.update(HOME=home.as_posix(), RUNE_KIT_NO_MODIFY_PATH='0')
        self.run_installer()
        self.run_installer()
        self.assertTrue((shared/'rune-kit').is_file())
        self.assertEqual((shared/'rune-infer').read_bytes(), b'infer')
        self.assertEqual((shared/'infer/config').read_bytes(), b'configuration')
        self.assertEqual((home/'.profile').read_text().count('export PATH='), 1)
        profile = (home/'.profile').as_posix()
        check = subprocess.run([BASH, '-c', 'source "$1"; source "$1"; printf "%s" "$PATH"', 'check', profile], env=self.env, text=True, capture_output=True)
        self.assertEqual(check.returncode, 0, check.stderr)
        expected = subprocess.check_output([BASH, "-c", 'printf "%s" "$HOME/rune"'], env=self.env, text=True)
        self.assertEqual(check.stdout.count(expected), 1)

    @unittest.skipUnless(BASH, 'POSIX shell or Git Bash required')
    def test_posix_install_reinstall_and_quoted_path_hint(self):
        self.package(self.tar, f'rune-kit-{TAG}-x86_64-unknown-linux-gnu.tar.gz')
        result = self.run_installer()
        self.run_installer()
        self.assertTrue((self.install/'rune-kit').is_file())
        self.assertFalse((self.install/'rune').exists())
        hint = next(line.strip() for line in result.stdout.splitlines() if line.strip().startswith('export PATH='))
        # Execute the printed command in a disposable subprocess: apostrophes/spaces survive.
        checked = subprocess.run([BASH, '-c', hint+'; printf "%s" "$PATH"'], text=True, capture_output=True)
        self.assertEqual(checked.returncode, 0, checked.stderr)
        self.assertTrue(checked.stdout.startswith(self.install.as_posix()+':'), checked.stdout)

    @unittest.skipUnless(BASH, 'POSIX shell or Git Bash required')
    def test_posix_bad_checksum_preserves_existing_install(self):
        target = self.package(self.tar, f'rune-kit-{TAG}-x86_64-unknown-linux-gnu.tar.gz')
        target.with_name(target.name+'.sha256').write_text('0'*64, encoding='ascii')
        self.install.mkdir()
        (self.install/'rune-kit').write_bytes(b'previous installation')
        self.run_installer(success=False)
        self.assertEqual((self.install/'rune-kit').read_bytes(), b'previous installation')

    @unittest.skipUnless(BASH, 'POSIX shell or Git Bash required')
    def test_posix_missing_checksum_does_not_install(self):
        target = self.package(self.tar, f'rune-kit-{TAG}-x86_64-unknown-linux-gnu.tar.gz')
        target.with_name(target.name+'.sha256').unlink()
        self.run_installer(success=False)
        self.assertFalse(self.install.exists())


if __name__ == '__main__':
    unittest.main(argv=[__file__, *remaining], verbosity=2)
