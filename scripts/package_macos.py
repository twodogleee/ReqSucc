#!/usr/bin/env python3
"""构建并临时签名原生 macOS 应用包，输出到 target 对应构建模式的目录。

默认先执行锁定依赖的 Cargo 构建；--skip-build 复用已有二进制，
--preview 使用独立应用名称和标识。临时签名不代表分发签名或公证。
作者：Andersen
"""
import argparse
import hashlib
import os
import pathlib
import plistlib
import shutil
import subprocess
from release import MACOS_MIN_VERSION, project_version, verify_binary

# 打包开关分别控制构建模式、二进制复用和独立验收应用身份。
parser=argparse.ArgumentParser()
parser.add_argument('--release',action='store_true')
parser.add_argument('--skip-build',action='store_true')
parser.add_argument('--preview',action='store_true',help='使用独立验收应用标识')
parser.add_argument('--binary-dir',type=pathlib.Path,help='已构建二进制目录，仅可与 --skip-build 一起使用')
args=parser.parse_args()
if args.binary_dir and not args.skip_build:
    parser.error('--binary-dir 必须与 --skip-build 一起使用')
# 从脚本位置定位仓库，确保从其他目录执行时仍能找到构建和资源路径。
root=pathlib.Path(__file__).resolve().parent.parent
# Cargo 输出目录名，须与实际构建模式一致。
profile='release' if args.release else 'debug'
# 自定义输出目录用于 Actions 的显式目标构建，默认保留本地打包路径。
binary_dir=args.binary_dir.resolve() if args.binary_dir else root/'target'/profile
if not args.skip_build:
    # 构建和包元数据使用同一部署目标，包含 Cargo 调用的原生依赖构建。
    build_env=os.environ.copy()
    build_env['MACOSX_DEPLOYMENT_TARGET']=MACOS_MIN_VERSION
    subprocess.run(['cargo','build','--locked']+(['--release'] if args.release else []),cwd=root,env=build_env,check=True)
# 即使复用现有二进制，也须阻止架构或最低版本与包声明不一致。
verify_binary(binary_dir/'reqsucc','macos-arm64')
# 验收应用使用独立名称和包标识，避免与常规应用混淆。
name='ReqSucc 验收' if args.preview else 'ReqSucc'
# 应用包及其标准 Contents 目录，重复打包时更新同一目标。
bundle=binary_dir/(name+'.app')
contents=bundle/'Contents'
(contents/'MacOS').mkdir(parents=True,exist_ok=True)
(contents/'Resources'/'zh_CN.lproj').mkdir(parents=True,exist_ok=True)
shutil.copy2(binary_dir/'reqsucc',contents/'MacOS'/'reqsucc')
# 图标文件名包含内容摘要，避免 macOS 继续复用旧图标缓存。
icon_source=root/'assets'/'ReqSucc.icns'
icon_name='ReqSucc-'+hashlib.sha256(icon_source.read_bytes()).hexdigest()[:12]+'.icns'
shutil.copy2(icon_source,contents/'Resources'/icon_name)
for old_icon in (contents/'Resources').glob('ReqSucc*.icns'):
    if old_icon.name != icon_name:
        old_icon.unlink()
# 读取既有验收包配置，以保留手动设置的独立验收环境变量。
previous_info={}
if (contents/'Info.plist').exists():
    with (contents/'Info.plist').open('rb') as f:
        previous_info=plistlib.load(f)
# 原生应用元数据；发布版本变更时应与 Cargo.toml 和 README 中的业务版本核对。
version=project_version()
info={'CFBundleName':name,'CFBundleDisplayName':name,'CFBundleIdentifier':'com.reqsucc.native-preview' if args.preview else 'com.reqsucc.desktop','CFBundleExecutable':'reqsucc','CFBundlePackageType':'APPL','CFBundleShortVersionString':version.split('-')[0],'CFBundleVersion':version.split('-')[0],'CFBundleIconFile':icon_name,'CFBundleDevelopmentRegion':'zh_CN','CFBundleLocalizations':['zh_CN'],'LSMinimumSystemVersion':MACOS_MIN_VERSION,'NSHighResolutionCapable':True,'NSPrincipalClass':'NSApplication','NSSupportsAutomaticGraphicsSwitching':True}
if args.preview and previous_info.get('LSEnvironment'):
    info['LSEnvironment']=previous_info['LSEnvironment']
with (contents/'Info.plist').open('wb') as f:plistlib.dump(info,f)
# 使用本机临时签名供本地运行，不配置 Developer ID 或执行公证。
subprocess.run(['codesign','--force','--deep','--sign','-',str(bundle)],check=True)
subprocess.run(['codesign','--verify','--deep','--strict',str(bundle)],check=True)
# 更新 Contents 内文件不会改变 .app 目录时间，因此显式更新时间使系统感知包已更新。
os.utime(bundle, None)
print(bundle)
