"""验证发布标签、平台归档和校验清单的拒绝边界，不启动应用或访问用户数据。"""
import hashlib
import os
from pathlib import Path
import struct
import tempfile
import unittest
from unittest.mock import patch
import zipfile

import release


class ReleaseTests(unittest.TestCase):
    """覆盖错误版本、错误架构和不完整分发等发布风险。"""

    def test_tag_must_match_package_version(self):
        """允许无标签检查与精确版本标签，拒绝错误版本和缺少 v 前缀。"""
        release.check_tag('0.4.0', '')
        release.check_tag('0.4.0', 'v0.4.0')
        release.check_tag('0.4.1-rc.1', 'v0.4.1-rc.1')
        for tag in ['v0.4.1', '0.4.0', 'v0.4.0/invalid']:
            with self.assertRaises(ValueError):
                release.check_tag('0.4.0', tag)

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
                    names = {Path(name).name for name in bundle.namelist()}
                    self.assertEqual(names, {'ReqSucc.exe', 'LICENSE', 'NOTICE', 'README.md',
                                             'msvcp140.dll', 'vcruntime140.dll', 'vcruntime140_1.dll'})
                    self.assertEqual(bundle.read('ReqSucc-v0.4.0-windows-x64/ReqSucc.exe'), binary.read_bytes())
                (runtime / 'vcruntime140.dll').unlink()
                with self.assertRaises(ValueError):
                    release.windows_runtime()
            binary.write_bytes(header + b'PE\0\0\x4c\x01')
            with self.assertRaises(ValueError):
                release.verify_binary(binary, 'windows-x64')
            with self.assertRaises(ValueError):
                release.verify_binary(binary, 'macos-arm64')

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
