# Changelog

## [0.12.3](https://github.com/jimeh/huterm/compare/v0.12.2...v0.12.3) (2026-09-23)


### Performance Improvements

* target pointer reveal and pending terminal work ([#155](https://github.com/jimeh/huterm/issues/155)) ([84e6fca](https://github.com/jimeh/huterm/commit/84e6fca8f63df1da0e534855d6f419201e565364))

## [0.12.2](https://github.com/jimeh/huterm/compare/v0.12.1...v0.12.2) (2026-09-23)


### Performance Improvements

* drive desktop animations from frames and deadlines ([#153](https://github.com/jimeh/huterm/issues/153)) ([42117b6](https://github.com/jimeh/huterm/commit/42117b6318ff182df0f591045f5c6ed0b5959e30))

## [0.12.1](https://github.com/jimeh/huterm/compare/v0.12.0...v0.12.1) (2026-09-21)


### Performance Improvements

* make terminal activity and runtime waits event-driven ([#147](https://github.com/jimeh/huterm/issues/147)) ([e28924b](https://github.com/jimeh/huterm/commit/e28924bdb717580f5c21d5d57e4f11b43dc19acd))

## [0.12.0](https://github.com/jimeh/huterm/compare/v0.11.2...v0.12.0) (2026-09-18)


### Features

* add terminal metadata and visual bell ([#139](https://github.com/jimeh/huterm/issues/139)) ([ee91a54](https://github.com/jimeh/huterm/commit/ee91a54efd20ff760f7295bcc5875965d8279c08))


### Performance Improvements

* activity-driven snapshots and renderer overhead cuts ([#141](https://github.com/jimeh/huterm/issues/141)) ([776e567](https://github.com/jimeh/huterm/commit/776e5671af18185ab56e691ef2902e0bf88e9020))

## [0.11.2](https://github.com/jimeh/huterm/compare/v0.11.1...v0.11.2) (2026-09-18)


### Bug Fixes

* settle quake activation after fullscreen exits and harden the quake smoke ([#143](https://github.com/jimeh/huterm/issues/143)) ([95c02c1](https://github.com/jimeh/huterm/commit/95c02c13513ffa70caaf251f3a0919f9ebfa4d99))

## [0.11.1](https://github.com/jimeh/huterm/compare/v0.11.0...v0.11.1) (2026-09-18)


### Bug Fixes

* keep quake windows open when non-activating panels take focus ([#140](https://github.com/jimeh/huterm/issues/140)) ([1316a12](https://github.com/jimeh/huterm/commit/1316a12bd8ab403728e39f29d850ec497b6de1e9))

## [0.11.0](https://github.com/jimeh/huterm/compare/v0.10.1...v0.11.0) (2026-09-16)


### ⚠ BREAKING CHANGES

* `[window] tab_position`, `always_show_tab_bar`, and `auto_hide_tab_bar_in_fullscreen` now fail to parse. Each reports an error naming its replacement, for example `window.tab_position moved to tabs.position`, and Huterm starts with default settings until the config is updated.

### Features

* restyle the tab bar with Strip and Pill styles, overlay scrollbars, and notch placement ([#128](https://github.com/jimeh/huterm/issues/128)) ([17c2b22](https://github.com/jimeh/huterm/commit/17c2b227c5694dffa497b2eac2849ebf07606b8a))

## [0.10.1](https://github.com/jimeh/huterm/compare/v0.10.0...v0.10.1) (2026-09-14)


### Bug Fixes

* preserve fullscreen Quake terminal layout ([#124](https://github.com/jimeh/huterm/issues/124)) ([879da3d](https://github.com/jimeh/huterm/commit/879da3daa851f40aee97b9352cc5e88074c996e2))

## [0.10.0](https://github.com/jimeh/huterm/compare/v0.9.0...v0.10.0) (2026-09-13)


### Features

* add retained terminal queries and truecolor terminfo ([#121](https://github.com/jimeh/huterm/issues/121)) ([0bca05b](https://github.com/jimeh/huterm/commit/0bca05bcee4541e08fbe74e4f0f1f6e3d5d3e8a2))

## [0.9.0](https://github.com/jimeh/huterm/compare/v0.8.1...v0.9.0) (2026-09-13)


### Features

* use Ghostty as the only terminal engine ([#119](https://github.com/jimeh/huterm/issues/119)) ([46f26cf](https://github.com/jimeh/huterm/commit/46f26cf7c4a144ebdc90955ba9d122bb2e11364b))

## [0.8.1](https://github.com/jimeh/huterm/compare/v0.8.0...v0.8.1) (2026-09-13)


### Bug Fixes

* **deps:** update grid past unsafe dimension growth ([#116](https://github.com/jimeh/huterm/issues/116)) ([1ebbe22](https://github.com/jimeh/huterm/commit/1ebbe223baaecbeffded1c4f6dfa55756b3ac191))

## [0.8.0](https://github.com/jimeh/huterm/compare/v0.7.3...v0.8.0) (2026-09-13)


### Features

* support OSC 52 clipboard writes from terminal applications ([#114](https://github.com/jimeh/huterm/issues/114)) ([b6ccbee](https://github.com/jimeh/huterm/commit/b6ccbee60c8d20c229c27ccbb9c2f296c2e96a54))


### Bug Fixes

* prevent PTY teardown from altering retained history ([#112](https://github.com/jimeh/huterm/issues/112)) ([da65305](https://github.com/jimeh/huterm/commit/da65305370933519509b90b26d01aa109acb6449))

## [0.7.3](https://github.com/jimeh/huterm/compare/v0.7.2...v0.7.3) (2026-09-13)


### Bug Fixes

* **build:** upgrade Ghostty and Zig for Xcode 27 ([#107](https://github.com/jimeh/huterm/issues/107)) ([178a8c6](https://github.com/jimeh/huterm/commit/178a8c6cfe7cf3ac1a3c5984a5cdb64a38eb7b05))

## [0.7.2](https://github.com/jimeh/huterm/compare/v0.7.1...v0.7.2) (2026-09-13)


### Bug Fixes

* **ci:** forward repository Sparkle signing secret ([#109](https://github.com/jimeh/huterm/issues/109)) ([2d65513](https://github.com/jimeh/huterm/commit/2d65513fdfe048a6e59930990cc46c4c13fbebe9))

## [0.7.1](https://github.com/jimeh/huterm/compare/v0.7.0...v0.7.1) (2026-09-12)


### Bug Fixes

* **ci:** repair and verify release publication tools ([#105](https://github.com/jimeh/huterm/issues/105)) ([9caac95](https://github.com/jimeh/huterm/commit/9caac9522a6b8dc22b5a1fda8651c36af119030c))
* stabilize scroll sampling and palette drag capture ([#103](https://github.com/jimeh/huterm/issues/103)) ([b8f42e3](https://github.com/jimeh/huterm/commit/b8f42e3fdb90fa27489a430df526e7f5c5b05f05))

## [0.7.0](https://github.com/jimeh/huterm/compare/v0.6.0...v0.7.0) (2026-09-12)


### Features

* add command palette and argument pickers ([#91](https://github.com/jimeh/huterm/issues/91)) ([d6327d4](https://github.com/jimeh/huterm/commit/d6327d4c8e7e7f77a22f2af6aee83e06f0f9ab5d))

## [0.6.0](https://github.com/jimeh/huterm/compare/v0.5.0...v0.6.0) (2026-09-12)


### Features

* add native macOS self-updates ([#92](https://github.com/jimeh/huterm/issues/92)) ([7567a20](https://github.com/jimeh/huterm/commit/7567a20d8efc05a1c8fe351c17633c3ee4ab396b))

## [0.5.0](https://github.com/jimeh/huterm/compare/v0.4.0...v0.5.0) (2026-09-11)


### Features

* refresh Huterm app icon ([#93](https://github.com/jimeh/huterm/issues/93)) ([1cd44d6](https://github.com/jimeh/huterm/commit/1cd44d6184372f270e3d16bad5db9e8105d660e3))
* ship native Linux AppImage and tarball releases ([#90](https://github.com/jimeh/huterm/issues/90)) ([72ddd6c](https://github.com/jimeh/huterm/commit/72ddd6c2c32615a17e9c1db65dee17f3a1ef55a0))

## [0.4.0](https://github.com/jimeh/huterm/compare/v0.3.0...v0.4.0) (2026-09-10)


### Features

* add named quake windows and global shortcuts ([#73](https://github.com/jimeh/huterm/issues/73)) ([cba68d6](https://github.com/jimeh/huterm/commit/cba68d634e8d6477a735a15dc925bdb70db13c63))
* adopt the new Huterm app icon ([#74](https://github.com/jimeh/huterm/issues/74)) ([561f054](https://github.com/jimeh/huterm/commit/561f0545feeab7bed17a1e4bc910472f50d09137))

## [0.3.0](https://github.com/jimeh/huterm/compare/v0.2.0...v0.3.0) (2026-09-09)


### Features

* hide single-tab bars and reveal fullscreen overlays ([#69](https://github.com/jimeh/huterm/issues/69)) ([e0ffad2](https://github.com/jimeh/huterm/commit/e0ffad204a98eb2551efccd43e15d6e153f5900c))

## [0.2.0](https://github.com/jimeh/huterm/compare/v0.1.1...v0.2.0) (2026-09-09)


### Features

* publish generated config and theme schemas ([#68](https://github.com/jimeh/huterm/issues/68)) ([67a4eef](https://github.com/jimeh/huterm/commit/67a4eef579ff2ec9d857fb89a18fbd319ba33683))


### Bug Fixes

* unblock tagged releases and allow branch verification builds ([#71](https://github.com/jimeh/huterm/issues/71)) ([62683b5](https://github.com/jimeh/huterm/commit/62683b504fb3a515fe02c29c110275d2cebff21e))

## [0.1.1](https://github.com/jimeh/huterm/compare/v0.1.0...v0.1.1) (2026-09-09)


### Bug Fixes

* authenticate draft release validation with push access ([#66](https://github.com/jimeh/huterm/issues/66)) ([ddfa486](https://github.com/jimeh/huterm/commit/ddfa48686d7b79888ecf694c8cb9086b17f3773c))

## 0.1.0 (2026-09-09)


### Features

* add configurable Ghostty engine and incremental snapshots ([#51](https://github.com/jimeh/huterm/issues/51)) ([bfb14c3](https://github.com/jimeh/huterm/commit/bfb14c3fa43d1b709eabcb98461308970f3b1a6f))
* add configurable native and non-native fullscreen ([#60](https://github.com/jimeh/huterm/issues/60)) ([6f531c6](https://github.com/jimeh/huterm/commit/6f531c682c38409253eed9a26af7fa36a1192b2b))
* add multiple windows with reorderable horizontal and vertical tabs ([#7](https://github.com/jimeh/huterm/issues/7)) ([d1fbff3](https://github.com/jimeh/huterm/commit/d1fbff37fec625ddb4b0ddb55c4c1d468d4b7fa5))
* add Option/Alt-as-Meta and native shortcut checks ([#58](https://github.com/jimeh/huterm/issues/58)) ([1c5d3e8](https://github.com/jimeh/huterm/commit/1c5d3e81d0f9f6c33ad83a696944b2b36f49fe17))
* add scrollback, themes, selection, and macOS packaging ([#4](https://github.com/jimeh/huterm/issues/4)) ([00b987b](https://github.com/jimeh/huterm/commit/00b987bfb9abf1f36fcd7a8d07e61c388b909334))
* add session lifecycle, cancellable quit, and shell-exit policy ([#52](https://github.com/jimeh/huterm/issues/52)) ([9b55c20](https://github.com/jimeh/huterm/commit/9b55c20a7732821bf786a452379e105645459686))
* add session ownership and atomic workspace transfers ([#50](https://github.com/jimeh/huterm/issues/50)) ([9800edc](https://github.com/jimeh/huterm/commit/9800edcd51963a91a11e4eebb74cc64ff3ee6b42))
* add the command catalog and configurable keybindings ([#56](https://github.com/jimeh/huterm/issues/56)) ([39eb5ad](https://github.com/jimeh/huterm/commit/39eb5ad75a7837c604473128d17ee9c63705d045))
* automate signed macOS releases ([#59](https://github.com/jimeh/huterm/issues/59)) ([0042b20](https://github.com/jimeh/huterm/commit/0042b20afc75f9cf58f98190aadcaa858a4bf621))
* build initial GPUI terminal proof of concept ([6f22933](https://github.com/jimeh/huterm/commit/6f22933b57c1ebdcaf39f9665a191be60e265dbc))
* build initial GPUI terminal proof of concept ([83f9ca0](https://github.com/jimeh/huterm/commit/83f9ca005c64bb40a5aace71c325a6507e8d5425))
* support clickable links and native file drops ([#64](https://github.com/jimeh/huterm/issues/64)) ([e30b07b](https://github.com/jimeh/huterm/commit/e30b07b85c5829ef38d4b5dc2ce632ec7a1257c0))
* support terminal application mouse input ([#6](https://github.com/jimeh/huterm/issues/6)) ([f27f908](https://github.com/jimeh/huterm/commit/f27f9085a06155a4f4c85ef067d7f606d17924fe))


### Bug Fixes

* harden terminal runtime and input ([b4facdc](https://github.com/jimeh/huterm/commit/b4facdc567a00124cfcc0ac89f1ca4c4f3802af7))
* render terminal graphics to cell boundaries ([#63](https://github.com/jimeh/huterm/issues/63)) ([2a1a51f](https://github.com/jimeh/huterm/commit/2a1a51fc1bb3d8463c174bee80cc21a65ae4b4b3))
* select compatible macOS SDK stubs for Zig ([#62](https://github.com/jimeh/huterm/issues/62)) ([a208119](https://github.com/jimeh/huterm/commit/a20811992c55380bcc7fc1751c40bdb92f05eb49))


### Performance Improvements

* remove terminal frame throughput bottlenecks ([#2](https://github.com/jimeh/huterm/issues/2)) ([ddb3c3e](https://github.com/jimeh/huterm/commit/ddb3c3ebaf7ba95d67203909ee047db25b1999a2))
