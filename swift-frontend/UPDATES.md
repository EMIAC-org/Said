# Swift Update Pipeline

Said's Swift frontend uses Sparkle 2 for in-app updates.

## Client

- `SoftwareUpdateManager` starts Sparkle when the app bundle contains both `SUFeedURL` and `SUPublicEDKey`.
- The menu bar and Settings > About expose `Check for Updates...`.
- Settings > About exposes Sparkle's own persisted toggles for automatic checks and automatic downloads.

## Bundle Metadata

`swift-frontend/bundle.sh` writes the Sparkle keys into `Contents/Info.plist`:

- `SUFeedURL`
- `SUPublicEDKey`
- `SUEnableDownloaderService`
- `SUEnableInstallerLauncherService`

For local builds, set:

```bash
SPARKLE_PUBLIC_ED_KEY="..." \
SPARKLE_FEED_URL="https://emiac-org.github.io/Said/appcast.xml" \
./scripts/build-swift-dmg.sh
```

If `SPARKLE_PUBLIC_ED_KEY` is empty, the app still builds, but the updater is disabled.

## Release Automation

`.github/workflows/swift-release.yml` is the end-to-end pipeline:

1. Builds `said-backend`.
2. Builds the Swift app bundle.
3. Embeds `Sparkle.framework` and the Rust sidecar.
4. Creates a DMG.
5. Builds Sparkle's `generate_appcast`.
6. Signs `updater/appcast.xml` with `SPARKLE_PRIVATE_KEY`.
7. Publishes the DMG to GitHub Releases.
8. Deploys the `updater/` folder to GitHub Pages.

Required GitHub configuration:

- Repository variable: `SPARKLE_PUBLIC_ED_KEY`
- Repository secret: `SPARKLE_PRIVATE_KEY`

Generate the Sparkle keys once with Sparkle's `generate_keys` tool. Commit only the public key value through the GitHub repository variable. Never commit the private key.
