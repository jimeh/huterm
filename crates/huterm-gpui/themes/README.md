# Bundled themes

These palettes are adapted to Huterm's supported color roles. Normal and bright
ANSI colors, foreground, background, cursor, and selection colors are imported.
Extra indexed colors, dim palettes, search, and application-specific roles are
not imported. Tokyo Night's Alacritty exports omit cursor and selection colors;
Huterm uses its foreground for the cursor, blue for selection, and background
for selected text.

## Sources

- [chriskempson/tomorrow-theme](https://github.com/chriskempson/tomorrow-theme/tree/ccf6666d888198d341b26b3a99d0bc96500ad503),
  revision `ccf6666d888198d341b26b3a99d0bc96500ad503`;
  [license](licenses/chriskempson.txt).

- [Binaryify/OneDark-Pro](https://github.com/Binaryify/OneDark-Pro/tree/54c3280b29f2c2ed9751e5ca4e071380b7b42205),
  revision `54c3280b29f2c2ed9751e5ca4e071380b7b42205`;
  [license](licenses/binaryify.txt).

- [catppuccin/alacritty](https://github.com/catppuccin/alacritty/tree/f6cb5a5c2b404cdaceaff193b9c52317f62c62f7)
  — revision `f6cb5a5c2b404cdaceaff193b9c52317f62c62f7`;
  [license](licenses/catppuccin.txt).
- [folke/tokyonight.nvim](https://github.com/folke/tokyonight.nvim/tree/cdc07ac78467a233fd62c493de29a17e0cf2b2b6)
  — revision `cdc07ac78467a233fd62c493de29a17e0cf2b2b6`;
  [license](licenses/folke.txt).
- [dracula/alacritty](https://github.com/dracula/alacritty/tree/c8a3a13404b78d520d04354e133b5075d9b785e1)
  — revision `c8a3a13404b78d520d04354e133b5075d9b785e1`;
  [license](licenses/dracula.txt).
- [nordtheme/alacritty](https://github.com/nordtheme/alacritty/tree/9949642f3903e8fcb62bfc03f09410e3d78440c2)
  — revision `9949642f3903e8fcb62bfc03f09410e3d78440c2`;
  [license](licenses/nordtheme.txt).

Huterm Dark is Huterm's original Tomorrow Night–based palette, unchanged.
`tomorrow-night` follows the upstream iTerm2 port, including its gray cursor,
`#373b41` selection background, and identical normal and bright ANSI palettes.
The upstream terminal ports differ; this import keeps the iTerm2 colors together.
`one-dark-pro` uses One Dark Pro's terminal ANSI, background, and foreground
colors, with its blue editor cursor. Its translucent terminal selection color
`#abb2bf30` is composited over `#282c34` to produce opaque `#41454e`.
`tango-with-monokai` is Jim's custom palette imported from
`~/.dotfiles/config/ghostty/themes/tango-with-monokai`. Its 16 ANSI colors
match the iTerm version; cursor and selection colors follow Ghostty.

The theme TOML files are modified conversions, not original upstream files.
Keep the upstream license notices with binary distributions.
