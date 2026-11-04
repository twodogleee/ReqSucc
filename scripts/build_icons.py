#!/usr/bin/env python3
"""从项目 PNG 图标生成 macOS 多分辨率 ICNS，输出覆盖 assets/ReqSucc.icns。

依赖 macOS 的 sips 缩放工具，中间 PNG 存放于自动清理的临时目录。
作者：Andersen
"""
from pathlib import Path
import struct
import subprocess
import tempfile

# 从脚本位置定位仓库根目录，不依赖执行命令时的工作目录。
root = Path(__file__).resolve().parent.parent
# 项目维护的应用图标源文件，所有分辨率均由此生成。
source = root / 'assets' / 'reqsucc-icon.png'
with tempfile.TemporaryDirectory(prefix='reqsucc-icon-') as temp:
    iconset = Path(temp) / 'ReqSucc.iconset'
    iconset.mkdir()
    # points 为逻辑尺寸，density 为像素密度；同时生成标准和 Retina 图标。
    for points in [16, 32, 128, 256, 512]:
        for density in [1, 2]:
            size = points * density
            suffix = '@2x' if density == 2 else ''
            output = iconset / f'icon_{points}x{points}{suffix}.png'
            subprocess.run(['sips', '-z', str(size), str(size), str(source), '--out', str(output)], check=True, stdout=subprocess.DEVNULL)
    # ICNS 类型码对应具体尺寸和密度，直接嵌入 PNG 以保留标准与 Retina 图像。
    entries = [
        ('icp4', '16x16'), ('ic11', '16x16@2x'),
        ('icp5', '32x32'), ('ic12', '32x32@2x'),
        ('ic07', '128x128'), ('ic13', '128x128@2x'),
        ('ic08', '256x256'), ('ic14', '256x256@2x'),
        ('ic09', '512x512'), ('ic10', '512x512@2x'),
    ]
    # 每个条目由四字节类型码、包含八字节条目头的总长度及 PNG 数据组成。
    chunks = []
    for kind, size in entries:
        data = (iconset / f'icon_{size}.png').read_bytes()
        chunks.append(kind.encode('ascii') + struct.pack('>I', len(data) + 8) + data)
    # 文件头使用大端总长度，包含 icns 标识及长度字段自身的八个字节。
    body = b''.join(chunks)
    (root / 'assets' / 'ReqSucc.icns').write_bytes(
        b'icns' + struct.pack('>I', len(body) + 8) + body
    )
print(root / 'assets' / 'ReqSucc.icns')
