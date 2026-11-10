# Application icons

The 2026-11-10 approved icon is used by the native Tauri application, macOS ICNS resources, Windows ICO resources, the in-app brand and the browser favicon. This is a development-branch asset update, not a signed or notarized release.

## Assets and preservation

- `desktop/icons/v2/source-approved.png` is the unchanged approved image.
- `desktop/icons/v2/icon.png` is the 1024-pixel RGBA master, with 16/32/64/128/256/512/1024 PNG sizes and a Retina 128@2x alias.
- `icon.icns` contains 11 standard macOS/Retina entries. `icon.ico` contains 16/32/64/128/256 sizes.
- `web/public/icons/v2/app-icon.png` and `favicon-32.png` use the same approved art.
- Existing `desktop/icons/icon.png` and `web/public/cpaper.png` remain untouched. Previous commits and published tags are unchanged.
- `manifest.json` records SHA-256 values. Resizing uses Lanczos; imperceptible alpha 1/255 becomes fully transparent and 254/255 becomes opaque. Fully transparent pixels have their hidden RGB cleared, preventing fringes in renderers that inspect those channels. The artwork is not redrawn.

## Regeneration and checking

Read-only validation requires only Python 3's standard library:

```sh
python3 scripts/app-icons.py --check
```

Regeneration additionally requires ImageMagick 7's `magick` command:

```sh
python3 scripts/app-icons.py
```

The script validates PNG dimensions/color type, decoded alpha and hidden RGB, every hash, ICNS entry lengths and original PNG payloads, ICO size entries, Tauri paths, favicon and in-app image copies. The original source is never overwritten.

## Desktop integration

`desktop/tauri.conf.json` explicitly lists the v2 PNG, ICNS and ICO paths. Tauri uses the ICNS for the macOS development application icon and the bundler uses it for macOS app resources. The existing `scripts/assemble-desktop.sh` now copies the repository's v2 ICNS instead of a missing external asset. Its missing `desktop/bundle/Info.plist` and embedded placeholder page have been restored so the preserved source includes its own required assembly inputs.

## Verification boundary

The isolated cloud check passed 65 frontend tests and the production TypeScript/Vite build. Icon checks passed for all formats; PNG opacity ranges and fully transparent corners were inspected. Native macOS Dock/Finder presentation, app signing and notarization require a separate Mac validation; cloud web builds do not prove those stages passed.

