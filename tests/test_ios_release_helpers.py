#!/usr/bin/env python3
"""Run release/install entrypoints only against fake external command binaries."""
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

DRAFTS = Path(__file__).resolve().parents[1]

FAKE = r'''
import json,os,sys,plistlib
from pathlib import Path
command = Path(sys.argv[1]).name
args = sys.argv[2:]
with open(os.environ['FAKE_CALLS'], 'a') as out:
    out.write(json.dumps([command, args]) + '\n')
if command == 'xcodebuild':
    if '-showBuildSettings' in args:
        print(json.dumps([{'target':'RustCode','buildSettings':{
            'MARKETING_VERSION':os.environ.get('FAKE_VERSION','4.5.6'),
            'CURRENT_PROJECT_VERSION':os.environ.get('FAKE_BUILD','12'),
            'PRODUCT_BUNDLE_IDENTIFIER':'com.example.fixture',
            'PRODUCT_NAME':'RustCode', 'WRAPPER_NAME':'RustCode.app'}}]))
    elif 'archive' in args:
        if os.environ.get('FAKE_ARCHIVE_FAIL'): sys.exit(12)
        archive = Path(args[args.index('-archivePath') + 1])
        app = archive / 'Products/Applications/RustCode.app'
        app.mkdir(parents=True)
        data={'CFBundleShortVersionString':os.environ.get('FAKE_VERSION','4.5.6'),
              'CFBundleVersion':os.environ.get('FAKE_BUILD','12'),
              'CFBundleIdentifier':'com.example.fixture',
              'CFBundleSupportedPlatforms':['iPhoneOS']}
        (app/'Info.plist').write_bytes(plistlib.dumps(data))
        (app/'embedded.mobileprovision').write_text('fixture profile')
    else: sys.exit(91)
elif command == 'xcrun':
    if args[:3] == ['devicectl','list','devices']:
        payload=Path(os.environ['FAKE_DEVICES']).read_text()
        output=args[args.index('--json-output') + 1]
        Path(output).write_text(payload)
    elif args[:4] == ['devicectl','device','install','app']:
        if os.environ.get('FAKE_INSTALL_FAIL'): sys.exit(13)
        if not Path(args[-1]).is_dir(): sys.exit(92)
    elif args[:4] == ['devicectl','device','process','launch']:
        pass
    else: sys.exit(93)
elif command == 'codesign':
    if os.environ.get('FAKE_CODESIGN_FAIL'): sys.exit(14)
else: sys.exit(94)
'''


def device(identifier='phone-a', name='Fixture iPhone', reality='physical',
           kind='iPhone', connected=True):
    return {'identifier': identifier, 'visibilityClass': 'default',
            'capabilities': [], 'propertyDisplayNames': {}, 'properties': {
                'hardware': {'deviceType': kind, 'reality': reality, 'platform': 'iOS',
                             'udid': identifier + '-udid'},
                'connection': {'state': 'connected' if connected else 'disconnected',
                               'pairingState': 'paired', 'transportType': 'wired'},
                'software': {'osVersionNumber': '27.0'},
                'state': {'name': name, 'bootState': 'booted',
                          'developerModeStatus': {'enabled': {'mode': 1}}}}}


class ReleaseHelperTests(unittest.TestCase):
    def setUp(self):
        self.temp = tempfile.TemporaryDirectory(prefix='ios helper fixture ')
        self.addCleanup(self.temp.cleanup)
        self.root = (Path(self.temp.name) / 'app checkout with spaces').resolve()
        (self.root / 'Scripts').mkdir(parents=True)
        (self.root / 'RustCode.xcodeproj').mkdir()
        (self.root / 'RustCode.xcodeproj/project.pbxproj').write_text('fixture project')
        (self.root / 'RustCode').mkdir()
        (self.root / 'RustCode/RustCodeApp.swift').write_text('struct FixtureApp {}\n')
        (self.root / 'RustCodeRemote').mkdir()
        (self.root / 'RustCodeRemote/Package.swift').write_text('// fixture manifest\n')
        (self.root / 'Config').mkdir()
        (self.root / 'Config/Base.xcconfig').write_text('// fixture shared config\n')
        for source in (DRAFTS / 'Scripts').glob('*'):
            if source.is_file(): shutil.copy2(source, self.root / 'Scripts' / source.name)
        self.bin = Path(self.temp.name) / 'fake binaries'
        self.bin.mkdir()
        dispatcher = self.bin / 'fake.py'
        dispatcher.write_text(FAKE)
        for name in ['xcodebuild', 'xcrun', 'codesign']:
            wrapper = self.bin / name
            wrapper.write_text('#!/bin/sh\nexec ' + shlex.quote(sys.executable) + ' '
                               + shlex.quote(str(dispatcher)) + ' "$0" "$@"\n')
            wrapper.chmod(0o755)
        self.calls_file = Path(self.temp.name) / 'calls.jsonl'
        self.devices_file = Path(self.temp.name) / 'devices.json'
        self.set_devices([device()])
        self.env = dict(os.environ, PATH=str(self.bin) + ':/usr/bin:/bin',
                        FAKE_CALLS=str(self.calls_file), FAKE_DEVICES=str(self.devices_file))

    def set_devices(self, devices):
        self.devices_file.write_text(json.dumps({'info': {}, 'result': {'devices': devices}}))

    def run_helper(self, name, *args):
        script = self.root / 'Scripts' / name
        self.assertTrue(script.is_file(), 'helper draft not implemented: ' + name)
        return subprocess.run(['bash', str(script), *args], cwd=self.root,
                              env=self.env, text=True, capture_output=True)

    def calls(self, kind=None):
        records = [json.loads(line) for line in self.calls_file.read_text().splitlines()] \
            if self.calls_file.exists() else []
        return [args for cmd, args in records if kind is None or cmd == kind]

    def app(self, version='4.5.6', build='12', bundle='com.example.fixture', profile=True,
            fingerprint=True):
        app = self.root / 'build/releases/v4.5.6/RustCode.xcarchive/Products/Applications/RustCode.app'
        app.mkdir(parents=True)
        (app / 'Info.plist').write_bytes(plistlib.dumps({
            'CFBundleShortVersionString': version, 'CFBundleVersion': build,
            'CFBundleIdentifier': bundle, 'CFBundleSupportedPlatforms': ['iPhoneOS']}))
        if profile: (app / 'embedded.mobileprovision').write_text('fixture profile')
        if fingerprint:
            result = subprocess.run([sys.executable, str(self.root / 'Scripts/ios-release-support.py'),
                                     'fingerprint', str(self.root)], text=True, capture_output=True)
            self.assertEqual(result.returncode, 0, result.stderr)
            (app.parents[2] / 'source-fingerprint.sha256').write_text(result.stdout)
        return app

    def archives(self):
        return [args for args in self.calls('xcodebuild') if 'archive' in args]

    def installs(self):
        return [args for args in self.calls('xcrun') if args[:4] == ['devicectl','device','install','app']]

    def launches(self):
        return [args for args in self.calls('xcrun') if args[:4] == ['devicectl','device','process','launch']]

    def test_archive_uses_resolved_metadata_and_cached_signing(self):
        p = self.run_helper('build-ios-release.sh')
        self.assertEqual(p.returncode, 0, p.stderr)
        archive = self.root / 'build/releases/v4.5.6/RustCode.xcarchive'
        self.assertTrue((archive / 'Products/Applications/RustCode.app/Info.plist').is_file())
        args = self.archives()[0]
        self.assertEqual(args[args.index('-destination') + 1], 'generic/platform=iOS')
        self.assertNotIn('-allowProvisioningUpdates', args)
        self.assertNotIn('CODE_SIGNING_ALLOWED=NO', args)
        self.assertIn(str(archive), p.stdout)
        self.assertEqual(self.installs(), [])

    def test_invalid_metadata_fails_before_archive(self):
        self.env['FAKE_VERSION'] = '../unsafe'
        p = self.run_helper('build-ios-release.sh')
        self.assertNotEqual(p.returncode, 0)
        self.assertEqual(self.archives(), [])

    def test_failed_rebuild_preserves_existing_archive(self):
        app = self.app(build='11')
        sentinel = app / 'keep-me'
        sentinel.write_text('existing archive')
        self.env['FAKE_ARCHIVE_FAIL'] = '1'
        p = self.run_helper('build-ios-release.sh')
        self.assertNotEqual(p.returncode, 0)
        self.assertEqual(sentinel.read_text(), 'existing archive')

    def test_signature_failure_never_publishes_archive(self):
        self.env['FAKE_CODESIGN_FAIL'] = '1'
        p = self.run_helper('build-ios-release.sh')
        self.assertNotEqual(p.returncode, 0)
        self.assertFalse((self.root / 'build/releases/v4.5.6/RustCode.xcarchive').exists())

    def test_single_physical_phone_selected_over_connected_simulator(self):
        app = self.app()
        self.set_devices([device(), device('sim-a', reality='simulated'), device('ipad-a', kind='iPad')])
        p = self.run_helper('install-on-iphone.sh')
        self.assertEqual(p.returncode, 0, p.stderr)
        self.assertEqual(self.archives(), [])
        args = self.installs()[0]
        self.assertEqual(args[args.index('--device') + 1], 'phone-a')
        self.assertEqual(args[-1], str(app))
        self.assertEqual(len(self.launches()), 1)
        self.assertEqual(self.launches()[0][-1], 'com.example.fixture')

    def test_ambiguous_devices_fail_before_build_or_install(self):
        self.set_devices([device(), device('phone-b', 'Second iPhone')])
        p = self.run_helper('install-on-iphone.sh')
        self.assertNotEqual(p.returncode, 0)
        self.assertIn('--device', p.stderr)
        self.assertEqual(self.archives(), [])
        self.assertEqual(self.installs(), [])

    def test_simulators_and_disconnected_phones_never_selected(self):
        self.set_devices([device('sim-a', reality='simulated'), device(connected=False)])
        p = self.run_helper('install-on-iphone.sh')
        self.assertNotEqual(p.returncode, 0)
        self.assertEqual(self.archives(), [])
        self.assertEqual(self.installs(), [])

    def test_explicit_phone_name_with_spaces_and_no_launch(self):
        self.app()
        self.set_devices([device(), device('phone-b', 'Second iPhone With Spaces')])
        p = self.run_helper('install-on-iphone.sh', '--device', 'Second iPhone With Spaces', '--no-launch')
        self.assertEqual(p.returncode, 0, p.stderr)
        args = self.installs()[0]
        self.assertEqual(args[args.index('--device') + 1], 'phone-b')
        self.assertEqual(self.launches(), [])

    def test_explicit_udid_chooses_the_named_device(self):
        self.app()
        self.set_devices([device(), device('phone-b', 'Second iPhone')])
        p = self.run_helper('install-on-iphone.sh', '--device', 'phone-b-udid')
        self.assertEqual(p.returncode, 0, p.stderr)
        args = self.installs()[0]
        self.assertEqual(args[args.index('--device') + 1], 'phone-b')

    def test_explicit_simulator_is_rejected(self):
        self.set_devices([device('sim-a', reality='simulated')])
        p = self.run_helper('install-on-iphone.sh', '--device', 'sim-a')
        self.assertNotEqual(p.returncode, 0)
        self.assertEqual(self.installs(), [])

    def test_legacy_device_json_selects_physical_phone(self):
        self.app()
        self.set_devices([{'identifier': 'legacy-phone', 'hardwareProperties': {
            'deviceType': 'iPhone', 'reality': 'physical', 'udid': 'legacy-udid'},
            'connectionProperties': {'tunnelState': 'connected', 'pairingState': 'paired'},
            'deviceProperties': {'name': 'Legacy Fixture iPhone'}}])
        p = self.run_helper('install-on-iphone.sh')
        self.assertEqual(p.returncode, 0, p.stderr)
        args = self.installs()[0]
        self.assertEqual(args[args.index('--device') + 1], 'legacy-phone')

    def test_missing_archive_is_built_before_install(self):
        p = self.run_helper('install-on-iphone.sh')
        self.assertEqual(p.returncode, 0, p.stderr)
        self.assertEqual(len(self.archives()), 1)
        self.assertEqual(len(self.installs()), 1)

    def test_stale_build_is_rebuilt_before_install(self):
        self.app(build='11')
        p = self.run_helper('install-on-iphone.sh')
        self.assertEqual(p.returncode, 0, p.stderr)
        self.assertEqual(len(self.archives()), 1)
        app = self.root / 'build/releases/v4.5.6/RustCode.xcarchive/Products/Applications/RustCode.app'
        self.assertEqual(plistlib.loads((app/'Info.plist').read_bytes())['CFBundleVersion'], '12')
        self.assertEqual(len(self.installs()), 1)

    def test_wrong_bundle_is_rebuilt_before_install(self):
        self.app(bundle='com.example.wrong')
        p = self.run_helper('install-on-iphone.sh')
        self.assertEqual(p.returncode, 0, p.stderr)
        self.assertEqual(len(self.archives()), 1)
        self.assertEqual(len(self.installs()), 1)

    def test_unsigned_archive_is_rebuilt_before_install(self):
        self.app(profile=False)
        p = self.run_helper('install-on-iphone.sh')
        self.assertEqual(p.returncode, 0, p.stderr)
        self.assertEqual(len(self.archives()), 1)

    def build_current_archive(self):
        p = self.run_helper('build-ios-release.sh')
        self.assertEqual(p.returncode, 0, p.stderr)
        self.calls_file.write_text('')

    def test_source_change_with_same_version_and_build_triggers_rebuild(self):
        self.build_current_archive()
        (self.root / 'RustCode/RustCodeApp.swift').write_text('struct UpdatedFixtureApp {}\n')
        p = self.run_helper('install-on-iphone.sh')
        self.assertEqual(p.returncode, 0, p.stderr)
        self.assertEqual(len(self.archives()), 1)
        self.assertEqual(len(self.installs()), 1)
        calls = self.calls_file.read_text().splitlines()
        discovery = next(i for i, line in enumerate(calls) if json.loads(line)[1][:3] == ['devicectl','list','devices'])
        build = next(i for i, line in enumerate(calls) if 'archive' in json.loads(line)[1])
        self.assertLess(discovery, build)

    def test_missing_source_fingerprint_triggers_rebuild(self):
        self.app(fingerprint=False)
        p = self.run_helper('install-on-iphone.sh')
        self.assertEqual(p.returncode, 0, p.stderr)
        self.assertEqual(len(self.archives()), 1)

    def test_unchanged_source_archive_is_reused(self):
        self.build_current_archive()
        p = self.run_helper('install-on-iphone.sh')
        self.assertEqual(p.returncode, 0, p.stderr)
        self.assertEqual(self.archives(), [])
        self.assertEqual(len(self.installs()), 1)

    def test_generated_docs_and_test_changes_do_not_force_rebuild(self):
        self.build_current_archive()
        for relative in ['RustCodeRemote/.build/output', 'build/generated',
                         'RustCode.xcodeproj/xcuserdata/local', 'docs/readme.md',
                         'RustCodeRemote/Tests/FixtureTests.swift']:
            path = self.root / relative
            path.parent.mkdir(parents=True, exist_ok=True)
            path.write_text('excluded from app source fingerprint')
        p = self.run_helper('install-on-iphone.sh')
        self.assertEqual(p.returncode, 0, p.stderr)
        self.assertEqual(self.archives(), [])

    def test_ignored_signing_config_change_triggers_rebuild_without_printing_contents(self):
        self.build_current_archive()
        private_text = 'PRIVATE_FIXTURE_SIGNING_SETTING = do-not-print-this\n'
        (self.root / 'Config/Local.xcconfig').write_text(private_text)
        p = self.run_helper('install-on-iphone.sh')
        self.assertEqual(p.returncode, 0, p.stderr)
        self.assertEqual(len(self.archives()), 1)
        self.assertNotIn('do-not-print-this', p.stdout + p.stderr)

    def test_install_failure_does_not_launch(self):
        self.app()
        self.env['FAKE_INSTALL_FAIL'] = '1'
        p = self.run_helper('install-on-iphone.sh')
        self.assertNotEqual(p.returncode, 0)
        self.assertEqual(self.launches(), [])

    def test_invalid_signature_never_reaches_device_install(self):
        self.app()
        self.env['FAKE_CODESIGN_FAIL'] = '1'
        p = self.run_helper('install-on-iphone.sh')
        self.assertNotEqual(p.returncode, 0)
        self.assertEqual(self.installs(), [])
        self.assertEqual(self.launches(), [])

    def test_unrecognized_arguments_do_not_call_external_tools(self):
        p = self.run_helper('install-on-iphone.sh', '--unknown')
        self.assertNotEqual(p.returncode, 0)
        self.assertEqual(self.calls(), [])

    def test_malformed_device_json_fails_before_build_or_install(self):
        self.devices_file.write_text('not JSON')
        p = self.run_helper('install-on-iphone.sh')
        self.assertNotEqual(p.returncode, 0)
        self.assertEqual(self.archives(), [])
        self.assertEqual(self.installs(), [])


if __name__ == '__main__':
    unittest.main(verbosity=2)
