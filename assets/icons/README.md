# Icons

Every icon in the browser chrome is a Lucide icon. https://lucide.dev

- Version: 1.59.0
- Source: https://github.com/lucide-icons/lucide/releases/download/1.59.0/lucide-icons-1.59.0.zip
- SHA-256 of that zip: `aae4638a541df91e28cd7733eed2d0d87e72da0942a28b306941284fd00e176a`
  (matches the digest GitHub publishes for the release asset)
- License: ISC. Text in `lucide/LICENSE`, fetched from the repo at tag 1.59.0
  (SHA-256 `b495047bd93a9b06913511076f504daba17d5bbeb3e0650f3bb53a4220329c57`; the zip carries no license file).

Only the icons Lumen draws are copied, byte for byte, into `lucide/`.
`src/chrome/icons.rs` maps Lumen's icon names to these files.
The SVGs are `include_str!`ed. Nothing is read from disk at run time.
To add an icon: copy its SVG from the same release, add one line to the table in `icons.rs`.
To bump the version: replace the files, update this page, check the hash.
