#!/usr/bin/env python3
"""Shared JSON/plist checks for the local release and iPhone install helpers."""
import hashlib
import json
import os
from pathlib import Path
import plistlib
import re
import sys


def fail(message):
    raise ValueError(message)


def metadata(path):
    targets = json.loads(Path(path).read_text())
    settings = [target['buildSettings'] for target in targets if target.get('target') == 'RustCode']
    if len(settings) != 1:
        fail('Expected one RustCode app target in Release build settings.')
    settings = settings[0]
    version = settings.get('MARKETING_VERSION', '')
    build = settings.get('CURRENT_PROJECT_VERSION', '')
    bundle = settings.get('PRODUCT_BUNDLE_IDENTIFIER', '')
    for name, value in [('marketing version', version), ('build number', build)]:
        if not isinstance(value, str) or not re.fullmatch(r'[0-9]+(?:\.[0-9]+){0,2}', value):
            fail('Invalid or unresolved app ' + name + '.')
    if not isinstance(bundle, str) or not re.fullmatch(r'[A-Za-z0-9-]+(?:\.[A-Za-z0-9-]+)+', bundle):
        fail('Invalid or unresolved app bundle identifier.')
    if settings.get('WRAPPER_NAME') != 'RustCode.app':
        fail('Expected the RustCode.app product.')
    print(version, build, bundle, sep='\n')


def validate_app(path, version, build, bundle):
    app = Path(path)
    with (app / 'Info.plist').open('rb') as file:
        info = plistlib.load(file)
    expected = {'CFBundleShortVersionString': version, 'CFBundleVersion': build,
                'CFBundleIdentifier': bundle}
    if any(info.get(key) != value for key, value in expected.items()):
        fail('The archived app does not match the repository version, build and bundle identifier.')
    if 'iPhoneOS' not in info.get('CFBundleSupportedPlatforms', []):
        fail('The archived app is not a physical iOS device build.')
    if not (app / 'embedded.mobileprovision').is_file():
        fail('The archived app has no embedded device signing profile.')


def fingerprint(root):
    root = Path(root)
    files = set()
    excluded = {'.build', 'build', 'xcuserdata', 'docs', 'tests'}
    for relative in ['RustCode', 'RustCodeRemote/Sources', 'RustCode.xcodeproj']:
        directory = root / relative
        if not directory.is_dir():
            continue
        for parent, directories, names in os.walk(directory):
            directories[:] = [name for name in directories if name.lower() not in excluded]
            for name in names:
                if name == '.DS_Store' or name.endswith('.xcuserstate'):
                    continue
                path = Path(parent) / name
                if path.is_file():
                    files.add(path)
    for relative in ['RustCodeRemote/Package.swift', 'RustCodeRemote/Package.resolved',
                     'Package.resolved', 'Config/Base.xcconfig', 'Config/Local.xcconfig']:
        path = root / relative
        if path.is_file():
            files.add(path)
    if not files:
        fail('No app source files were found for the archive fingerprint.')
    digest = hashlib.sha256(b'rustcode-ios-source-v1\0')
    for path in sorted(files, key=lambda path: path.relative_to(root).as_posix()):
        # Include paths and byte lengths so additions, deletions and renames change the hash.
        content = path.read_bytes()
        relative = path.relative_to(root).as_posix().encode('utf-8')
        digest.update(relative + b'\0' + str(len(content)).encode('ascii') + b'\0' + content)
    # Personal signing configuration contributes only to this digest, never output content.
    print(digest.hexdigest())


def validate_fingerprint(archive, expected):
    if not re.fullmatch(r'[0-9a-f]{64}', expected):
        fail('Invalid source fingerprint.')
    marker = Path(archive) / 'source-fingerprint.sha256'
    if not marker.is_file() or marker.read_text().strip() != expected:
        fail('The archive source fingerprint is missing or does not match the current app source.')


def select_device(path, selector=''):
    payload = json.loads(Path(path).read_text())
    devices = payload.get('result', {}).get('devices')
    if not isinstance(devices, list):
        fail('Device discovery returned an unexpected JSON structure.')
    candidates = []
    for device in devices:
        props = device.get('properties', {})
        hardware = props.get('hardware', device.get('hardwareProperties', {}))
        connection = props.get('connection', device.get('connectionProperties', {}))
        state = props.get('state', device.get('deviceProperties', {}))
        if hardware.get('reality') != 'physical' or hardware.get('deviceType') != 'iPhone':
            continue
        if connection.get('state', connection.get('tunnelState')) != 'connected':
            continue
        if connection.get('pairingState') != 'paired':
            continue
        identifier = device.get('identifier')
        if not isinstance(identifier, str) or not identifier or '\n' in identifier or '\r' in identifier:
            continue
        values = [identifier, state.get('name'), hardware.get('udid'),
                  hardware.get('serialNumber'), hardware.get('ecid')]
        if selector and selector not in [str(value) for value in values if value is not None]:
            continue
        candidates.append((identifier, state.get('name', 'Unnamed iPhone')))
    if not candidates:
        fail('No matching connected, paired physical iPhone. Connect and unlock the phone, '
             'trust this Mac, and enable Developer Mode; use --device NAME/ID if needed.')
    if len(candidates) != 1:
        names = ', '.join(str(name).replace('\n', ' ') for _, name in candidates)
        fail('More than one physical iPhone matches (' + names + '). Choose one with --device NAME/ID.')
    print(candidates[0][0])


def main():
    command, *args = sys.argv[1:]
    if command == 'metadata':
        metadata(*args)
    elif command == 'validate-app':
        validate_app(*args)
    elif command == 'select-device':
        select_device(*args)
    elif command == 'fingerprint':
        fingerprint(*args)
    elif command == 'validate-fingerprint':
        validate_fingerprint(*args)
    else:
        fail('Unknown release-support command.')


if __name__ == '__main__':
    try:
        main()
    except (ValueError, KeyError, TypeError, OSError, plistlib.InvalidFileException) as error:
        print('Error: ' + str(error), file=sys.stderr)
        sys.exit(1)
