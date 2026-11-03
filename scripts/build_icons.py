#!/usr/bin/env python3
"""从 ReqSucc 图标源文件生成 macOS 多分辨率 ICNS。"""
from pathlib import Path
import subprocess
import tempfile

root = Path(__file__).resolve().parent.parent
source = root / 'assets' / 'reqsucc-icon.png'
with tempfile.TemporaryDirectory(prefix='reqsucc-icon-') as temp:
    iconset = Path(temp) / 'ReqSucc.iconset'
    iconset.mkdir()
    for points in [16, 32, 128, 256, 512]:
        for density in [1, 2]:
            size = points * density
            suffix = '@2x' if density == 2 else ''
            output = iconset / f'icon_{points}x{points}{suffix}.png'
            subprocess.run(['sips', '-z', str(size), str(size), str(source), '--out', str(output)], check=True, stdout=subprocess.DEVNULL)
    subprocess.run(['iconutil', '-c', 'icns', str(iconset), '-o', str(root / 'assets' / 'ReqSucc.icns')], check=True)
print(root / 'assets' / 'ReqSucc.icns')
