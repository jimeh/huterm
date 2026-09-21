"""Temporary bounded scroll admission experiment; diagnostic branch only."""
from pathlib import Path

def edit(name, old, new):
    p = Path(name)
    text = p.read_text()
    assert text.count(old) == 1, (name, old, text.count(old))
    p.write_text(text.replace(old, new))

refresh = "crates/huterm-gpui/src/desktop/refresh.rs"
edit(refresh, "    available: bool,", "    available: bool,\n    scroll_available: bool,")
edit(refresh, "Self { available: true }", "Self { available: true, scroll_available: true }")
edit(refresh, "fn admits(&self, mode: RefreshMode) -> bool", "fn admits(&self, mode: RefreshMode, scrolling: bool) -> bool")
edit(refresh, "mode == RefreshMode::Unlimited || self.available", "mode == RefreshMode::Unlimited || self.available || (scrolling && self.scroll_available)")
edit(refresh, "        self.available = false;", "        if self.available { self.available = false; } else { self.scroll_available = false; }")
edit(refresh, "        self.available = true;", "        self.available = true;\n        self.scroll_available = true;")
p = Path(refresh)
p.write_text(p.read_text().replace("admits(RefreshMode::Display)", "admits(RefreshMode::Display, false)").replace("admits(RefreshMode::Unlimited)", "admits(RefreshMode::Unlimited, false)"))
edit("crates/huterm-gpui/src/scroll.rs", "    pub(super) fn desired(&self) -> usize {", "    pub(super) fn pending_scroll(&self) -> bool { self.pending_scroll.is_some() }\n\n    pub(super) fn desired(&self) -> usize {")
edit("crates/huterm-gpui/src/desktop.rs", "self.snapshot_pacer.admits(self.refresh_mode)", "self.snapshot_pacer.admits(self.refresh_mode, self.scroll.pending_scroll())")
