# Release signing

Releases are signed with Ed25519 (card B-06). The workflows sign `SHA256SUMS` (or `SHA256SUMS-<version>` for nightlies) with the private key in the GitHub Actions secret `RELEASE_SIGNING_KEY` (a PKCS#8 PEM). The public key goes in `docs/release/pelago-release.pub` (a PEM `PUBLIC KEY`), which the workflows use to check their own signature, and which `pelago self update` will embed.

Until the secret exists, nightlies publish checksums with no signature (a warning in the run), and a tagged release fails closed.

## Create the key (once, PowerShell)

Git for Windows ships OpenSSL 3. Keep the private key out of the repository and out of chat.

```powershell
$ssl = "C:\Program Files\Git\usr\bin\openssl.exe"
& $ssl genpkey -algorithm ed25519 -out pelago-release.key
& $ssl pkey -in pelago-release.key -pubout -out pelago-release.pub
Get-Content pelago-release.key -Raw | gh secret set RELEASE_SIGNING_KEY --repo pelagolang/pelago
```

Then commit `pelago-release.pub` as `docs/release/pelago-release.pub`, and store `pelago-release.key` somewhere safe and offline (a password manager). Losing it means rotating the key (a release signed by the old key announces the new one).

## Verify a download by hand

```powershell
Get-FileHash pelago-0.1.0-x86_64-windows.zip -Algorithm SHA256   # compare with the line in SHA256SUMS
& $ssl pkeyutl -verify -rawin -pubin -inkey pelago-release.pub -in SHA256SUMS -sigfile SHA256SUMS.sig
```

Windows binaries are not Authenticode-signed at first, so SmartScreen shows an "unknown publisher" warning on first run. The checksum and signature above are the way to check them.
