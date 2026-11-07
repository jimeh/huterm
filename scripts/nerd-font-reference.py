"""Record how patched Nerd Font Mono families draw each icon, for comparison.

Huterm draws Nerd Font icons from its bundled symbols font into a regular
font's cells. Its `renderer::symbols` reference test compares that placement
with the same icons in the Nerd Fonts project's own patched "Nerd Font Mono"
build of each family below. This script downloads each family's pinned
release archive and its unpatched source from the Nerd Fonts repository,
checks that patching kept the cell (advance and line), and writes one fixture
per family: the cell metrics and, for every icon the regular font lacks and
the bundled symbols font has, the patched glyph's ink box in font units.

Usage (through `mise run nerd-fonts:reference`):

    python -I scripts/nerd-font-reference.py <SymbolsNerdFont-Regular.ttf> \
        <cache directory> <output directory>
"""

import hashlib
import sys
import tarfile
from pathlib import Path
from urllib.parse import quote
from urllib.request import urlretrieve

from fontTools.pens.boundsPen import BoundsPen
from fontTools.ttLib import TTFont

NF_VERSION = "3.5.1"
RELEASE = f"https://github.com/ryanoasis/nerd-fonts/releases/download/v{NF_VERSION}"
UNPATCHED = (
    f"https://raw.githubusercontent.com/ryanoasis/nerd-fonts/v{NF_VERSION}"
    "/src/unpatched-fonts"
)

FAMILIES = [
    {
        "slug": "dejavu-sans-mono",
        "archive": "DejaVuSansMono.tar.xz",
        "archive_sha256": "9d3f2d60faa790b101903c2984b89c11397fc63b9ce43f2dcd25ffed4fb44e5f",
        "patched": "DejaVuSansMNerdFontMono-Regular.ttf",
        "regular": "DejaVuSansMono/DejaVuSansMono.ttf",
        "regular_sha256": "f3886748a8fc6d90e711b005aa7ca2921e78c6d5d3e5ec9e673887f841e06fe7",
    },
    {
        "slug": "meslo-lg-m",
        "archive": "Meslo.tar.xz",
        "archive_sha256": "6b6624632dc6873dfb7681c3f818e7c01ab601ab707690b6440933bbe57e2b11",
        "patched": "MesloLGMNerdFontMono-Regular.ttf",
        "regular": "Meslo/M/Meslo LG M Regular for Powerline.ttf",
        "regular_sha256": "be991137ccbe87765db7cbcdc034447b15913aa2fad01acebabbcb65b1d4a8a8",
    },
    {
        "slug": "jetbrains-mono",
        "archive": "JetBrainsMono.tar.xz",
        "archive_sha256": "04d5e8f903693f9dd13e16f867e994834e681eb3c72c0d337a770dcda09010cf",
        "patched": "JetBrainsMonoNerdFontMono-Regular.ttf",
        "regular": "JetBrainsMono/Ligatures/JetBrainsMono-Regular.ttf",
        "regular_sha256": "a0bf60ef0f83c5ed4d7a75d45838548b1f6873372dfac88f71804491898d138f",
    },
]


def fetch(url: str, target: Path, sha256: str) -> Path:
    if not target.exists():
        target.parent.mkdir(parents=True, exist_ok=True)
        print(f"Info: downloading {url}")
        partial = target.with_suffix(target.suffix + ".partial")
        urlretrieve(url, partial)
        partial.rename(target)
    actual = hashlib.sha256(target.read_bytes()).hexdigest()
    if actual != sha256:
        raise ValueError(f"{target} has SHA-256 {actual}, expected {sha256}")
    return target


def candidate(code: int) -> bool:
    """Mirrors `renderer::symbols::candidate`."""
    return (
        0xE000 <= code <= 0xF8FF
        or 0xF0000 <= code <= 0xFFFFD
        or 0x23FB <= code <= 0x23FE
        or code == 0x2B58
    )


def cell(font: TTFont) -> tuple[int, int, int, int]:
    """Units per em, the advance of `M`, and the hhea ascent and descent."""
    advance = font["hmtx"][font.getBestCmap()[ord("M")]][0]
    hhea = font["hhea"]
    return font["head"].unitsPerEm, advance, hhea.ascent, hhea.descent


def write_fixture(family: dict, cache: Path, symbols: set[int], out: Path) -> None:
    archive = fetch(
        f"{RELEASE}/{family['archive']}",
        cache / family["archive"],
        family["archive_sha256"],
    )
    regular_path = fetch(
        f"{UNPATCHED}/{quote(family['regular'])}",
        cache / "unpatched" / family["regular"],
        family["regular_sha256"],
    )
    with tarfile.open(archive) as tar:
        member = tar.extractfile(family["patched"])
        if member is None:
            raise ValueError(f"{family['patched']} is not in {archive}")
        patched_path = cache / "patched" / family["patched"]
        patched_path.parent.mkdir(parents=True, exist_ok=True)
        patched_path.write_bytes(member.read())

    regular = TTFont(regular_path)
    patched = TTFont(patched_path)
    if cell(regular) != cell(patched):
        raise ValueError(
            f"{family['slug']}: patching changed the cell {cell(regular)} -> {cell(patched)}"
        )
    units_per_em, advance, ascent, descent = cell(patched)
    # The patcher scales icons by the cap height it measured, which it
    # records in the patched font even where the regular font has none.
    cap_height = patched["OS/2"].sCapHeight
    name = patched["name"].getDebugName(1)

    regular_cmap = regular.getBestCmap()
    cmap = patched.getBestCmap()
    glyphs = patched.getGlyphSet()
    rows = []
    for code in sorted(cmap):
        if not candidate(code) or code in regular_cmap or code not in symbols:
            continue
        pen = BoundsPen(glyphSet=glyphs)
        glyphs[cmap[code]].draw(pen)
        if pen.bounds is None:
            continue
        x_min, y_min, x_max, y_max = (round(value) for value in pen.bounds)
        rows.append(f"{code:05X} {x_min} {y_min} {x_max} {y_max}")

    header = [
        "# @generated by scripts/nerd-font-reference.py; run",
        "# `mise run nerd-fonts:reference` to regenerate it.",
        f"# {family['patched']} from Nerd Fonts {NF_VERSION} {family['archive']}",
        f"# (SHA-256 {family['archive_sha256']}), compared with the unpatched",
        f"# {family['regular']} (SHA-256 {family['regular_sha256']}).",
        "# Icon rows: code point, then the patched glyph's ink box in font units:",
        "# x min, y min, x max, y max.",
        f"family {name}",
        f"units_per_em {units_per_em}",
        f"advance {advance}",
        f"ascent {ascent}",
        f"descent {descent}",
        f"cap_height {cap_height}",
    ]
    path = out / f"{family['slug']}.txt"
    path.write_text("\n".join(header + rows) + "\n", encoding="utf-8")
    print(f"Info: wrote {path} with {len(rows)} icons")


def main() -> None:
    symbols_path, cache_dir, out_dir = (Path(arg) for arg in sys.argv[1:4])
    symbols = set(TTFont(symbols_path).getBestCmap())
    out_dir.mkdir(parents=True, exist_ok=True)
    for family in FAMILIES:
        write_fixture(family, cache_dir / NF_VERSION / "reference", symbols, out_dir)


if __name__ == "__main__":
    main()
