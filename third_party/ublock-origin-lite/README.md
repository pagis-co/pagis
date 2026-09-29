# uBlock Origin Lite

The content blocker the computer's Chromium loads. It is the
Manifest V3 build: Chromium 152 no longer loads the Manifest V2 uBlock
Origin. The release is taken as it is published, with no fork and no
rename.

- Upstream: <https://github.com/uBlockOrigin/uBOL-home>
- Release tag: `2026.901.1442`
- Asset: `uBOLite_2026.901.1442.chromium.zip`
- sha256: `71a8c65573489e0702f6398d3d744fcbe2bbc48336a75781c90bec50acb00b7e`
- License: GPL-3.0-only, the `LICENSE` file beside this one

The image downloads that asset in the `blocker` stage of
`computer/Dockerfile`, which verifies the sha256 and unpacks the tree
to `/opt/pagis/ublock-origin-lite`. `computer/browser.sh` starts
Chromium with `--load-extension` and `--disable-extensions-except` on
that path. The extension keeps the filtering mode it ships with.

The extension is a separate program: Chromium runs it, and no Pagis
crate or bundle links it, imports it or shares memory with it. So the
image is an aggregate under section 5 of the GPL, and the license of
the extension does not reach the rest of Pagis. The published image
carries the extension, so it carries the extension's license with it.
The rulesets in the release are compiled from EasyList, EasyPrivacy,
uAssets, Peter Lowe's list and urlhaus-filter, which carry their own
terms.

To take a newer release:

```bash
tag=<tag>
curl -fsSLO "https://github.com/uBlockOrigin/uBOL-home/releases/download/${tag}/uBOLite_${tag}.chromium.zip"
shasum -a 256 "uBOLite_${tag}.chromium.zip"
unzip -p "uBOLite_${tag}.chromium.zip" LICENSE.txt > third_party/ublock-origin-lite/LICENSE
```

Then write the new tag and sha256 above, set `UBOL_VERSION` and
`UBOL_SHA256` in `computer/Dockerfile`, and bump the image version.
