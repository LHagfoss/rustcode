#!/usr/bin/env python3
"""Download a CI-built device archive, sign locally, and optionally install it."""
import argparse
import datetime
import hashlib
import importlib.util
import json
from pathlib import Path, PurePosixPath
import plistlib
import re
import shutil
import stat
import subprocess
import sys
import tempfile
import zipfile

ROOT = Path(__file__).resolve().parents[1]
REPO = 'LHagfoss/rustcode'
BUNDLE = 'com.lhagfoss.rustcode'
ASSET = 'rustcode-ios-unsigned.xcarchive.zip'
RECOVERY = ('Unlock the iPhone and keep its screen on, reconnect it directly by USB, '
            'then try again. If Xcode also cannot connect, restart the phone and Xcode. '
            'The installer has not uninstalled the existing app.')


def run(*args):
    result = subprocess.run(args, capture_output=True)
    if result.returncode:
        detail = result.stderr.decode(errors='replace').strip() or result.stdout.decode(errors='replace').strip()
        raise ValueError(Path(args[0]).name + ' failed' + (': ' + detail if detail else '.'))
    return result.stdout


def parse_args():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument('--version', help='Published version, e.g. 0.1.7 (default: latest release)')
    parser.add_argument('--device', help='Connected iPhone name, identifier, or UDID')
    parser.add_argument('--no-launch', action='store_true', help='Install without launching')
    parser.add_argument('--prepare-only', action='store_true', help='Download and sign without installing')
    parser.add_argument('--profile', type=Path, help='Use this development provisioning profile')
    parser.add_argument('--local', action='store_true', help='Build/install the current checkout instead')
    args = parser.parse_args()
    if args.device is not None and not args.device:
        parser.error('--device needs a nonempty name or ID.')
    args.device = args.device or ''
    if args.version is not None:
        args.version = args.version.removeprefix('v')
        if not re.fullmatch(r'[0-9]+\.[0-9]+\.[0-9]+', args.version):
            parser.error('--version must be a published version such as 0.1.7.')
    if args.local and (args.version or args.profile or args.prepare_only):
        parser.error('--local cannot be combined with --version, --profile, or --prepare-only.')
    return args


def connected_device(directory, selector):
    path = directory / 'devices.json'
    try:
        run('xcrun', 'devicectl', 'list', 'devices', '--timeout', '15', '--json-output', str(path))
        spec = importlib.util.spec_from_file_location('ios_support', ROOT / 'Scripts/ios-release-support.py')
        support = importlib.util.module_from_spec(spec)
        spec.loader.exec_module(support)
        identifier, _, udid = support.device_metadata(path, selector)
        if not isinstance(udid, str) or not udid:
            raise ValueError('The connected iPhone did not report a hardware UDID for profile validation.')
        return identifier, udid
    except ValueError as error:
        raise ValueError(str(error) + '\n' + RECOVERY) from error


def download_release(directory, version):
    tag_args = ['v' + version] if version else []
    release = json.loads(run('gh', 'release', 'view', *tag_args, '--repo', REPO,
                             '--json', 'tagName,isDraft,isPrerelease,assets'))
    tag = release.get('tagName', '')
    if not re.fullmatch(r'v[0-9]+\.[0-9]+\.[0-9]+', tag) or release.get('isDraft') or release.get('isPrerelease'):
        raise ValueError('Expected a published stable RustCode release.')
    if version and tag != 'v' + version:
        raise ValueError('GitHub returned a different release than requested.')
    assets = {asset.get('name') for asset in release.get('assets', [])}
    if not {ASSET, 'SHA256SUMS'} <= assets:
        raise ValueError('The release is missing its unsigned iOS archive or checksums.')
    print('Downloading the GitHub CI build ' + tag + '...', flush=True)
    run('gh', 'release', 'download', tag, '--repo', REPO, '--dir', str(directory),
        '--pattern', ASSET, '--pattern', 'SHA256SUMS')
    matches = []
    for line in (directory / 'SHA256SUMS').read_text().splitlines():
        match = re.fullmatch(r'([a-fA-F0-9]{64})\s+\*?(.+)', line)
        if match and match[2] == ASSET:
            matches.append(match[1].lower())
    archive = directory / ASSET
    if len(matches) != 1 or hashlib.sha256(archive.read_bytes()).hexdigest() != matches[0]:
        raise ValueError('The downloaded iOS archive checksum does not match SHA256SUMS.')
    return tag, archive


def extract_archive(archive, directory):
    with zipfile.ZipFile(archive) as source:
        entries = source.infolist()
        if len(entries) > 20000 or sum(entry.file_size for entry in entries) > 1024 * 1024 * 1024:
            raise ValueError('The release archive exceeds extraction limits.')
        seen = set()
        for entry in entries:
            path = PurePosixPath(entry.filename)
            if path.is_absolute() or '..' in path.parts or '\\' in entry.filename or not path.parts:
                raise ValueError('The release archive contains an unsafe path.')
            mode = entry.external_attr >> 16
            if stat.S_ISLNK(mode) or (stat.S_IFMT(mode) not in (0, stat.S_IFREG, stat.S_IFDIR)):
                raise ValueError('The release archive contains an unsupported file type.')
            if path.parts[0] == '__MACOSX':
                continue
            if path.parts[0] != 'RustCode.xcarchive' or path in seen:
                raise ValueError('The release archive has an unexpected or duplicate path.')
            seen.add(path)
            target = directory.joinpath(*path.parts)
            if entry.is_dir():
                target.mkdir(parents=True, exist_ok=True)
            else:
                target.parent.mkdir(parents=True, exist_ok=True)
                with source.open(entry) as incoming, target.open('wb') as outgoing:
                    shutil.copyfileobj(incoming, outgoing)
                target.chmod((mode & 0o777) or 0o644)
    return directory / 'RustCode.xcarchive/Products/Applications/RustCode.app'


def app_metadata(app, tag):
    info = plistlib.loads((app / 'Info.plist').read_bytes())
    if info.get('CFBundleIdentifier') != BUNDLE or info.get('CFBundleShortVersionString') != tag[1:]:
        raise ValueError('The archived app bundle or version does not match the published release.')
    if 'iPhoneOS' not in info.get('CFBundleSupportedPlatforms', []):
        raise ValueError('The release archive is not a physical iOS device build.')
    build = info.get('CFBundleVersion', '')
    if not isinstance(build, str) or not re.fullmatch(r'[0-9]+(?:\.[0-9]+){0,2}', build):
        raise ValueError('The archived app has an invalid build number.')
    if info.get('CFBundleExecutable') != 'RustCode' or not (app / 'RustCode').is_file():
        raise ValueError('The archived app is missing the RustCode executable.')
    if any(p.suffix in ('.appex', '.app') for p in app.rglob('*')):
        raise ValueError('This installer does not support extensions requiring separate signing profiles.')
    return build


def profile_entitlements(profile, udid):
    expiration = profile.get('ExpirationDate')
    if not isinstance(expiration, datetime.datetime):
        raise ValueError('Missing provisioning profile expiration date.')
    if expiration.replace(tzinfo=datetime.timezone.utc) <= datetime.datetime.now(datetime.timezone.utc):
        raise ValueError('The provisioning profile has expired.')
    if udid and udid not in profile.get('ProvisionedDevices', []) and not profile.get('ProvisionsAllDevices'):
        raise ValueError('The provisioning profile does not include this iPhone.')
    ent = profile.get('Entitlements', {})
    teams, prefixes = profile.get('TeamIdentifier', []), profile.get('ApplicationIdentifierPrefix', [])
    if not teams or not prefixes:
        raise ValueError('The provisioning profile has no signing team or application prefix.')
    application = prefixes[0] + '.' + BUNDLE
    if ent.get('application-identifier') not in (application, prefixes[0] + '.*'):
        raise ValueError('The provisioning profile is for a different app.')
    if ent.get('com.apple.developer.team-identifier') != teams[0]:
        raise ValueError('The provisioning profile has inconsistent signing team metadata.')
    result = {'application-identifier': application, 'com.apple.developer.team-identifier': teams[0]}
    if 'get-task-allow' in ent:
        result['get-task-allow'] = ent['get-task-allow']
    if 'keychain-access-groups' in ent:
        result['keychain-access-groups'] = [application if group == prefixes[0] + '.*' else group
                                            for group in ent['keychain-access-groups']]
        if any('*' in group for group in result['keychain-access-groups']):
            raise ValueError('The profile has unsupported keychain groups.')
    return result


def signing_profile(explicit, udid):
    identities = set(re.findall(r'\b[0-9A-Fa-f]{40}\b', run('security', 'find-identity', '-v', '-p', 'codesigning').decode()))
    identities = {identity.upper() for identity in identities}
    if not identities:
        raise ValueError('No valid private code-signing identity is available in Keychain. Configure signing in Xcode first.')
    candidates = [explicit.expanduser()] if explicit else sorted((ROOT / 'build').glob('**/embedded.mobileprovision'))
    if not explicit:
        for folder in ['Library/Developer/Xcode/UserData/Provisioning Profiles', 'Library/MobileDevice/Provisioning Profiles']:
            candidates.extend(sorted((Path.home() / folder).glob('*.mobileprovision')))
    errors = []
    for path in candidates:
        try:
            profile = plistlib.loads(run('security', 'cms', '-D', '-i', str(path)))
            entitlements = profile_entitlements(profile, udid)
            certificates = {hashlib.sha1(cert).hexdigest().upper() for cert in profile.get('DeveloperCertificates', [])}
            matching = sorted(certificates & identities)
            if not matching:
                raise ValueError('The profile has no matching valid private signing identity.')
            return path, matching[0], entitlements
        except (ValueError, OSError, plistlib.InvalidFileException) as error:
            # Do not print decoded profiles, certificate data, or personal paths.
            if explicit:
                raise
            errors.append(str(error) if not str(error).startswith('security failed') else 'Cannot decode a cached profile.')
    detail = (' ' + '; '.join(sorted(set(errors)))) if errors else ''
    raise ValueError('No usable cached provisioning profile.' + detail +
                     ' Configure signing in Xcode, or pass --profile PATH. Use --local to build unpublished source.')


def sign_app(app, profile, identity, entitlements, directory):
    shutil.copy2(profile, app / 'embedded.mobileprovision')
    entitlements_path = directory / 'signing-entitlements.plist'
    entitlements_path.write_bytes(plistlib.dumps(entitlements))
    nested = [p for p in app.rglob('*') if p.suffix in ('.framework', '.dylib')]
    for target in sorted(nested, key=lambda p: len(p.parts), reverse=True):
        run('codesign', '--force', '--sign', identity, '--timestamp=none', str(target))
    run('codesign', '--force', '--sign', identity, '--timestamp=none', '--generate-entitlement-der',
        '--entitlements', str(entitlements_path), str(app))
    run('codesign', '--verify', '--deep', '--strict', str(app))


def main():
    args = parse_args()
    if args.local:
        command = ['bash', str(ROOT / 'Scripts/install-local-on-iphone.sh')]
        if args.device:
            command += ['--device', args.device]
        if args.no_launch:
            command += ['--no-launch']
        return subprocess.call(command)
    for tool in ['gh', 'security', 'codesign'] + ([] if args.prepare_only and not args.device else ['xcrun']):
        if not shutil.which(tool):
            raise ValueError(tool + ' is required. Install GitHub CLI and Xcode command line tools.')
    with tempfile.TemporaryDirectory(prefix='rustcode-github-install-') as temporary:
        directory = Path(temporary)
        device, udid = ('', '') if args.prepare_only and not args.device else connected_device(directory, args.device)
        tag, archive = download_release(directory, args.version)
        app = extract_archive(archive, directory)
        build = app_metadata(app, tag)
        profile, identity, entitlements = signing_profile(args.profile, udid)
        sign_app(app, profile, identity, entitlements, directory)
        cache = ROOT / 'build/github-releases' / tag
        cache.mkdir(parents=True, exist_ok=True)
        prepared = Path(tempfile.mkdtemp(prefix='signed-', dir=cache)) / 'RustCode.app'
        shutil.copytree(app, prepared)
        print(f'Prepared GitHub CI build {tag} (build {build}): {prepared}', flush=True)
        if args.prepare_only:
            return 0
        print(f'Installing RustCode {tag[1:]} (build {build}) over the existing app...', flush=True)
        try:
            install_result = directory / 'install-result.json'
            output = run('xcrun', 'devicectl', 'device', 'install', 'app', '--timeout', '120',
                         '--device', device, '--json-output', str(install_result), str(prepared))
            if json.loads(install_result.read_text()).get('info', {}).get('outcome') != 'success':
                raise ValueError('Device installation did not report success.')
            print(output.decode(errors='replace'), end='')
        except (ValueError, OSError) as error:
            raise ValueError(str(error) + '\n' + RECOVERY) from error
        if not args.no_launch:
            print(run('xcrun', 'devicectl', 'device', 'process', 'launch', '--timeout', '30',
                      '--device', device, BUNDLE).decode(errors='replace'), end='')
        print(f'Installed RustCode {tag[1:]} (build {build}).')
    return 0


if __name__ == '__main__':
    try:
        sys.exit(main())
    except (ValueError, OSError, KeyError, TypeError, zipfile.BadZipFile, plistlib.InvalidFileException) as error:
        print('Error: ' + str(error), file=sys.stderr)
        sys.exit(1)
