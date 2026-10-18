# macOS release signing and notarization

Tagged releases publish two macOS assets: the CLI archive
`rustcode-macos-aarch64.tar.gz` (also used by the Homebrew formula) and the
SwiftUI app `rustcode-macos-aarch64-swift-app.zip`, which bundles the same
engine as `Contents/Helpers/rustcode`.

The release workflow (`.github/workflows/build.yml`) signs both with a
Developer ID and notarizes them **only when all five secrets below are set**.
Until then it prints a notice and ships them as before: the app ad-hoc
signed, the CLI as the linker signed it, neither notarized. If only some of
the secrets are set the job fails, so a half-finished setup is not mistaken
for a signed release.

## What the workflow does

`scripts/macos-release-signing.sh` holds the steps:

1. `check` decides whether signing is on and reports missing secrets.
2. `import` decodes the `.p12` into a temporary keychain under
   `$RUNNER_TEMP` and exports its `Developer ID Application` identity as
   `MACOS_SIGN_IDENTITY`. `cleanup` deletes the keychain at the end of the
   job, also after a failure.
3. CLI (`build` job, macOS): sign `rustcode` with the hardened runtime and a
   secure timestamp, then submit it to the notary service in a zip. A bare
   executable cannot be stapled; Gatekeeper fetches its ticket online.
4. App (`swift-app` job): `scripts/package-swift-app.sh` sees
   `MACOS_SIGN_IDENTITY` and signs `Contents/Helpers/rustcode`, then the
   bundle, both with `--options runtime --timestamp`. `notarize` submits the
   app with `xcrun notarytool submit --wait`, fails unless the result is
   `Accepted` (printing the notary log), staples the ticket, then runs
   `xcrun stapler validate` and `spctl --assess --type execute`.
5. The released zip is unpacked again, given the quarantine flag a browser
   download sets, and checked with `stapler validate`, `spctl --assess` and
   `codesign --verify --deep --strict`.

No entitlements are needed: the app is not sandboxed, and neither executable
uses JIT or loads libraries signed by someone else. Add an entitlements file
only if that changes (for example `com.apple.security.cs.disable-library-validation`
for third-party plug-ins).

## Secrets

Notarization uses an Apple ID with an app-specific password. (An App Store
Connect API key would work too, but needs a different `notarytool` call; it
is not wired up.)

| Secret | Value |
| --- | --- |
| `MACOS_DEVELOPER_ID_P12_BASE64` | The **Developer ID Application** certificate with its private key, exported as `.p12` and base64 encoded: `base64 -i DeveloperID.p12 \| pbcopy` |
| `MACOS_DEVELOPER_ID_P12_PASSWORD` | The password chosen when exporting the `.p12` |
| `MACOS_NOTARY_APPLE_ID` | The Apple ID email of an Apple Developer Program member of the team |
| `MACOS_NOTARY_TEAM_ID` | The 10-character Team ID (developer.apple.com → Membership details) |
| `MACOS_NOTARY_PASSWORD` | An app-specific password for that Apple ID (account.apple.com → Sign-In and Security → App-Specific Passwords) |

## Setup

1. In the Apple Developer account, create a **Developer ID Application**
   certificate (Certificates, IDs & Profiles → +). Only the Account Holder
   can create one. Install it in the login keychain.
2. In Keychain Access, select the certificate together with its private key
   and export it as `DeveloperID.p12` with a strong password.
3. Create the app-specific password.
4. Add the secrets to the repository, for example:

   ```bash
   gh secret set MACOS_DEVELOPER_ID_P12_BASE64 < <(base64 -i DeveloperID.p12)
   gh secret set MACOS_DEVELOPER_ID_P12_PASSWORD
   gh secret set MACOS_NOTARY_APPLE_ID
   gh secret set MACOS_NOTARY_TEAM_ID
   gh secret set MACOS_NOTARY_PASSWORD
   ```

5. Run the Build workflow with **Run workflow** (`workflow_dispatch`) on
   `main` and check that the `swift-app` job ends with `spctl` reporting
   `accepted` and `source=Notarized Developer ID`. Delete the local `.p12`.
6. Once a release is signed, remove the quarantine workaround from
   [install.md](install.md#native-desktop-app).

To check a downloaded release by hand:

```bash
ditto -x -k rustcode-macos-aarch64-swift-app.zip .
spctl --assess --type execute --verbose=4 RustCode.app
xcrun stapler validate RustCode.app
```
