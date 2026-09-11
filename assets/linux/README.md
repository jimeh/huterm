# Huterm for Linux

Huterm requires an X11 or XWayland desktop, a working Vulkan driver, system
fonts, locale data, and XKB keyboard data. The archive keeps glibc, XCB core,
the Vulkan loader, Mesa, and hardware drivers under system package management.

Run Huterm from the extracted directory:

```sh
./bin/huterm
```

If archive permissions were lost while copying files, restore the executable
bit with `chmod +x bin/huterm`.

For per-user desktop integration, copy the metadata and icon, then change the
desktop entry's `Exec` value to the absolute path of the extracted executable:

```sh
install -Dm644 share/applications/app.huterm.dev.desktop \
  "$HOME/.local/share/applications/app.huterm.dev.desktop"
install -Dm644 share/metainfo/app.huterm.dev.metainfo.xml \
  "$HOME/.local/share/metainfo/app.huterm.dev.metainfo.xml"
install -Dm644 share/icons/hicolor/512x512/apps/app.huterm.dev.png \
  "$HOME/.local/share/icons/hicolor/512x512/apps/app.huterm.dev.png"
```

AppImage normally uses FUSE. If FUSE is unavailable, run it with:

```sh
APPIMAGE_EXTRACT_AND_RUN=1 ./Huterm-<version>-Linux-<arch>.AppImage
```
