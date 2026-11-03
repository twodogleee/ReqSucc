#!/usr/bin/env python3
"""构建原生 macOS 应用包，不依赖 Node.js、Tauri 或 WebView。"""
import argparse
import pathlib
import plistlib
import shutil
import subprocess

parser=argparse.ArgumentParser()
parser.add_argument('--release',action='store_true')
parser.add_argument('--skip-build',action='store_true')
parser.add_argument('--preview',action='store_true',help='使用独立验收应用标识')
args=parser.parse_args()
root=pathlib.Path(__file__).resolve().parent.parent
profile='release' if args.release else 'debug'
if not args.skip_build:
    subprocess.run(['cargo','build','--locked']+(['--release'] if args.release else []),cwd=root,check=True)
name='ReqSucc 验收' if args.preview else 'ReqSucc'
bundle=root/'target'/profile/(name+'.app')
contents=bundle/'Contents'
(contents/'MacOS').mkdir(parents=True,exist_ok=True)
(contents/'Resources'/'zh_CN.lproj').mkdir(parents=True,exist_ok=True)
shutil.copy2(root/'target'/profile/'reqsucc',contents/'MacOS'/'reqsucc')
shutil.copy2(root/'assets'/'ReqSucc.icns',contents/'Resources'/'ReqSucc.icns')
info={'CFBundleName':name,'CFBundleDisplayName':name,'CFBundleIdentifier':'com.reqsucc.native-preview' if args.preview else 'com.reqsucc.desktop','CFBundleExecutable':'reqsucc','CFBundlePackageType':'APPL','CFBundleShortVersionString':'0.4.0','CFBundleVersion':'4','CFBundleIconFile':'ReqSucc.icns','CFBundleDevelopmentRegion':'zh_CN','CFBundleLocalizations':['zh_CN'],'LSMinimumSystemVersion':'15.0','NSHighResolutionCapable':True,'NSPrincipalClass':'NSApplication','NSSupportsAutomaticGraphicsSwitching':True}
with (contents/'Info.plist').open('wb') as f:plistlib.dump(info,f)
subprocess.run(['codesign','--force','--deep','--sign','-',str(bundle)],check=True)
print(bundle)
