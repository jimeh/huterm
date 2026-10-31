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
- [projekt0n/github-nvim-theme](https://github.com/projekt0n/github-nvim-theme/tree/c106c9472154d6b2c74b74565616b877ae8ed31d),
  revision `c106c9472154d6b2c74b74565616b877ae8ed31d`;
  [license](licenses/projekt0n.txt).
- [altercation/solarized](https://github.com/altercation/solarized/tree/62f656a02f93c5190a8753159e34b385588d5ff3),
  revision `62f656a02f93c5190a8753159e34b385588d5ff3`;
  [license](licenses/altercation.txt).
- [ellisonleao/gruvbox.nvim](https://github.com/ellisonleao/gruvbox.nvim/tree/154eb5ff5b96d0641307113fa385eaf0d36d9796),
  revision `154eb5ff5b96d0641307113fa385eaf0d36d9796`;
  [license](licenses/ellisonleao.txt).
- [rose-pine/alacritty](https://github.com/rose-pine/alacritty/tree/a6f7c8245e5bba639befe52e1e025f84ba8b3ee5),
  revision `a6f7c8245e5bba639befe52e1e025f84ba8b3ee5`;
  [license](licenses/rose-pine.txt).
- [ayu-theme/ayu-colors](https://github.com/ayu-theme/ayu-colors/tree/d461f02983546636bc6e1e0c48190fdb74036426),
  revision `d461f02983546636bc6e1e0c48190fdb74036426`;
  [license](licenses/ayu-theme.txt).
- [navarasu/onedark.nvim](https://github.com/navarasu/onedark.nvim/tree/df4792accde9db0043121f32628bcf8e645d9aea),
  revision `df4792accde9db0043121f32628bcf8e645d9aea`;
  [license](licenses/navarasu.txt).
- [sainnhe/everforest](https://github.com/sainnhe/everforest/tree/85a86eb62409e3ec88713bff3d1b9d7374e112e4),
  revision `85a86eb62409e3ec88713bff3d1b9d7374e112e4`;
  [license](licenses/sainnhe.txt).
- [rebelot/kanagawa.nvim](https://github.com/rebelot/kanagawa.nvim/tree/bb85e4bfc8d89b0e62c8fa53ccdd13d12e2f77b3),
  revision `bb85e4bfc8d89b0e62c8fa53ccdd13d12e2f77b3`;
  [license](licenses/rebelot.txt).

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

`github-light` and `github-dark` come from the Neovim theme's `github_light`
and `github_dark_default` palettes, GitHub's default light and dark looks,
which have no terminal export. (`github_light` and `github_light_default`
resolve to the same colors; `github_dark` uses a lighter `#30363d` canvas.)
Their ANSI colors are the theme's `terminal_color_*` mapping, the cursor is
the foreground as in its `Cursor` group, and the selection is the `Visual`
color that the theme itself blends opaque over the background: `#dae9f9`
light and `#17335a` dark. The palette scales come from GitHub Primer
primitives bundled in that repository, whose MIT notice
`licenses/projekt0n.txt` also carries. As upstream maps it, dark black
matches the `#0d1117` background.
`solarized-light` and `solarized-dark` follow the iTerm2 port's roles: one
ANSI palette for both modes, the body text color for the cursor, and base2 or
base02 for selection. The iTerm2 files store calibrated color-space values, so
the hex values are Solarized's canonical sRGB colors from its Xresources port.
As upstream intends, dark bright black matches the background.
`gruvbox-light` and `gruvbox-dark` use gruvbox.nvim's default medium contrast
and its own `terminal_color_*` mapping. Its reversed cursor becomes the
foreground, and its `Visual` group sets no text color.
ayu-colors ships no terminal configuration file. `ayu-*` uses the `terminal`
and `editor` colors that it generates from `themes/*.yaml`, read from its `ayu`
9.1.0 npm package. That package was built from a commit whose theme files match
the pinned revision. The translucent editor selections `#035bd626`, `#3388ff40`,
and `#409fff40` are composited over each editor background to produce `#d7e4f6`,
`#193155`, and `#2b4768`. Ayu defines no cursor color, so the foreground is
used. Its dark and mirage terminal palettes set black and bright black to
`#0a0000`, which leaves shell comments and suggestions invisible, so Huterm
uses each palette's syntax comment color for bright black instead: `#5a6673`
dark and `#6e7c8f` mirage.
`one-light` is onedark.nvim's `light` style with its `term_colors` mapping;
its reversed cursor becomes the foreground.
`everforest-light` and `everforest-dark` use the medium background and the
colorscheme's terminal mapping, which repeats the normal colors as bright
colors. Everforest leaves the cursor unset by default, so the foreground is
used.
`kanagawa-*` follows the Ghostty extras, which, unlike the Alacritty extras,
include cursor colors.
Solarized, Rosé Pine, and Rosé Pine Moon have no color darker than their dark
backgrounds, so `solarized-dark`, `rose-pine`, and `rose-pine-moon` use
hand-picked darker tab bars.

## Window chrome colors

Themes may set six flat keys for Huterm's window chrome: `tab_bar_background`,
`tab_active_background`, `tab_foreground`, `tab_inactive_foreground`,
`tab_border`, and `tab_accent`. Like palette colors, they inherit through
`extends`; a definition without `extends` that sets any key derives the chrome
it leaves unset from its own colors instead of inheriting Huterm Dark's, while
an entirely empty definition is Huterm Dark itself. Every bundled theme
and Huterm Dark set all six, chosen by hand from or to match each palette;
they are Huterm additions, not upstream colors.

A theme that omits a key derives it from the resolved palette. Dark themes get a
bar darker than the background, and near-black or light themes get a bar that
stays distinguishable from the terminal. The active tab background lightens the
terminal background, inactive text and borders mix the foreground with the bar,
and the accent is ANSI blue. An explicit bar color also drives the derived
inactive text and border colors.

Overriding only `background` or `foreground` on top of a bundled theme keeps
that theme's chrome colors, including on `tokyo-night`, the theme a config
without `[theme] name` uses. Set the chrome keys too when they should follow.

Three more optional keys color overlays and accept `#rrggbb` or `#rrggbbaa`:
`tab_hover_background` for hovered tabs and chrome controls, and
`scrollbar_thumb` and `scrollbar_track` for every overlay scrollbar, in the
terminal, the command palette list, and the tab bars. Unset keys derive from
the resolved `foreground` at fixed alphas: about 4 percent for hover, 73
percent for the thumb, and 8 percent for the track. Bundled themes leave them
derived.

The theme TOML files are modified conversions, not original upstream files.
Keep the upstream license notices with binary distributions.
