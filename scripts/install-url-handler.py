#!/usr/bin/env python3
"""Build/install the local macOS URL handler, preserving unrelated applications."""
import argparse
import pathlib
import plistlib
import shutil
import subprocess
import tempfile

parser = argparse.ArgumentParser(description=__doc__)
parser.add_argument('--destination', default=str(pathlib.Path.home()/'Applications/Orca Bridge.app'))
args = parser.parse_args()
root = pathlib.Path(__file__).resolve().parent.parent
app = pathlib.Path(args.destination).expanduser().absolute()
identifier = 'dev.joel.orca-bridge.url'
if app.exists():
    with (app/'Contents/Info.plist').open('rb') as f:
        assert plistlib.load(f).get('CFBundleIdentifier') == identifier, 'Destination belongs to another application'
app.parent.mkdir(parents=True, exist_ok=True)
with tempfile.TemporaryDirectory(prefix='.orca-bridge-', dir=app.parent) as temporary:
    bundle = pathlib.Path(temporary)/app.name
    macos = bundle/'Contents/MacOS'
    resources = bundle/'Contents/Resources'
    macos.mkdir(parents=True)
    resources.mkdir()
    subprocess.run(['xcrun','swiftc','-O',str(root/'integrations/macos/main.swift'),'-o',str(macos/'OrcaBridge')], check=True)
    shutil.copy2(root/'target/release/orca-bridge',resources/'orca-bridge')
    with (bundle/'Contents/Info.plist').open('wb') as f:
        plistlib.dump({'CFBundleIdentifier':identifier,'CFBundleName':'Orca Bridge',
                      'CFBundleExecutable':'OrcaBridge','CFBundlePackageType':'APPL',
                      'CFBundleVersion':'1','CFBundleShortVersionString':'0.1.0','LSUIElement':True,
                      'CFBundleURLTypes':[{'CFBundleURLName':'Orca Bridge session','CFBundleURLSchemes':['orca-bridge']}],
                      'NSHighResolutionCapable':True},f)
    subprocess.run(['codesign','--force','--sign','-',str(bundle)],check=True,capture_output=True)
    previous = pathlib.Path(temporary)/'previous.app'
    if app.exists():
        app.rename(previous)
    try:
        bundle.rename(app)
    except BaseException:
        if previous.exists(): previous.rename(app)
        raise
subprocess.run(['/System/Library/Frameworks/CoreServices.framework/Frameworks/LaunchServices.framework/Support/lsregister','-f',str(app)],check=True)
subprocess.run([str(app/'Contents/MacOS/OrcaBridge'),'--check-handler'],check=True)
print('Installed and verified: '+str(app))
