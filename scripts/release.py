#!/usr/bin/env python3
"""校验发布版本、打包平台成品并汇总 SHA-256；不执行 Git 推送或远程发布。

作者：Andersen
"""
import argparse
import hashlib
import json
import os
from pathlib import Path
import shutil
import struct
import subprocess
import sys
import tempfile
import zipfile

try:
    import tomllib
except ImportError:
    # macOS 系统 Python 3.9 没有 TOML 解析器，可复用本机 Cargo。
    tomllib = None

# 所有输入均相对仓库定位，发布产物统一放在被 Git 忽略的 target/dist。
ROOT = Path(__file__).resolve().parent.parent
# 所有 macOS 成品使用一致的最低系统版本；版本编码遵循 Mach-O 的 X.Y.Z 格式。
MACOS_MIN_VERSION = '13.0'
MACOS_MIN_VERSION_ENCODED = 13 << 16


def verify_macos_binary(stream):
    """校验 ARM64 可执行文件及其 Mach-O 最低系统版本，拒绝未知或不一致的输入。"""
    header = stream.read(32)
    if len(header) != 32:
        raise ValueError('macOS 打包输入不是完整的 arm64 Mach-O 可执行文件')
    magic, cpu, _, filetype, count, commands_size, _, _ = struct.unpack('<IiiIIIII', header)
    if magic != 0xFEEDFACF or cpu != 0x0100000C or filetype != 2:
        raise ValueError('macOS 打包输入必须是 arm64 Mach-O 可执行文件')
    # 仅解析载入命令，限制读取长度并检查文件边界，避免损坏文件导致虚假通过。
    stream.seek(0, os.SEEK_END)
    if commands_size > 16 * 1024 * 1024 or commands_size > stream.tell() - 32:
        raise ValueError('macOS 可执行文件的载入命令不完整')
    stream.seek(32)
    commands = stream.read(commands_size)
    offset = 0
    versions = []
    for _ in range(count):
        if offset + 8 > len(commands):
            raise ValueError('macOS 可执行文件的载入命令不完整')
        command, length = struct.unpack_from('<II', commands, offset)
        if length < 8 or length % 8 or offset + length > len(commands):
            raise ValueError('macOS 可执行文件的载入命令无效')
        if command == 0x32:  # LC_BUILD_VERSION：platform 1 表示 macOS。
            if length < 24:
                raise ValueError('macOS 可执行文件的版本信息不完整')
            platform, minimum = struct.unpack_from('<II', commands, offset + 8)
            if platform != 1:
                raise ValueError('macOS 可执行文件的平台版本信息不匹配')
            versions.append(minimum)
        elif command == 0x24:  # 旧版 LC_VERSION_MIN_MACOSX。
            if length < 16:
                raise ValueError('macOS 可执行文件的版本信息不完整')
            versions.append(struct.unpack_from('<I', commands, offset + 8)[0])
        offset += length
    if offset != len(commands) or versions != [MACOS_MIN_VERSION_ENCODED]:
        raise ValueError(f'macOS 可执行文件的最低部署版本必须为 {MACOS_MIN_VERSION}；请重新构建')


def project_version():
    """读取 Cargo 包版本；Python 3.11 以前复用 Cargo，无需额外 Python 依赖。"""
    if tomllib is not None:
        return tomllib.loads((ROOT / 'Cargo.toml').read_text(encoding='utf-8'))['package']['version']
    metadata = json.loads(subprocess.check_output(
        ['cargo', 'metadata', '--locked', '--offline', '--no-deps', '--format-version', '1'],
        cwd=ROOT, text=True,
    ))
    return next(package['version'] for package in metadata['packages']
                if package['name'] == 'reqsucc')


def check_tag(version, tag):
    """阻止标签与实际应用版本不一致的发布，空标签用于日常构建。"""
    if tag and tag != f'v{version}':
        raise ValueError(f'标签 {tag} 与 Cargo.toml 版本 v{version} 不一致')


def verify_binary(binary, platform):
    """校验可执行文件架构及 macOS 部署版本，防止发布错误的平台成品。"""
    with binary.open('rb') as stream:
        if platform == 'windows-x64':
            header = stream.read(64)
            if len(header) != 64 or header[:2] != b'MZ':
                raise ValueError('Windows 打包输入不是 PE 可执行文件')
            stream.seek(struct.unpack_from('<I', header, 60)[0])
            pe = stream.read(6)
            if pe != b'PE\0\0\x64\x86':
                raise ValueError('Windows 打包输入必须是 x64 可执行文件')
        elif platform == 'macos-arm64':
            verify_macos_binary(stream)
        else:
            raise ValueError(f'不支持的打包平台：{platform}')


def windows_runtime():
    """查找 MSVC x64 可再分发运行库；缺失时拒绝生成依赖不完整的便携包。"""
    redist = os.environ.get('VCToolsRedistDir')
    if not redist:
        raise ValueError('请在 Visual Studio x64 开发环境中打包，缺少 VCToolsRedistDir')
    candidates = sorted((Path(redist) / 'x64').glob('Microsoft.VC*.CRT'))
    if not candidates:
        raise ValueError('未找到 MSVC x64 CRT 可再分发目录')
    runtime = candidates[-1]
    for name in ['msvcp140.dll', 'vcruntime140.dll', 'vcruntime140_1.dll']:
        if not (runtime / name).is_file():
            raise ValueError(f'MSVC 运行库缺少 {name}')
    return runtime


def package(platform, version, binary_dir, output_dir):
    """校验并打包已编译的正式程序；不隐式构建，也不读取用户数据。"""
    binary = binary_dir / ('reqsucc.exe' if platform == 'windows-x64' else 'reqsucc')
    verify_binary(binary, platform)
    output_dir.mkdir(parents=True, exist_ok=True)
    name = f'ReqSucc-v{version}-{platform}'
    archive = output_dir / f'{name}.zip'
    # 临时目录退出时自动清理；归档完成后替换成品，避免留下半个下载包。
    with tempfile.TemporaryDirectory(prefix='reqsucc-release-') as temporary:
        staging = Path(temporary) / name
        staging.mkdir()
        for document in ['LICENSE', 'NOTICE', 'README.md']:
            shutil.copy2(ROOT / document, staging / document)
        # NOTICE 与 README 引用的第三方许可和补丁说明必须随分发包提供。
        for document in [
            'vendor/gpui-pre-macos/LICENSE-APACHE',
            'vendor/gpui-pre-macos/PATCHES.md',
            'vendor/core-text/COPYRIGHT',
            'vendor/core-text/LICENSE-APACHE',
            'vendor/core-text/LICENSE-MIT',
            'vendor/core-text/PATCHES.md',
        ]:
            destination = staging / document
            destination.parent.mkdir(parents=True, exist_ok=True)
            shutil.copy2(ROOT / document, destination)
        if platform == 'windows-x64':
            shutil.copy2(binary, staging / 'ReqSucc.exe')
            for library in windows_runtime().glob('*.dll'):
                shutil.copy2(library, staging / library.name)
            with zipfile.ZipFile(archive.with_suffix('.tmp'), 'w', zipfile.ZIP_DEFLATED) as bundle:
                for path in sorted(staging.rglob('*')):
                    if path.is_file():
                        bundle.write(path, f'{name}/{path.relative_to(staging).as_posix()}')
        else:
            subprocess.run([
                sys.executable, str(ROOT / 'scripts/package_macos.py'),
                '--release', '--skip-build', '--binary-dir', str(binary_dir),
            ], check=True)
            shutil.copytree(binary_dir / 'ReqSucc.app', staging / 'ReqSucc.app', symlinks=True)
            # ditto 保留 .app 的执行权限与资源属性，便于解压后直接启动。
            subprocess.run(['ditto', '-c', '-k', '--sequesterRsrc', '--keepParent',
                            str(staging), str(archive.with_suffix('.tmp'))], check=True)
        archive.with_suffix('.tmp').replace(archive)
    return archive


def write_checksums(directory, version):
    """仅在两个预期平台成品齐全时生成校验清单，阻止不完整版本发布。"""
    expected = [f'ReqSucc-v{version}-{platform}.zip'
                for platform in ['macos-arm64', 'windows-x64']]
    actual = sorted(path.name for path in directory.glob('*.zip'))
    if actual != expected:
        raise ValueError(f'发布包不完整或包含其他版本：预期 {expected}，实际 {actual}')
    lines = []
    for name in expected:
        digest = hashlib.sha256()
        with (directory / name).open('rb') as stream:
            for chunk in iter(lambda: stream.read(1024 * 1024), b''):
                digest.update(chunk)
        lines.append(f'{digest.hexdigest()}  {name}\n')
    (directory / 'SHA256SUMS.txt').write_text(''.join(lines), encoding='utf-8')


def main():
    """提供 Actions 和本地打包共用的版本、归档与校验清单命令。"""
    parser = argparse.ArgumentParser(description=__doc__)
    commands = parser.add_subparsers(dest='command', required=True)
    metadata = commands.add_parser('metadata')
    metadata.add_argument('--tag', default='')
    metadata.add_argument('--github-output', type=Path)
    archive = commands.add_parser('package')
    archive.add_argument('--platform', choices=['windows-x64', 'macos-arm64'], required=True)
    archive.add_argument('--binary-dir', type=Path, default=ROOT / 'target/release')
    archive.add_argument('--output-dir', type=Path, default=ROOT / 'target/dist')
    checksums = commands.add_parser('checksums')
    checksums.add_argument('--directory', type=Path, default=ROOT / 'target/dist')
    args = parser.parse_args()
    version = project_version()
    if args.command == 'metadata':
        check_tag(version, args.tag)
        if args.github_output:
            with args.github_output.open('a', encoding='utf-8') as output:
                output.write(f'version={version}\n')
        print(version)
    elif args.command == 'package':
        print(package(args.platform, version, args.binary_dir.resolve(), args.output_dir.resolve()))
    else:
        write_checksums(args.directory, version)


if __name__ == '__main__':
    main()
