# Native platform signing

Minisign protects plusplus updates after download. Apple code signing/notarization and
Windows Authenticode are separate trust systems that prevent operating-system warnings on
first launch. A public release should ideally have both layers.

The release workflow is ready to use native signing after the maintainer obtains the
certificates and configures repository secrets. Until then it deliberately publishes the
same Minisign-verified packages and the README discloses the limitation.

## macOS

Prerequisites:

1. Join the Apple Developer Program and create a **Developer ID Application** certificate.
2. Export the certificate and private key as a password-protected `.p12` file.
3. Create an app-specific password for the Apple account used by `notarytool`.

Configure these GitHub Actions secrets:

| Secret | Value |
| --- | --- |
| `APPLE_CERTIFICATE_BASE64` | Base64 of the exported `.p12` file. |
| `APPLE_CERTIFICATE_PASSWORD` | Password used when exporting the `.p12`. |
| `APPLE_SIGNING_IDENTITY` | Full identity, for example `Developer ID Application: Example (TEAMID)`. |
| `APPLE_ID` | Apple ID used for notarization. |
| `APPLE_TEAM_ID` | Developer Program team identifier. |
| `APPLE_APP_PASSWORD` | App-specific password for the Apple ID. |

The workflow imports the certificate into a temporary keychain, signs the `.app` with the
hardened runtime, builds the DMG, submits it to Apple, staples the notarization ticket, and
only then creates the Minisign signature.

## Windows

Obtain a standard or EV code-signing certificate that supports CI use and export it as a
password-protected PFX. Configure:

| Secret | Value |
| --- | --- |
| `WINDOWS_CERTIFICATE_BASE64` | Base64 of the PFX file. |
| `WINDOWS_CERTIFICATE_PASSWORD` | PFX password. |

The workflow signs `plusplus.exe` with SHA-256 and a public timestamp before creating the
portable ZIP. If the secrets are absent, the workflow skips Authenticode without pretending
that the executable is platform-signed.

## Verification

After a signed release, verify downloaded artifacts independently:

```bash
# macOS
codesign --verify --deep --strict --verbose=2 /Applications/plusplus.app
spctl --assess --type execute --verbose=2 /Applications/plusplus.app

# Windows PowerShell
Get-AuthenticodeSignature .\plusplus.exe
```

Keep certificate files out of the repository. Rotate or revoke them through the relevant
platform provider if a key is exposed.
