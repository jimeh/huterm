use std::borrow::Cow;

use gpui::{Application, AssetSource, SharedString};

const ASSETS: [(&str, &[u8]); 7] = [
    ("icons/x.svg", include_bytes!("../assets/icons/x.svg")),
    ("icons/plus.svg", include_bytes!("../assets/icons/plus.svg")),
    (
        "icons/chevron-left.svg",
        include_bytes!("../assets/icons/chevron-left.svg"),
    ),
    (
        "icons/chevron-right.svg",
        include_bytes!("../assets/icons/chevron-right.svg"),
    ),
    (
        "icons/chevron-up.svg",
        include_bytes!("../assets/icons/chevron-up.svg"),
    ),
    (
        "icons/chevron-down.svg",
        include_bytes!("../assets/icons/chevron-down.svg"),
    ),
    (
        "icons/circle-alert.svg",
        include_bytes!("../assets/icons/circle-alert.svg"),
    ),
];

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub(crate) enum Icon {
    X,
    Plus,
    ChevronLeft,
    ChevronRight,
    ChevronUp,
    ChevronDown,
    CircleAlert,
}

impl Icon {
    #[cfg(test)]
    const ALL: [Self; 7] = [
        Self::X,
        Self::Plus,
        Self::ChevronLeft,
        Self::ChevronRight,
        Self::ChevronUp,
        Self::ChevronDown,
        Self::CircleAlert,
    ];

    pub(crate) const fn asset_path(self) -> &'static str {
        match self {
            Self::X => "icons/x.svg",
            Self::Plus => "icons/plus.svg",
            Self::ChevronLeft => "icons/chevron-left.svg",
            Self::ChevronRight => "icons/chevron-right.svg",
            Self::ChevronUp => "icons/chevron-up.svg",
            Self::ChevronDown => "icons/chevron-down.svg",
            Self::CircleAlert => "icons/circle-alert.svg",
        }
    }
}

#[derive(Debug)]
struct UiAssets;

impl AssetSource for UiAssets {
    fn load(&self, path: &str) -> gpui::Result<Option<Cow<'static, [u8]>>> {
        Ok(ASSETS.iter().find_map(|(asset_path, bytes)| {
            (*asset_path == path).then_some(Cow::Borrowed(*bytes))
        }))
    }

    fn list(&self, path: &str) -> gpui::Result<Vec<SharedString>> {
        if path != "icons/" {
            return Ok(Vec::new());
        }
        Ok(ASSETS
            .iter()
            .map(|(asset_path, _)| SharedString::from(*asset_path))
            .collect())
    }
}

pub(crate) fn application() -> Application {
    Application::new().with_assets(UiAssets)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn every_icon_loads_non_empty_bytes_and_list_returns_exactly_all_icons() {
        let source = UiAssets;
        let loaded = Icon::ALL.map(|icon| {
            source
                .load(icon.asset_path())
                .expect("load embedded icon")
                .expect("embedded icon exists")
        });
        assert!(loaded.iter().all(|bytes| !bytes.is_empty()));

        let expected: Vec<SharedString> = Icon::ALL
            .iter()
            .map(|icon| SharedString::from(icon.asset_path()))
            .collect();
        assert_eq!(
            source.list("icons/").expect("list embedded icons"),
            expected
        );
    }
}
