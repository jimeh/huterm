# Symbols Nerd Font provenance

Huterm embeds the unmodified `SymbolsNerdFont-Regular.ttf` from the Nerd Fonts
3.5.1
[NerdFontsSymbolsOnly.tar.xz](https://github.com/ryanoasis/nerd-fonts/releases/download/v3.5.1/NerdFontsSymbolsOnly.tar.xz)
release archive, whose SHA-256 is
`01172f37db8543edb102e5cb5c64101c9f4686630804d49b419aa07b23a69996`.
The TTF is in `crates/huterm-gpui/assets/fonts/nerd-fonts/`. `LICENSE` is the
archive's Nerd Fonts MIT license. `provenance.json` pins the font, that
license, and each icon set's license text in `licenses/` by URL and SHA-256.
The renderer sizes each icon by the release's font-patcher rules through a
port of Ghostty's constraints, whose notice is in
`third-party/nerd-font-constraints/`.

The font combines 14 icon sets under their own licenses. `NOTICE.txt` credits
each one and records the changes Nerd Fonts made, and every icon set has its
own entry in `third-party/licenses/components.json`:

| Icon set | Version | License | Text |
| --- | --- | --- | --- |
| Codicons | 0.0.45 | CC-BY-4.0 | `CC-BY-4.0.txt` |
| Devicons | 2.17.0 | MIT | `devicons-LICENSE` |
| Font Awesome | 6.5.1 | CC-BY-4.0 | `font-awesome-LICENSE.txt`, `CC-BY-4.0.txt` |
| Font Awesome Extension | 0.0.3 | MIT | `font-awesome-extension-LICENCE` |
| Font Logos | 1.3.0 | Unlicense | `font-logos-LICENSE` |
| Hack extra glyphs | - | MIT AND Bitstream-Vera | `hack-LICENSE.txt` |
| Material Design Icons | 2022-10-06 | Apache-2.0 | `material-design-icons-LICENSE` |
| Octicons | 18.3.0 | MIT | `octicons-LICENSE` |
| Seti UI | 0.8.1 | MIT | `seti-ui-LICENSE.txt` |
| Pomicons | 1.001 | OFL-1.1 | `pomicons-LICENSE` |
| Powerline Extra Symbols | 1.200 | MIT | `powerline-extra-LICENSE` |
| Powerline Symbols | 1.000 | MIT | `powerline-symbols-LICENSE.txt` |
| IEC Power Symbols | 2015-02 | MIT | `iec-power-symbols-LICENSE.txt` |
| Weather Icons | 2.0.10 | OFL-1.1 | `weather-icons-OFL.txt` |

Licensing notes, from each set's own license file:

- The Nerd Fonts README lists Font Logos as "unlicensed". Its `LICENSE` at
  v1.3.0 is The Unlicense, a public-domain dedication.
- Font Awesome's license applies CC BY 4.0 to icons packaged as SVG and the
  OFL to its font files. Nerd Fonts built its glyphs from the release SVGs.
  Material Design Icons' license distributes its fonts under Apache 2.0.
- OFL 1.1 requires the Pomicons and Weather Icons glyphs to stay under the OFL
  even though Nerd Fonts labels the whole font MIT, so their components
  present them under OFL 1.1. Huterm ships the file unmodified, and its family
  name uses neither set's reserved name.
- The Weather Icons `OFL.txt` that Nerd Fonts ships leaves its copyright line
  as a template. `NOTICE.txt` credits the authors from the upstream README
  instead of supplying a copyright line.
- Brand icons are trademarks of their owners; no license grants trademark
  rights.

Licenses that live outside the Nerd Fonts tag are pinned to the upstream tag
or commit that matches the Nerd Fonts version table. `CC-BY-4.0.txt` is the
Creative Commons plain-text legal code. The license texts ship as resources in
the macOS bundle and the Linux packages. GPUI loads the embedded font bytes at
startup, without reading package resources, and never installs the font.
