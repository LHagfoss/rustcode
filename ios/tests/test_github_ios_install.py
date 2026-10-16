#!/usr/bin/env python3
"""Exercise the published-release installer without signing keys or device mutations."""
import datetime
import hashlib
import json
import os
from pathlib import Path
import plistlib
import shlex
import shutil
import subprocess
import sys
import tempfile
import unittest
import zipfile

ROOT = Path(__file__).resolve().parents[1]

FAKE = r'''
import json,os,sys,plistlib,shutil,hashlib
from pathlib import Path
command=Path(sys.argv[1]).name
args=sys.argv[2:]
with open(os.environ['FAKE_CALLS'],'a') as f: f.write(json.dumps([command,args])+'\n')
if command=='gh':
    if os.environ.get('FAKE_GH_FAIL'): sys.exit(22)
    if args[:2]==['release','view']:
        print(Path(os.environ['FAKE_RELEASE']).read_text())
    elif args[:2]==['release','download']:
        dest=Path(args[args.index('--dir')+1]);dest.mkdir(exist_ok=True)
        for p in Path(os.environ['FAKE_ASSETS']).iterdir(): shutil.copy2(p,dest/p.name)
    else: sys.exit(90)
elif command=='security':
    if args[0]=='find-identity':
        if not os.environ.get('FAKE_NO_IDENTITY'):
            print('  1) '+hashlib.sha1(b'fixture certificate').hexdigest().upper()+' "Apple Development: Fixture"')
    elif args[:3]==['cms','-D','-i']: sys.stdout.buffer.write(Path(args[3]).read_bytes())
    else: sys.exit(91)
elif command=='codesign':
    if os.environ.get('FAKE_CODESIGN_FAIL'): sys.exit(14)
    if '--sign' in args:
        app=Path(args[-1]);signature=app/'_CodeSignature';signature.mkdir(exist_ok=True)
        if '--entitlements' in args:
            shutil.copy2(args[args.index('--entitlements')+1],signature/'entitlements.plist')
        (signature/'CodeResources').write_text('signed fixture')
    elif '--verify' in args:
        if not (Path(args[-1])/'_CodeSignature/CodeResources').is_file(): sys.exit(15)
    else: sys.exit(92)
elif command=='xcrun':
    if args[:3]==['devicectl','list','devices']:
        Path(args[args.index('--json-output')+1]).write_text(Path(os.environ['FAKE_DEVICES']).read_text())
    elif args[:4]==['devicectl','device','install','app']:
        if os.environ.get('FAKE_INSTALL_FAIL'): sys.exit(13)
        app=Path(args[-1])
        if not (app/'embedded.mobileprovision').is_file(): sys.exit(16)
        outcome='failure' if os.environ.get('FAKE_INSTALL_OUTCOME_FAILURE') else 'success'
        Path(args[args.index('--json-output')+1]).write_text(json.dumps({'info':{'outcome':outcome}}))
    elif args[:4]==['devicectl','device','process','launch']: pass
    else: sys.exit(93)
else: sys.exit(94)
'''


class GitHubInstallTests(unittest.TestCase):
    def setUp(self):
        self.temp=tempfile.TemporaryDirectory(prefix='github iOS installer ')
        self.addCleanup(self.temp.cleanup)
        self.root=Path(self.temp.name)/'checkout with spaces'
        self.root.mkdir()
        shutil.copytree(ROOT/'Scripts',self.root/'Scripts')
        self.bin=Path(self.temp.name)/'bin';self.bin.mkdir()
        dispatcher=self.bin/'fake.py';dispatcher.write_text(FAKE)
        for name in ['gh','security','codesign','xcrun','xcodebuild']:
            p=self.bin/name
            p.write_text('#!/bin/sh\nexec '+shlex.quote(sys.executable)+' '+shlex.quote(str(dispatcher))+' "$0" "$@"\n')
            p.chmod(0o755)
        self.calls_file=Path(self.temp.name)/'calls.jsonl'
        self.assets=Path(self.temp.name)/'assets';self.assets.mkdir()
        self.devices=Path(self.temp.name)/'devices.json'
        self.set_devices(['phone-a'])
        self.release=Path(self.temp.name)/'release.json'
        self.release.write_text(json.dumps({'tagName':'v4.5.6','isDraft':False,'isPrerelease':False,
            'assets':[{'name':'rustcode-ios-unsigned.xcarchive.zip'},{'name':'SHA256SUMS'}]}))
        self.info={'CFBundleIdentifier':'com.lhagfoss.rustcode','CFBundleShortVersionString':'4.5.6',
            'CFBundleVersion':'12','CFBundleExecutable':'RustCode','CFBundleSupportedPlatforms':['iPhoneOS']}
        self.make_archive()
        self.profile=self.root/'build/releases/v4.5.5/RustCode.xcarchive/Products/Applications/RustCode.app/embedded.mobileprovision'
        self.profile.parent.mkdir(parents=True)
        self.profile_data={'ExpirationDate':datetime.datetime.now(datetime.timezone.utc).replace(tzinfo=None)+datetime.timedelta(days=5),
            'TeamIdentifier':['ABCDE12345'],'ApplicationIdentifierPrefix':['ABCDE12345'],
            'DeveloperCertificates':[b'fixture certificate'],'ProvisionedDevices':['phone-a-udid','phone-b-udid'],
            'Entitlements':{'application-identifier':'ABCDE12345.com.lhagfoss.rustcode',
                'com.apple.developer.team-identifier':'ABCDE12345','get-task-allow':True,
                'keychain-access-groups':['ABCDE12345.*']}}
        self.save_profile()
        self.env=dict(os.environ,PATH=str(self.bin)+':/usr/bin:/bin',
            FAKE_CALLS=str(self.calls_file),FAKE_ASSETS=str(self.assets),FAKE_RELEASE=str(self.release),FAKE_DEVICES=str(self.devices))

    def save_profile(self): self.profile.write_bytes(plistlib.dumps(self.profile_data))

    def set_devices(self,ids):
        self.devices.write_text(json.dumps({'result':{'devices':[{'identifier':i,'hardwareProperties':{
            'deviceType':'iPhone','reality':'physical','udid':i+'-udid'},'connectionProperties':{
            'tunnelState':'connected','pairingState':'paired'},'deviceProperties':{'name':'Fixture '+i}} for i in ids]}}))

    def make_archive(self,extra=None):
        p=self.assets/'rustcode-ios-unsigned.xcarchive.zip'
        with zipfile.ZipFile(p,'w') as z:
            base='RustCode.xcarchive/Products/Applications/RustCode.app/'
            z.writestr(base+'Info.plist',plistlib.dumps(self.info))
            z.writestr(base+'RustCode',b'fixture CI binary')
            if extra: z.writestr(*extra)
        (self.assets/'SHA256SUMS').write_text(hashlib.sha256(p.read_bytes()).hexdigest()+'  '+p.name+'\n')

    def run_install(self,*args):
        return subprocess.run(['bash',str(self.root/'Scripts/install-on-iphone.sh'),*args],cwd=self.root,env=self.env,text=True,capture_output=True)

    def calls(self,name=None):
        records=[json.loads(line) for line in self.calls_file.read_text().splitlines()] if self.calls_file.exists() else []
        return [args for cmd,args in records if name is None or cmd==name]

    def assert_no_install(self):
        self.assertFalse(any(a[:4]==['devicectl','device','install','app'] for a in self.calls('xcrun')))
        self.assertFalse(any(a[:4]==['devicectl','device','process','launch'] for a in self.calls('xcrun')))

    def test_default_downloads_verifies_signs_installs_and_launches_without_build(self):
        p=self.run_install()
        self.assertEqual(p.returncode,0,p.stderr)
        self.assertEqual(self.calls('xcodebuild'),[])
        install=next(a for a in self.calls('xcrun') if a[:4]==['devicectl','device','install','app'])
        app=Path(install[-1])
        self.assertEqual((app/'RustCode').read_bytes(),b'fixture CI binary')
        self.assertTrue((app/'embedded.mobileprovision').is_file())
        ent=plistlib.loads((app/'_CodeSignature/entitlements.plist').read_bytes())
        self.assertEqual(ent['application-identifier'],'ABCDE12345.com.lhagfoss.rustcode')
        self.assertEqual(ent['keychain-access-groups'],['ABCDE12345.com.lhagfoss.rustcode'])
        self.assertEqual(sum(a[:4]==['devicectl','device','process','launch'] for a in self.calls('xcrun')),1)

    def test_prepare_only_signs_without_device_discovery_or_install(self):
        self.set_devices([])
        p=self.run_install('--prepare-only')
        self.assertEqual(p.returncode,0,p.stderr)
        self.assert_no_install()
        self.assertEqual(self.calls('xcrun'),[])
        self.assertEqual(self.calls('xcodebuild'),[])

    def test_explicit_version_device_and_no_launch(self):
        self.set_devices(['phone-a','phone-b'])
        p=self.run_install('--version','4.5.6','--device','Fixture phone-b','--no-launch')
        self.assertEqual(p.returncode,0,p.stderr)
        self.assertTrue(any('v4.5.6' in a for a in self.calls('gh')))
        install=next(a for a in self.calls('xcrun') if a[:4]==['devicectl','device','install','app'])
        self.assertEqual(install[install.index('--device')+1],'phone-b')
        self.assertFalse(any(a[:4]==['devicectl','device','process','launch'] for a in self.calls('xcrun')))

    def test_ambiguous_device_stops_before_download_or_signing(self):
        self.set_devices(['phone-a','phone-b'])
        p=self.run_install()
        self.assertNotEqual(p.returncode,0)
        self.assertEqual(self.calls('gh'),[])
        self.assertEqual(self.calls('codesign'),[])
        self.assert_no_install()

    def test_corrupt_download_stops_before_signing_and_install(self):
        (self.assets/'rustcode-ios-unsigned.xcarchive.zip').write_bytes(b'corrupt')
        p=self.run_install()
        self.assertNotEqual(p.returncode,0)
        self.assertIn('checksum',p.stderr.lower())
        self.assertEqual(self.calls('codesign'),[])
        self.assert_no_install()

    def test_wrong_version_bundle_and_platform_stop_before_signing(self):
        for key,value in [('CFBundleShortVersionString','1.0.0'),('CFBundleIdentifier','com.example.other'),('CFBundleSupportedPlatforms',['iPhoneSimulator'])]:
            with self.subTest(key=key):
                original=self.info[key];self.info[key]=value;self.make_archive()
                p=self.run_install();self.assertNotEqual(p.returncode,0)
                self.assertEqual(self.calls('codesign'),[]);self.assert_no_install()
                self.info[key]=original

    def test_zip_traversal_is_rejected(self):
        self.make_archive(('../outside',b'bad'))
        p=self.run_install();self.assertNotEqual(p.returncode,0)
        self.assertFalse((self.root/'outside').exists());self.assert_no_install()

    def test_profile_must_be_unexpired_and_include_device(self):
        for key,value in [('ExpirationDate',datetime.datetime(2000,1,1)),('ProvisionedDevices',['other']),
                          ('Entitlements',{'application-identifier':'ABCDE12345.com.example.other'})]:
            with self.subTest(key=key):
                original=self.profile_data[key];self.profile_data[key]=value;self.save_profile()
                p=self.run_install();self.assertNotEqual(p.returncode,0)
                self.assertEqual(self.calls('codesign'),[]);self.assert_no_install()
                self.profile_data[key]=original

    def test_missing_private_signing_identity_stops_before_install(self):
        self.env['FAKE_NO_IDENTITY']='1'
        p=self.run_install();self.assertNotEqual(p.returncode,0);self.assert_no_install()

    def test_signing_failure_never_installs(self):
        self.env['FAKE_CODESIGN_FAIL']='1'
        p=self.run_install();self.assertNotEqual(p.returncode,0);self.assert_no_install()

    def test_install_failure_never_launches(self):
        self.env['FAKE_INSTALL_FAIL']='1'
        p=self.run_install();self.assertNotEqual(p.returncode,0)
        self.assertIn('USB', p.stderr)
        self.assertFalse(any(a[:4]==['devicectl','device','process','launch'] for a in self.calls('xcrun')))

    def test_zero_exit_install_failure_never_launches(self):
        self.env['FAKE_INSTALL_OUTCOME_FAILURE'] = '1'
        p = self.run_install()
        self.assertNotEqual(p.returncode, 0)
        self.assertIn('did not report success', p.stderr)
        self.assertFalse(any(a[:4] == ['devicectl', 'device', 'process', 'launch']
                             for a in self.calls('xcrun')))

    def test_github_failure_has_no_local_build_fallback(self):
        self.env['FAKE_GH_FAIL']='1'
        p=self.run_install();self.assertNotEqual(p.returncode,0)
        self.assertEqual(self.calls('xcodebuild'),[]);self.assert_no_install()

    def test_connecting_phone_stops_before_download_with_recovery(self):
        payload = json.loads(self.devices.read_text())
        payload['result']['devices'][0]['connectionProperties']['tunnelState'] = 'connecting'
        self.devices.write_text(json.dumps(payload))
        p = self.run_install()
        self.assertNotEqual(p.returncode, 0)
        self.assertIn('USB', p.stderr)
        self.assertEqual(self.calls('gh'), [])
        self.assertEqual(self.calls('codesign'), [])
        self.assert_no_install()

    def test_missing_device_udid_stops_before_signing(self):
        payload = json.loads(self.devices.read_text())
        del payload['result']['devices'][0]['hardwareProperties']['udid']
        self.devices.write_text(json.dumps(payload))
        p = self.run_install()
        self.assertNotEqual(p.returncode, 0)
        self.assertEqual(self.calls('codesign'), [])
        self.assert_no_install()

    def test_missing_or_duplicate_checksum_stops_before_signing(self):
        checksum = self.assets / 'SHA256SUMS'
        original = checksum.read_text()
        for content in ['', original + original]:
            with self.subTest(content=content):
                checksum.write_text(content)
                p = self.run_install('--prepare-only')
                self.assertNotEqual(p.returncode, 0)
                self.assertIn('checksum', p.stderr.lower())
                self.assertEqual(self.calls('codesign'), [])

    def test_missing_release_asset_stops_before_download(self):
        release = json.loads(self.release.read_text())
        release['assets'] = []
        self.release.write_text(json.dumps(release))
        p = self.run_install('--prepare-only')
        self.assertNotEqual(p.returncode, 0)
        self.assertFalse(any(a[:2] == ['release', 'download'] for a in self.calls('gh')))
        self.assertEqual(self.calls('codesign'), [])

    def test_explicit_invalid_profile_does_not_fall_back_to_valid_cached_profile(self):
        invalid = self.root / 'expired.mobileprovision'
        data = dict(self.profile_data, ExpirationDate=datetime.datetime(2000, 1, 1))
        invalid.write_bytes(plistlib.dumps(data))
        p = self.run_install('--prepare-only', '--profile', str(invalid))
        self.assertNotEqual(p.returncode, 0)
        self.assertIn('expired', p.stderr)
        self.assertEqual(self.calls('codesign'), [])

    def test_local_mode_rejects_release_only_options_without_build(self):
        for args in [('--version', '4.5.6'), ('--prepare-only',), ('--profile', str(self.profile))]:
            p = self.run_install('--local', *args)
            self.assertNotEqual(p.returncode, 0)
            self.assertEqual(self.calls(), [])

    def test_invalid_version_and_unknown_flags_stop_before_external_tools(self):
        for args in [('--version','../bad'),('--version',''),('--device',''),('--unknown',)]:
            p=self.run_install(*args);self.assertNotEqual(p.returncode,0)
            self.assertEqual(self.calls(),[])


if __name__=='__main__': unittest.main(verbosity=2)
