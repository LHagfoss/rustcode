# Install the GitHub-built iOS release

Goal: Make the standard iPhone installer consume the unsigned device archive built by the existing GitHub tag pipeline. Sign on the Mac without compiling, uploading signing material, uninstalling the app, or changing provisioning.

1. Preserve the current source installer behind `--local` and retain its fixture tests.
2. Add an installer that resolves the latest published release or an explicit version, downloads the device archive and SHA256SUMS, verifies integrity and metadata, and extracts into staging.
3. Select a valid cached provisioning profile and matching local signing identity. Check bundle, expiry and selected-device membership. Sign nested frameworks before the app and verify the signature. `--prepare-only` stops before device installation.
4. Test with fake GitHub, security, codesign and device tools: no compiler invocation; correct default/version/device selection; checksum, profile, signing and install failures; retained app data through an install-over operation.
5. Verify against the actual CI-built release without installing on a phone, update usage docs, and bump to 0.1.7/build 8. Review, auto-merge after CI, tag and verify public release assets, then sign that published artifact locally and sync main.

The GitHub archive remains unsigned. The local source build is an explicit alternative for unpublished changes. No app UI or host protocol changes are required.
