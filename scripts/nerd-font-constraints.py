"""Generate Huterm's Nerd Font glyph constraint table.

Adapted from Ghostty's src/font/nerd_font_codegen.py, copyright (c) 2024
Mitchell Hashimoto, Ghostty contributors, under MIT. The full notice is
third-party/nerd-font-constraints/LICENSE-Ghostty, and README.md beside it
records what was ported and changed.

The Nerd Fonts font patcher scales and aligns each icon set into a patched
font's cell by rules in its `setup_patch_set`. This script reads those rules
from the pinned font-patcher release, measures each icon in the bundled
Symbols Nerd Font, and writes one constraint per code point to a Rust table
that the renderer applies at draw time.

Usage (through `mise run nerd-fonts:constraints`):

    python -I scripts/nerd-font-constraints.py <SymbolsNerdFont-Regular.ttf> \
        <cache directory> <output .rs file>

The cache directory holds the downloaded font-patcher and the icon sets'
source fonts, which supply their original code points. The font-patcher file
is verified against a pinned SHA-256.
"""

import ast
import hashlib
import math
import sys
from pathlib import Path
from types import SimpleNamespace
from typing import Literal, TypedDict, cast
from urllib.request import urlretrieve

from fontTools.pens.boundsPen import BoundsPen
from fontTools.ttLib import TTFont, TTLibError

NF_VERSION = "3.5.1"
PATCHER_URL = (
    f"https://raw.githubusercontent.com/ryanoasis/nerd-fonts/v{NF_VERSION}/font-patcher"
)
PATCHER_SHA256 = "55be7e619e1b2193ab59b03d04625cd3c1ce3e86cd58ea3bb63f171c9005b8a2"
GLYPHS_URL = (
    f"https://github.com/ryanoasis/nerd-fonts/raw/refs/tags/v{NF_VERSION}/src/glyphs"
)

type PatchSetAttributes = dict[Literal["default"] | int, PatchSetAttributeEntry]
type ResolvedSymbol = PatchSetAttributes | PatchSetScaleRules | int | None


class PatchSetScaleRules(TypedDict):
    ShiftMode: str
    ScaleGroups: list[list[int] | range]


class PatchSetAttributeEntry(TypedDict):
    align: str
    valign: str
    stretch: str
    params: dict[str, float | bool]

    relative_x: float
    relative_y: float
    relative_width: float
    relative_height: float


class PatchSet(TypedDict):
    Name: str
    Filename: str
    Exact: bool
    SymStart: int
    SymEnd: int
    SrcStart: int | None
    ScaleRules: PatchSetScaleRules | None
    Attributes: PatchSetAttributes


class PatchSetExtractor(ast.NodeVisitor):
    def __init__(self) -> None:
        self.symbol_table: dict[str, ast.expr] = {}
        self.patch_set_values: list[PatchSet] = []
        self.nf_version: str = ""

    def visit_Assign(self, node):
        if (
            node.col_offset == 0
            and len(node.targets) == 1
            and isinstance(node.targets[0], ast.Name)
            and node.targets[0].id == "version"
        ):
            self.nf_version = ast.literal_eval(node.value)
        else:
            return self.generic_visit(node)

    def visit_ClassDef(self, node: ast.ClassDef) -> None:
        if node.name != "font_patcher":
            return
        for item in node.body:
            if isinstance(item, ast.FunctionDef) and item.name == "setup_patch_set":
                self.visit_setup_patch_set(item)

    def visit_setup_patch_set(self, node: ast.FunctionDef) -> None:
        for stmt in node.body:
            match stmt:
                case ast.Assign(targets=[ast.Name(id=symbol)]):
                    self.symbol_table[symbol] = stmt.value

        for stmt in node.body:
            if not isinstance(stmt, ast.Assign):
                continue
            for target in stmt.targets:
                if (
                    isinstance(target, ast.Attribute)
                    and target.attr == "patch_set"
                    and isinstance(stmt.value, ast.List)
                ):
                    for elt in stmt.value.elts:
                        if isinstance(elt, ast.Dict):
                            self.process_patch_entry(elt)

    def resolve_symbol(self, node: ast.expr) -> ResolvedSymbol:
        if isinstance(node, ast.Name) and node.id in self.symbol_table:
            return self.safe_literal_eval(self.symbol_table[node.id])
        return self.safe_literal_eval(node)

    def safe_literal_eval(self, node: ast.expr) -> ResolvedSymbol:
        try:
            return ast.literal_eval(node)
        except ValueError:
            # font-patcher builds some values from expressions, such as
            # ranges and flags. The file is pinned by SHA-256 above, so its
            # expressions are trusted in the same way as Ghostty trusts them.
            return eval(
                ast.unparse(node),
                {
                    "basics_enabled": True,
                    # The symbols-only font carries neither box drawing nor braille.
                    "box_enabled": False,
                    "box_keep": False,
                    "braille_enabled": False,
                },
                {
                    "self": SimpleNamespace(
                        args=SimpleNamespace(
                            careful=False,
                            custom=False,
                            fontawesome=True,
                            fontawesomeextension=True,
                            fontlogos=True,
                            octicons=True,
                            codicons=True,
                            powersymbols=True,
                            pomicons=True,
                            powerline=True,
                            powerlineextra=True,
                            material=True,
                            weather=True,
                        )
                    ),
                },
            )

    def process_patch_entry(self, dict_node: ast.Dict) -> None:
        entry = {}
        for key_node, value_node in zip(dict_node.keys, dict_node.values):
            if isinstance(key_node, ast.Constant):
                if key_node.value == "Enabled":
                    if self.safe_literal_eval(value_node):
                        continue
                    return
                key = ast.literal_eval(cast("ast.Constant", key_node))
                entry[key] = self.resolve_symbol(value_node)
        self.patch_set_values.append(cast("PatchSet", entry))


def fetch(url: str, target: Path, sha256: str | None = None) -> Path:
    if not target.exists():
        target.parent.mkdir(parents=True, exist_ok=True)
        print(f"Info: downloading {url}")
        partial = target.with_suffix(target.suffix + ".partial")
        urlretrieve(url, partial)
        partial.rename(target)
    if sha256 is not None:
        actual = hashlib.sha256(target.read_bytes()).hexdigest()
        if actual != sha256:
            raise ValueError(f"{target} has SHA-256 {actual}, expected {sha256}")
    return target


def codepoint_tables(
    patch_sets: list[PatchSet], nerd_font: TTFont, cache: Path
) -> dict[str, dict[int, int]]:
    """Maps each patch set's original code points to Nerd Font code points."""
    cp_tables: dict[str, dict[int, int]] = {}
    cp_nerdfont_used: set[int] = set()
    cmap = nerd_font.getBestCmap()
    for entry in patch_sets:
        name = entry["Name"]
        source = fetch(
            f"{GLYPHS_URL}/{entry['Filename']}",
            cache / "glyphs" / entry["Filename"],
        )
        try:
            with TTFont(source) as patchfont:
                patch_cmap = patchfont.getBestCmap()
        except TTLibError:
            # Not a TTF/OTF font, which an exact patch set allows.
            patch_cmap = None

        # An exact set keeps its code points. Otherwise SymStart..SymEnd in
        # the source font map in order onto a packed range from SrcStart.
        if entry["Exact"]:
            assert entry["SrcStart"] is None
            cp_nerdfont = 0
        else:
            assert entry["SrcStart"]
            assert patch_cmap is not None
            cp_nerdfont = entry["SrcStart"] - 1

        table = cp_tables.setdefault(name, {})
        for cp_original in range(entry["SymStart"], entry["SymEnd"] + 1):
            if patch_cmap and cp_original not in patch_cmap:
                continue
            if not entry["Exact"]:
                cp_nerdfont += 1
            else:
                cp_nerdfont = cp_original
            if cp_nerdfont not in cmap:
                raise ValueError(
                    f"Missing code point {cp_nerdfont:#x} in patch set '{name}'"
                )
            if cp_nerdfont in cp_nerdfont_used:
                raise ValueError(f"Overlap at {cp_nerdfont:#x} in patch set '{name}'")
            table[cp_original] = cp_nerdfont
            cp_nerdfont_used.add(cp_nerdfont)
    return cp_tables


def collect_entries(
    patch_sets: list[PatchSet], nerd_font: TTFont, cache: Path
) -> dict[int, PatchSetAttributeEntry]:
    cmap = nerd_font.getBestCmap()
    glyphs = nerd_font.getGlyphSet()
    cp_tables = codepoint_tables(patch_sets, nerd_font, cache)

    def bounds_of(cp: int) -> tuple[float, float, float, float]:
        pen = BoundsPen(glyphSet=glyphs)
        glyphs[cmap[cp]].draw(pen)
        return pen.bounds

    entries: dict[int, PatchSetAttributeEntry] = {}
    for entry in patch_sets:
        name = entry["Name"]
        attributes = entry["Attributes"]
        patch_set_entries: dict[int, PatchSetAttributeEntry] = {}

        cp_table = cp_tables[name]
        for cp_original in range(entry["SymStart"], entry["SymEnd"] + 1):
            if cp_original not in cp_table:
                continue
            cp_nerdfont = cp_table[cp_original]
            if cp_nerdfont in entries:
                raise ValueError(f"Overlap at {cp_nerdfont:#x} in patch set '{name}'")
            source = attributes.get(cp_original, attributes["default"])
            patch_set_entries[cp_nerdfont] = cast("PatchSetAttributeEntry", dict(source))

        if entry["ScaleRules"] is not None:
            for group in entry["ScaleRules"]["ScaleGroups"]:
                x_min = y_min = math.inf
                x_max = y_max = -math.inf
                individual: dict[int, tuple[float, float, float, float]] = {}
                advances: set[float] = set()
                for cp_original in group:
                    if cp_original not in cp_table:
                        # One progress-indicator group names a glyph that is
                        # not in the font only to pad the group vertically;
                        # any stretched glyph with overlap, such as U+E0B0,
                        # gives the same vertical bounds.
                        if name == "Progress Indicators" and cp_original == 0xEDFF:
                            bounds = bounds_of(0xE0B0)
                            y_min = min(bounds[1], y_min)
                            y_max = max(bounds[3], y_max)
                        continue
                    cp_nerdfont = cp_table[cp_original]
                    advances.add(glyphs[cmap[cp_nerdfont]].width)
                    bounds = bounds_of(cp_nerdfont)
                    individual[cp_nerdfont] = bounds
                    x_min = min(bounds[0], x_min)
                    y_min = min(bounds[1], y_min)
                    x_max = max(bounds[2], x_max)
                    y_max = max(bounds[3], y_max)
                group_width = x_max - x_min
                group_height = y_max - y_min
                monospace = len(individual) > 1 and len(advances) == 1
                for cp_original in group:
                    if cp_original not in cp_table:
                        continue
                    cp_nerdfont = cp_table[cp_original]
                    # Groups may cross patch sets, and a glyph takes the
                    # first group it appears in.
                    if (
                        cp_nerdfont not in patch_set_entries
                        or "relative_height" in patch_set_entries[cp_nerdfont]
                    ):
                        continue
                    this = individual[cp_nerdfont]
                    attr = patch_set_entries[cp_nerdfont]
                    attr["relative_height"] = (this[3] - this[1]) / group_height
                    attr["relative_y"] = (this[1] - y_min) / group_height
                    # Horizontal positions only group when every glyph in
                    # the group has the same advance.
                    if monospace:
                        attr["relative_width"] = (this[2] - this[0]) / group_width
                        attr["relative_x"] = (this[0] - x_min) / group_width
        entries |= patch_set_entries
    return entries


def alignment(value: str) -> str | None:
    return {
        "l": "Align::Start",
        "r": "Align::End",
        "c": "Align::Center1",
        "": None,
    }.get(value, "Align::None")


def rust_float(value: float) -> str:
    text = repr(float(value))
    return text if "." in text or "e" in text else f"{text}.0"


def constraint_literal(attr: PatchSetAttributeEntry) -> str:
    stretch = attr.get("stretch", "")
    params = attr.get("params", {})
    overlap = float(params.get("overlap", 0.0))
    xy_ratio = float(params.get("xy-ratio", -1.0))
    y_padding = float(params.get("ypadding", 0.0))

    fields: list[str] = []
    if "pa" in stretch:
        fields.append(
            "size: Size::Cover" if "!" in stretch or overlap else "size: Size::FitCover1"
        )
    elif "xy" in stretch:
        fields.append("size: Size::Stretch")
    else:
        raise ValueError(f"unknown stretch rule {stretch!r}")

    # `^` scales against the full line height rather than the icon height.
    if "^" not in stretch:
        fields.append("height: Height::Icon")

    # A `1`, or a rule other than `pa` without an explicit `2`, limits the
    # constraint to one cell.
    if "1" in stretch or ("pa" not in stretch and "2" not in stretch):
        fields.append("max_constraint_width: 1")

    if (align := alignment(attr.get("align", ""))) is not None:
        fields.append(f"align_horizontal: {align}")
    if (valign := alignment(attr.get("valign", ""))) is not None:
        fields.append(f"align_vertical: {valign}")

    for key, default in (
        ("relative_width", 1.0),
        ("relative_height", 1.0),
        ("relative_x", 0.0),
        ("relative_y", 0.0),
    ):
        value = float(attr.get(key, default))
        if value != default:
            fields.append(f"{key}: {rust_float(value)}")

    # font-patcher asserts that overlap and ypadding never combine.
    if overlap:
        pad = -overlap / 2
        v_pad = -min(0.01, overlap) / 2
        fields += [
            f"pad_left: {rust_float(pad)}",
            f"pad_right: {rust_float(pad)}",
            f"pad_top: {rust_float(v_pad)}",
            f"pad_bottom: {rust_float(v_pad)}",
        ]
    elif y_padding:
        fields += [
            f"pad_top: {rust_float(y_padding / 2)}",
            f"pad_bottom: {rust_float(y_padding / 2)}",
        ]
    if xy_ratio > 0:
        fields.append(f"max_xy_ratio: Some({rust_float(xy_ratio)})")

    body = "".join(f"        {field},\n" for field in fields)
    return f"    Constraint {{\n{body}        ..Constraint::DEFAULT\n    }}"


def emit_rust(entries: dict[int, PatchSetAttributeEntry]) -> str:
    literals: list[str] = []
    index_of: dict[str, int] = {}
    ranges: list[list[int]] = []
    for cp in sorted(entries):
        literal = constraint_literal(entries[cp])
        index = index_of.get(literal)
        if index is None:
            index = index_of[literal] = len(literals)
            literals.append(literal)
        if ranges and ranges[-1][1] == cp - 1 and ranges[-1][2] == index:
            ranges[-1][1] = cp
        else:
            ranges.append([cp, cp, index])
    if len(literals) > 0xFFFF:
        raise ValueError("too many distinct constraints for u16 indices")
    print(f"Info: {len(entries)} code points, {len(ranges)} ranges, {len(literals)} constraints")
    range_lines = "".join(
        f"    (0x{start:05X}, 0x{end:05X}, {index}),\n" for start, end, index in ranges
    )
    return f"""// @generated by scripts/nerd-font-constraints.py from Nerd Fonts
// {NF_VERSION} font-patcher rules and the bundled Symbols Nerd Font. Run
// `mise run nerd-fonts:constraints` to regenerate it; do not edit it by hand.
//
// The format and generator are adapted from Ghostty's nerd_font_tables.zig
// and nerd_font_codegen.py, copyright (c) 2024 Mitchell Hashimoto, Ghostty
// contributors, under MIT; the rules are the Nerd Fonts font-patcher's,
// copyright (c) 2014 Ryan L McIntyre, under MIT. See
// third-party/nerd-font-constraints/README.md.

use super::{{Align, Constraint, Height, Size}};

/// The Nerd Fonts release these constraints were generated from, which a
/// test checks against the bundled font.
#[cfg(test)]
pub(super) const NERD_FONTS_VERSION: &str = "{NF_VERSION}";

/// Distinct constraints, indexed by `RANGES`.
pub(super) static CONSTRAINTS: [Constraint; {len(literals)}] = [
{",\n".join(literals)},
];

/// Sorted, disjoint, inclusive code point ranges and their constraint.
pub(super) static RANGES: [(u32, u32, u16); {len(ranges)}] = [
{range_lines}];
"""


def main() -> None:
    font_path, cache_dir, out_path = (Path(arg) for arg in sys.argv[1:4])
    cache = cache_dir / NF_VERSION
    patcher = fetch(PATCHER_URL, cache / "font-patcher", PATCHER_SHA256)
    patch_sets_extractor = PatchSetExtractor()
    patch_sets_extractor.visit(ast.parse(patcher.read_text(encoding="utf-8")))
    if patch_sets_extractor.nf_version != NF_VERSION:
        raise ValueError(
            f"font-patcher reports {patch_sets_extractor.nf_version}, expected {NF_VERSION}"
        )

    nerd_font = TTFont(font_path)
    version = nerd_font["name"].getDebugName(5) or ""
    if f"Nerd Fonts {NF_VERSION}" not in version:
        raise ValueError(f"{font_path} is {version!r}, not Nerd Fonts {NF_VERSION}")

    entries = collect_entries(patch_sets_extractor.patch_set_values, nerd_font, cache)
    out_path.write_text(emit_rust(entries), encoding="utf-8")
    print(f"Info: wrote {out_path}")


if __name__ == "__main__":
    main()
