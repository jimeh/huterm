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

## Window chrome colors

Themes may set six flat keys for Huterm's window chrome: `tab_bar_background`,
`tab_active_background`, `tab_foreground`, `tab_inactive_foreground`,
`tab_border`, and `tab_accent`. Like palette colors, they inherit through
`extends`. Every bundled theme and Huterm Dark set all six, chosen by hand from
or to match each palette; they are Huterm additions, not upstream colors.

A theme that omits a key derives it from the resolved palette. Dark themes get a
bar darker than the background, and near-black or light themes get a bar that
stays distinguishable from the terminal. The active tab background lightens the
terminal background, inactive text and borders mix the foreground with the bar,
and the accent is ANSI blue. An explicit bar color also drives the derived
inactive text and border colors.

Overriding only `background` or `foreground` on top of a bundled theme keeps
that theme's chrome colors. Set the chrome keys too when they should follow.

The theme TOML files are modified conversions, not original upstream files.
Keep the upstream license notices with binary distributions.
