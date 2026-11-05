"""验证发布标签、平台归档和校验清单的拒绝边界，不启动应用或访问用户数据。"""
import hashlib
import os
from pathlib import Path
import struct
import subprocess
import sys
import tempfile
import unittest
from unittest.mock import patch
import zipfile

import release


class ReleaseTests(unittest.TestCase):
    """覆盖错误版本、错误架构和不完整分发等发布风险。"""

    def assert_vendor_documents(self, bundle, prefix):
        """核对归档中与 NOTICE 引用一致的上游许可证和补丁说明。"""
        for document in [
            'vendor/gpui-pre-macos/LICENSE-APACHE',
            'vendor/gpui-pre-macos/PATCHES.md',
            'vendor/core-text/COPYRIGHT',
            'vendor/core-text/LICENSE-APACHE',
            'vendor/core-text/LICENSE-MIT',
            'vendor/core-text/PATCHES.md',
        ]:
            with self.subTest(document=document):
                self.assertEqual(bundle.read(f'{prefix}/{document}'),
                                 (release.ROOT / document).read_bytes())

    def test_tag_must_match_package_version(self):
        """允许无标签检查与精确版本标签，拒绝错误版本和缺少 v 前缀。"""
        release.check_tag('0.4.0', '')
        release.check_tag('0.4.0', 'v0.4.0')
        release.check_tag('0.4.1-rc.1', 'v0.4.1-rc.1')
        for tag in ['v0.4.1', '0.4.0', 'v0.4.0/invalid']:
            with self.assertRaises(ValueError):
                release.check_tag('0.4.0', tag)

    def test_macos_binary_requires_arm64_and_deployment_target_13(self):
        """允许 macOS 13 ARM64 可执行文件，拒绝错误架构、版本与损坏的载入命令。"""
        def macho(cpu, commands, count=1, filetype=2):
            """生成仅含 Mach-O 头和指定载入命令的隔离测试输入。"""
            return struct.pack('<IiiIIIII', 0xFEEDFACF, cpu, 0, filetype, count,
                               len(commands), 0, 0) + commands

        def build_version(platform, version):
            """生成不含工具条目的 LC_BUILD_VERSION。"""
            return struct.pack('<IIIIII', 0x32, 24, platform, version, 0, 0)

        arm64 = 0x0100000C
        good = build_version(1, 13 << 16)
        legacy = struct.pack('<IIII', 0x24, 16, 13 << 16, 0)
        with tempfile.TemporaryDirectory() as temporary:
            binary = Path(temporary) / 'reqsucc'
            for command in [good, legacy]:
                binary.write_bytes(macho(arm64, command))
                release.verify_binary(binary, 'macos-arm64')
            for fixture in [
                macho(0x01000007, good),  # Intel
                macho(arm64, good, filetype=6),  # 动态库
                macho(arm64, build_version(1, 11 << 16)),
                macho(arm64, build_version(1, (13 << 16) | (1 << 8))),
                macho(arm64, build_version(1, 15 << 16)),
                macho(arm64, build_version(2, 13 << 16)),  # iOS
                macho(arm64, b'', count=0),
                macho(arm64, struct.pack('<II', 0x32, 24)),
                macho(arm64, good + good, count=2),
                b'\xcf\xfa\xed\xfe\x0c\x00\x00\x01',
            ]:
                binary.write_bytes(fixture)
                with self.assertRaises(ValueError):
                    release.verify_binary(binary, 'macos-arm64')

    def test_macos_skip_build_rejects_wrong_version_before_writing_bundle(self):
        """复用旧二进制时，最低版本不符会在创建或覆盖应用包之前失败。"""
        with tempfile.TemporaryDirectory() as temporary:
            directory = Path(temporary)
            command = struct.pack('<IIIIII', 0x32, 24, 1, 15 << 16, 0, 0)
            header = struct.pack('<IiiIIIII', 0xFEEDFACF, 0x0100000C, 0, 2, 1,
                                 len(command), 0, 0)
            (directory / 'reqsucc').write_bytes(header + command)
            result = subprocess.run(
                [sys.executable, str(release.ROOT / 'scripts/package_macos.py'),
                 '--skip-build', '--binary-dir', str(directory)],
                capture_output=True, text=True,
            )
            self.assertNotEqual(result.returncode, 0)
            self.assertIn('最低部署版本必须为 13.0', result.stderr)
            self.assertFalse((directory / 'ReqSucc.app').exists())

    def test_windows_archive_includes_runtime_and_documents(self):
        """归档包含应用、运行库与许可文档，并拒绝将 x86 程序标为 x64。"""
        with tempfile.TemporaryDirectory() as temporary:
            root = Path(temporary)
            binary_dir = root / 'release'
            binary_dir.mkdir()
            header = bytearray(64)
            header[:2] = b'MZ'
            struct.pack_into('<I', header, 60, 64)
            binary = binary_dir / 'reqsucc.exe'
            binary.write_bytes(header + b'PE\0\0\x64\x86')
            runtime = root / 'redist/x64/Microsoft.VC143.CRT'
            runtime.mkdir(parents=True)
            for name in ['msvcp140.dll', 'vcruntime140.dll', 'vcruntime140_1.dll']:
                (runtime / name).write_bytes(b'fixture-runtime')
            with patch.dict(os.environ, {'VCToolsRedistDir': str(root / 'redist')}):
                archive = release.package('windows-x64', '0.4.0', binary_dir, root / 'dist')
                with zipfile.ZipFile(archive) as bundle:
                    names = {Path(name).name for name in bundle.namelist()
                             if '/vendor/' not in name}
                    self.assertEqual(names, {'ReqSucc.exe', 'LICENSE', 'NOTICE', 'README.md',
                                             'msvcp140.dll', 'vcruntime140.dll', 'vcruntime140_1.dll'})
                    self.assertEqual(bundle.read('ReqSucc-v0.4.0-windows-x64/ReqSucc.exe'), binary.read_bytes())
                    self.assert_vendor_documents(bundle, 'ReqSucc-v0.4.0-windows-x64')
                (runtime / 'vcruntime140.dll').unlink()
                with self.assertRaises(ValueError):
                    release.windows_runtime()
            binary.write_bytes(header + b'PE\0\0\x4c\x01')
            with self.assertRaises(ValueError):
                release.verify_binary(binary, 'windows-x64')
            with self.assertRaises(ValueError):
                release.verify_binary(binary, 'macos-arm64')

    def test_macos_archive_includes_vendor_documents(self):
        """macOS 打包暂存与归档均包含 NOTICE 引用的上游许可和补丁说明。"""
        with tempfile.TemporaryDirectory() as temporary:
            root = Path(temporary)
            binary_dir = root / 'release'
            binary_dir.mkdir()
            command = struct.pack('<IIIIII', 0x32, 24, 1, 13 << 16, 0, 0)
            header = struct.pack('<IiiIIIII', 0xFEEDFACF, 0x0100000C, 0, 2, 1,
                                 len(command), 0, 0)
            (binary_dir / 'reqsucc').write_bytes(header + command)

            def fake_run(command, **kwargs):
                """模拟应用包和 ditto 归档，验证真实打包函数提供的暂存内容。"""
                if command[0] == sys.executable:
                    app = binary_dir / 'ReqSucc.app/Contents/MacOS/reqsucc'
                    app.parent.mkdir(parents=True)
                    app.write_bytes(b'macos-fixture')
                elif command[0] == 'ditto':
                    staging, target = Path(command[-2]), Path(command[-1])
                    with zipfile.ZipFile(target, 'w') as bundle:
                        for path in staging.rglob('*'):
                            if path.is_file():
                                bundle.write(path, f'{staging.name}/{path.relative_to(staging).as_posix()}')
                else:
                    self.fail(f'意外的打包命令：{command}')

            with patch.object(release.subprocess, 'run', side_effect=fake_run):
                archive = release.package('macos-arm64', '0.4.0', binary_dir, root / 'dist')
            with zipfile.ZipFile(archive) as bundle:
                self.assertEqual(bundle.read('ReqSucc-v0.4.0-macos-arm64/ReqSucc.app/Contents/MacOS/reqsucc'),
                                 b'macos-fixture')
                self.assert_vendor_documents(bundle, 'ReqSucc-v0.4.0-macos-arm64')

    def test_checksums_require_both_platforms(self):
        """只有同版本的完整双平台成品可生成校验清单，内容与真实文件摘要一致。"""
        with tempfile.TemporaryDirectory() as temporary:
            root = Path(temporary)
            windows = root / 'ReqSucc-v0.4.0-windows-x64.zip'
            windows.write_bytes(b'windows-fixture')
            with self.assertRaises(ValueError):
                release.write_checksums(root, '0.4.0')
            macos = root / 'ReqSucc-v0.4.0-macos-arm64.zip'
            macos.write_bytes(b'macos-fixture')
            release.write_checksums(root, '0.4.0')
            expected = ''.join(f'{hashlib.sha256(path.read_bytes()).hexdigest()}  {path.name}\n'
                               for path in [macos, windows])
            self.assertEqual((root / 'SHA256SUMS.txt').read_text(), expected)
            (root / 'ReqSucc-v0.3.0-windows-x64.zip').write_bytes(b'old-version')
            with self.assertRaises(ValueError):
                release.write_checksums(root, '0.4.0')


if __name__ == '__main__':
    unittest.main()
