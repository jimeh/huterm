# DejaVu Sans Mono provenance

Huterm embeds the unmodified Book, Bold, Oblique, and Bold Oblique faces of
DejaVu Sans Mono 2.37 from the official
[release archive](https://github.com/dejavu-fonts/dejavu-fonts/releases/download/version_2_37/dejavu-fonts-ttf-2.37.tar.bz2).
Its SHA-256 is
`fa9ca4d13871dd122f61258a80d01751d603b4d3ee14095d65453b4e846e17d7`.
The TTFs are in `crates/huterm-gpui/assets/fonts/dejavu/`; `provenance.json`
pins each file and the upstream `LICENSE` alongside this document.

The upstream license includes the Bitstream Vera and Arev terms. DejaVu's
changes are in the public domain. The notices component uses `Bitstream-Vera`
and local `LicenseRef-Arev` and `LicenseRef-DejaVu-Public-Domain` identifiers
for those grants; all their terms are in the unmodified `LICENSE`.
That file also includes notices for TeX Gyre DJV Math, which Huterm does not
bundle. The license accompanies Huterm's macOS and Linux packages.

GPUI loads these embedded bytes at startup, without reading package resources.
On macOS the bundled faces take precedence over an installed DejaVu Sans Mono.
On Linux an installed face with the same properties takes precedence; bundled
faces supply any missing styles and weights.
