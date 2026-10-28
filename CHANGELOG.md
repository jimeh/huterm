# Changelog

## [0.23.0](https://github.com/jimeh/huterm/compare/v0.22.0...v0.23.0) (2026-10-28)


### Features

* arrange, cycle, and balance pane layouts ([#286](https://github.com/jimeh/huterm/issues/286)) ([745c98a](https://github.com/jimeh/huterm/commit/745c98a397b95b56f98d3fb9b834e7a16bff1734))
* compress idle scrollback history ([#288](https://github.com/jimeh/huterm/issues/288)) ([1a9af85](https://github.com/jimeh/huterm/commit/1a9af856288dc18e1412330b1eb5be2d77d2e6ce))

## [0.22.0](https://github.com/jimeh/huterm/compare/v0.21.0...v0.22.0) (2026-10-27)


### Features

* edit keybindings in Settings ([#274](https://github.com/jimeh/huterm/issues/274)) ([c9c9be1](https://github.com/jimeh/huterm/commit/c9c9be151b31903bb68f2316ede72dcf7e2e3ab2))
* edit quake profiles and global shortcuts in Settings ([#275](https://github.com/jimeh/huterm/issues/275)) ([f0b0174](https://github.com/jimeh/huterm/commit/f0b0174c5c25033cd5bbddde49aefa2fc11854e8))
* edit settings in a Settings window ([#269](https://github.com/jimeh/huterm/issues/269)) ([10abd56](https://github.com/jimeh/huterm/commit/10abd56fb5b9e95d7111f3471190fe9ad61218ae))
* undo and redo pane moves and tab drops ([#281](https://github.com/jimeh/huterm/issues/281)) ([010f12f](https://github.com/jimeh/huterm/commit/010f12ff7ead2a63d18d2007fb20346734e2429d))


### Bug Fixes

* update Ghostty to 1.4.0 ([#280](https://github.com/jimeh/huterm/issues/280)) ([5b6699f](https://github.com/jimeh/huterm/commit/5b6699f25854329ad398e014931251bd674399af))

## [0.21.0](https://github.com/jimeh/huterm/compare/v0.20.0...v0.21.0) (2026-10-27)


### Features

* add commands to swap, move, and extract panes ([#262](https://github.com/jimeh/huterm/issues/262)) ([3212fb4](https://github.com/jimeh/huterm/commit/3212fb4c8d4c038ba42f6370452103b0fbd433bf))
* add pane layout contexts for keybindings ([#255](https://github.com/jimeh/huterm/issues/255)) ([42104c0](https://github.com/jimeh/huterm/commit/42104c051cfd702234d239299a82213118a3f525))
* allow up to 100,000 rows of scrollback ([#268](https://github.com/jimeh/huterm/issues/268)) ([8449bf9](https://github.com/jimeh/huterm/commit/8449bf90543c27fc7852c45662c8a5546b3d01d2))
* drag panes and tabs into splits, with spring-loaded tabs ([#264](https://github.com/jimeh/huterm/issues/264)) ([3ba4ac0](https://github.com/jimeh/huterm/commit/3ba4ac0751b40a0723e8494f37744680e8bdf81a))
* reload the configuration when it changes on disk ([#263](https://github.com/jimeh/huterm/issues/263)) ([c67aeb0](https://github.com/jimeh/huterm/commit/c67aeb0f2bb3599b45f860eb4c4b751435067a63))
* show open source licenses from the About panel ([#259](https://github.com/jimeh/huterm/issues/259)) ([19c2c1a](https://github.com/jimeh/huterm/commit/19c2c1aebf26b34a3ec5318b50f4451803dcb3b0))


### Bug Fixes

* stop the command palette crashing on Resize Pane and Close Tab ([#265](https://github.com/jimeh/huterm/issues/265)) ([20213b9](https://github.com/jimeh/huterm/commit/20213b9de58c518858163340fe9cadaabcfb5e7e))

## [0.20.0](https://github.com/jimeh/huterm/compare/v0.19.1...v0.20.0) (2026-10-26)


### Features

* make scrollback configurable by rows and memory size ([#243](https://github.com/jimeh/huterm/issues/243)) ([a16765d](https://github.com/jimeh/huterm/commit/a16765da4a59ac13130f67e3b50acb022f245a26))
* resize and zoom split panes ([#250](https://github.com/jimeh/huterm/issues/250)) ([ea43dab](https://github.com/jimeh/huterm/commit/ea43dab1818b304a80d121468d76f0432a6865bc))
* split, focus, and close panes in the desktop client ([#249](https://github.com/jimeh/huterm/issues/249)) ([02a7c17](https://github.com/jimeh/huterm/commit/02a7c17fdad8613b6766ea8e4cf5e2548b021264))

## [0.19.1](https://github.com/jimeh/huterm/compare/v0.19.0...v0.19.1) (2026-10-26)


### Bug Fixes

* close emptied windows without stopping terminals and ignore stale Confirm clicks ([#238](https://github.com/jimeh/huterm/issues/238)) ([b4fe19c](https://github.com/jimeh/huterm/commit/b4fe19c72f3400b73f915d1ae8324f0e632f5ea4))
* keep terminal history when a shell exits mid-reply, and end a crashed terminal's processes ([#237](https://github.com/jimeh/huterm/issues/237)) ([5fff877](https://github.com/jimeh/huterm/commit/5fff877bc8f134edfce7a23be22dd4f487b6accb))

## [0.19.0](https://github.com/jimeh/huterm/compare/v0.18.0...v0.19.0) (2026-10-25)


### Features

* map terminal views against the grid they display ([#225](https://github.com/jimeh/huterm/issues/225)) ([fe7c3d6](https://github.com/jimeh/huterm/commit/fe7c3d6c3800163b06d586905d9481026d82d6d3))
* show one terminal in several desktop windows ([#226](https://github.com/jimeh/huterm/issues/226)) ([475b1f4](https://github.com/jimeh/huterm/commit/475b1f402e356b8585aecb598556444ed6a6e793))


### Bug Fixes

* keep rewritten OSC 8 links from redrawing the whole terminal ([#235](https://github.com/jimeh/huterm/issues/235)) ([792f4f0](https://github.com/jimeh/huterm/commit/792f4f00689d00da779e8eb7ef5b2a62c9bfd74c))
* reject Ghostty source that does not match the pinned manifest ([#234](https://github.com/jimeh/huterm/issues/234)) ([cde18ce](https://github.com/jimeh/huterm/commit/cde18ce6660e3c569380540add14747104179d95))

## [0.18.0](https://github.com/jimeh/huterm/compare/v0.17.0...v0.18.0) (2026-10-23)


### Features

* show release notes for every version a macOS update brings ([#224](https://github.com/jimeh/huterm/issues/224)) ([67a9849](https://github.com/jimeh/huterm/commit/67a98495dd335bf3afa4a5d4a06202dc13c3c201))

## [0.17.0](https://github.com/jimeh/huterm/compare/v0.16.0...v0.17.0) (2026-10-08)


### Features

* give each terminal view its own runtime viewer ([#208](https://github.com/jimeh/huterm/issues/208)) ([8dfb406](https://github.com/jimeh/huterm/commit/8dfb406ca09da4c8d9481ab08a1f3c3ee6503f42))


### Bug Fixes

* show each command failure in one window instead of replaying it ([#214](https://github.com/jimeh/huterm/issues/214)) ([3abee97](https://github.com/jimeh/huterm/commit/3abee97d0a98aa52e98d581f71d982bd965dc0c0))

## [0.16.0](https://github.com/jimeh/huterm/compare/v0.15.1...v0.16.0) (2026-10-01)


### Features

* project runtime hierarchy events into the desktop client ([#199](https://github.com/jimeh/huterm/issues/199)) ([e87c608](https://github.com/jimeh/huterm/commit/e87c60846edb66f51319f71ef860a560c49622c2))


### Bug Fixes

* remove races behind intermittent Linux desktop smoke failures ([#203](https://github.com/jimeh/huterm/issues/203)) ([7f8f239](https://github.com/jimeh/huterm/commit/7f8f239b4cfeb6e74435df3c03d6678d955323c4))

## [0.15.1](https://github.com/jimeh/huterm/compare/v0.15.0...v0.15.1) (2026-09-29)


### Performance Improvements

* cut per-frame GPUI paint and scene sort cost for terminal grids ([#192](https://github.com/jimeh/huterm/issues/192)) ([15ee8a0](https://github.com/jimeh/huterm/commit/15ee8a0b22f1626c8f3358c5c80e6c1b3cf762db))

## [0.15.0](https://github.com/jimeh/huterm/compare/v0.14.1...v0.15.0) (2026-09-29)


### Features

* bind libghostty-vt through Huterm's own huterm-ghostty crate ([#184](https://github.com/jimeh/huterm/issues/184)) ([b47c4df](https://github.com/jimeh/huterm/commit/b47c4df52aa7f81416b1e31b33d03bd32d205970))
* support Kitty clipboard protocol writes ([#185](https://github.com/jimeh/huterm/issues/185)) ([75152ff](https://github.com/jimeh/huterm/commit/75152ffb5a7eaeba800c7d3d774df1b27314ad69))

## [0.14.1](https://github.com/jimeh/huterm/compare/v0.14.0...v0.14.1) (2026-09-27)


### Bug Fixes

* stop a click behind a just-opened dialog from focusing the terminal ([#180](https://github.com/jimeh/huterm/issues/180)) ([fa5a419](https://github.com/jimeh/huterm/commit/fa5a4192c303a704416804a703ae40edd224c9e5))

## [0.14.0](https://github.com/jimeh/huterm/compare/v0.13.2...v0.14.0) (2026-09-27)


### Features

* overhaul dialogs, notices, and window chrome ([#176](https://github.com/jimeh/huterm/issues/176)) ([81262f1](https://github.com/jimeh/huterm/commit/81262f1d66e53b8f73c16a7194db252fc473583b))

## [0.13.2](https://github.com/jimeh/huterm/compare/v0.13.1...v0.13.2) (2026-09-27)


### Performance Improvements

* make terminal link lookup cost independent of scrollback length ([#172](https://github.com/jimeh/huterm/issues/172)) ([8e38546](https://github.com/jimeh/huterm/commit/8e38546e8a8cfabc91c256244696461fdc6b9093))
* remove the pre-echo snapshot and no-op re-renders from terminal input ([#175](https://github.com/jimeh/huterm/issues/175)) ([b48283a](https://github.com/jimeh/huterm/commit/b48283a7d7f6f1fb699d9929b78fc1da4ce0851c))

## [0.13.1](https://github.com/jimeh/huterm/compare/v0.13.0...v0.13.1) (2026-09-26)


### Bug Fixes

* let Huterm open hundreds of terminals under launchd's descriptor limit ([#171](https://github.com/jimeh/huterm/issues/171)) ([a08b3a9](https://github.com/jimeh/huterm/commit/a08b3a921826ca411ad7345830b4bcf43b180b86))


### Performance Improvements

* reduce terminal snapshot rebuild cost ([#166](https://github.com/jimeh/huterm/issues/166)) ([cdb720d](https://github.com/jimeh/huterm/commit/cdb720dd04f9fec8db76475f7be54288b1df88f6))

## [0.13.0](https://github.com/jimeh/huterm/compare/v0.12.3...v0.13.0) (2026-09-25)


### Features

* add smart tab labels and replace ps with in-process process detection ([#160](https://github.com/jimeh/huterm/issues/160)) ([5be9643](https://github.com/jimeh/huterm/commit/5be964383b20ca1d33bcec97bc8aedf23a0dec36))


### Performance Improvements

* make Quake scheduling event-driven on macOS 14 and Linux ([#159](https://github.com/jimeh/huterm/issues/159)) ([66c003e](https://github.com/jimeh/huterm/commit/66c003ec9949ef9bfe22c284c8dc6897153f5e1b))
* replace fullscreen polling with event-driven scheduling ([#157](https://github.com/jimeh/huterm/issues/157)) ([d96b832](https://github.com/jimeh/huterm/commit/d96b8323254050aceeb46f2e845367dc6385912c))

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
