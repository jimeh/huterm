//! Portable quake profile configuration.

use std::collections::BTreeMap;

use serde::{Deserialize, Serialize};

/// Named quake windows. The `default` profile exists even when unset.
#[derive(Clone, Debug, Default, Deserialize, Serialize, PartialEq)]
#[cfg_attr(feature = "schema", derive(schemars::JsonSchema))]
#[serde(default, deny_unknown_fields)]
pub struct Config {
    #[cfg_attr(
        feature = "schema",
        schemars(extend("propertyNames" = {"pattern": "\\S"}))
    )]
    pub profiles: BTreeMap<String, Profile>,
}

impl Config {
    /// Adds the default profile and validates portable profile settings.
    ///
    /// # Errors
    /// Returns the invalid profile and its constraint.
    pub fn validated(mut self) -> Result<Self, String> {
        self.profiles.entry("default".into()).or_default();
        self.validate()?;
        Ok(self)
    }

    /// Checks profile names and settings without adding defaults.
    ///
    /// # Errors
    /// Returns the invalid profile and its constraint.
    pub fn validate(&self) -> Result<(), String> {
        for (name, profile) in &self.profiles {
            if name.trim().is_empty() {
                return Err("quake profile names must not be blank".into());
            }
            profile
                .validate()
                .map_err(|error| format!("quake.profiles.{name}: {error}"))?;
        }
        Ok(())
    }
}

#[derive(Clone, Debug, Deserialize, Serialize, PartialEq)]
#[cfg_attr(feature = "schema", derive(schemars::JsonSchema))]
#[serde(default, deny_unknown_fields)]
pub struct Profile {
    /// The display edge the window sits against, or Center. Fullscreen uses it
    /// only for Slide.
    #[cfg_attr(
        feature = "schema",
        schemars(extend(
            "x-huterm-section" = "quake",
            "x-huterm-group" = "Profile",
            "x-huterm-label" = "Position",
            "x-huterm-order" = 20,
            "x-huterm-apply" = "later_events",
            "x-huterm-visible-when" = [
                {"setting": "fullscreen", "equals": false},
                {"setting": "animation", "equals": "slide"}
            ]
        ))
    )]
    pub position: Position,
    /// The display the window opens on: the active one, the one under the
    /// pointer, the primary one, or `id:` and a display identifier.
    #[cfg_attr(
        feature = "schema",
        schemars(regex(pattern = r"^(active|pointer|primary|id:.*\S.*)$"))
    )]
    #[cfg_attr(
        feature = "schema",
        schemars(extend(
            "x-huterm-section" = "quake",
            "x-huterm-group" = "Profile",
            "x-huterm-label" = "Display",
            "x-huterm-order" = 50,
            "x-huterm-apply" = "later_events"
        ))
    )]
    pub display: String,
    /// The fraction of the display's work area it takes, above 0 and at
    /// most 1.
    #[cfg_attr(
        feature = "schema",
        schemars(range(max = 1), extend("exclusiveMinimum" = 0))
    )]
    #[cfg_attr(
        feature = "schema",
        schemars(extend(
            "x-huterm-section" = "quake",
            "x-huterm-group" = "Profile",
            "x-huterm-label" = "Width",
            "x-huterm-order" = 30,
            "x-huterm-apply" = "later_events",
            "x-huterm-step" = 0.05,
            "x-huterm-control" = "slider",
            "x-huterm-visible-when" = {"setting": "fullscreen", "equals": false}
        ))
    )]
    pub width: f64,
    /// The fraction of the display's work area it takes, above 0 and at
    /// most 1.
    #[cfg_attr(
        feature = "schema",
        schemars(range(max = 1), extend("exclusiveMinimum" = 0))
    )]
    #[cfg_attr(
        feature = "schema",
        schemars(extend(
            "x-huterm-section" = "quake",
            "x-huterm-group" = "Profile",
            "x-huterm-label" = "Height",
            "x-huterm-order" = 40,
            "x-huterm-apply" = "later_events",
            "x-huterm-step" = 0.05,
            "x-huterm-control" = "slider",
            "x-huterm-visible-when" = {"setting": "fullscreen", "equals": false}
        ))
    )]
    pub height: f64,
    /// Covers the whole display frame instead of a part of its work area.
    #[cfg_attr(
        feature = "schema",
        schemars(extend(
            "x-huterm-section" = "quake",
            "x-huterm-group" = "Profile",
            "x-huterm-label" = "Fullscreen",
            "x-huterm-order" = 10,
            "x-huterm-apply" = "later_events"
        ))
    )]
    pub fullscreen: bool,
    /// Hides the window when another application takes focus.
    #[cfg_attr(
        feature = "schema",
        schemars(extend(
            "x-huterm-section" = "quake",
            "x-huterm-group" = "Profile",
            "x-huterm-label" = "Hide when it loses focus",
            "x-huterm-order" = 60,
            "x-huterm-apply" = "later_events"
        ))
    )]
    pub hide_on_focus_loss: bool,
    /// How the window shows and hides. Auto fades fullscreen windows and slides
    /// others in.
    #[cfg_attr(
        feature = "schema",
        schemars(extend(
            "x-huterm-section" = "quake",
            "x-huterm-group" = "Profile",
            "x-huterm-label" = "Animation",
            "x-huterm-order" = 70,
            "x-huterm-apply" = "later_events"
        ))
    )]
    pub animation: Animation,
    /// How long it animates, in milliseconds, from 0 to 1000.
    #[cfg_attr(feature = "schema", schemars(range(max = 1000)))]
    #[cfg_attr(
        feature = "schema",
        schemars(extend(
            "x-huterm-section" = "quake",
            "x-huterm-group" = "Profile",
            "x-huterm-label" = "Duration",
            "x-huterm-order" = 80,
            "x-huterm-apply" = "later_events",
            "x-huterm-unit" = "ms"
        ))
    )]
    pub animation_ms: u16,
}
impl Default for Profile {
    fn default() -> Self {
        Self {
            position: Position::Top,
            display: "active".into(),
            width: 1.0,
            height: 0.5,
            fullscreen: false,
            hide_on_focus_loss: true,
            animation: Animation::Auto,
            animation_ms: 150,
        }
    }
}
impl Profile {
    fn validate(&self) -> Result<(), String> {
        if [self.width, self.height]
            .iter()
            .any(|value| !value.is_finite() || *value <= 0.0 || *value > 1.0)
        {
            return Err(
                "width and height must be finite fractions in (0, 1]".into()
            );
        }
        if self.animation_ms > 1000 {
            return Err("animation_ms must be between 0 and 1000".into());
        }
        if !matches!(self.display.as_str(), "active" | "pointer" | "primary")
            && self
                .display
                .strip_prefix("id:")
                .is_none_or(|id| id.trim().is_empty())
        {
            return Err("display must be active, pointer, primary, or id:<platform display identifier>".into());
        }
        Ok(())
    }
}

#[derive(Clone, Copy, Debug, Deserialize, Serialize, Eq, PartialEq)]
#[cfg_attr(feature = "schema", derive(schemars::JsonSchema))]
#[serde(rename_all = "snake_case")]
pub enum Position {
    #[cfg_attr(feature = "schema", schemars(extend("x-huterm-label" = "Top")))]
    Top,
    #[cfg_attr(feature = "schema", schemars(extend("x-huterm-label" = "Bottom")))]
    Bottom,
    #[cfg_attr(feature = "schema", schemars(extend("x-huterm-label" = "Left")))]
    Left,
    #[cfg_attr(feature = "schema", schemars(extend("x-huterm-label" = "Right")))]
    Right,
    #[cfg_attr(feature = "schema", schemars(extend("x-huterm-label" = "Center")))]
    Center,
}
#[derive(Clone, Copy, Debug, Deserialize, Serialize, Eq, PartialEq)]
#[cfg_attr(feature = "schema", derive(schemars::JsonSchema))]
#[serde(rename_all = "snake_case")]
pub enum Animation {
    #[cfg_attr(feature = "schema", schemars(extend("x-huterm-label" = "Auto")))]
    Auto,
    #[cfg_attr(feature = "schema", schemars(extend("x-huterm-label" = "None")))]
    None,
    #[cfg_attr(feature = "schema", schemars(extend("x-huterm-label" = "Fade")))]
    Fade,
    #[cfg_attr(feature = "schema", schemars(extend("x-huterm-label" = "Slide")))]
    Slide,
    #[cfg_attr(feature = "schema", schemars(extend("x-huterm-label" = "Slide from the top")))]
    SlideTop,
    #[cfg_attr(feature = "schema", schemars(extend("x-huterm-label" = "Slide from the bottom")))]
    SlideBottom,
    #[cfg_attr(feature = "schema", schemars(extend("x-huterm-label" = "Slide from the left")))]
    SlideLeft,
    #[cfg_attr(feature = "schema", schemars(extend("x-huterm-label" = "Slide from the right")))]
    SlideRight,
    #[cfg_attr(feature = "schema", schemars(extend("x-huterm-label" = "Fade and slide from the top")))]
    FadeSlideTop,
    #[cfg_attr(feature = "schema", schemars(extend("x-huterm-label" = "Fade and slide from the bottom")))]
    FadeSlideBottom,
    #[cfg_attr(feature = "schema", schemars(extend("x-huterm-label" = "Fade and slide from the left")))]
    FadeSlideLeft,
    #[cfg_attr(feature = "schema", schemars(extend("x-huterm-label" = "Fade and slide from the right")))]
    FadeSlideRight,
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn malformed_profiles_are_rejected_and_default_is_always_present() {
        assert!(
            Config::default()
                .validated()
                .unwrap()
                .profiles
                .contains_key("default")
        );
        for value in [0.0, -0.1, f64::NAN, f64::INFINITY, 1.1] {
            assert!(
                Profile {
                    width: value,
                    ..Profile::default()
                }
                .validate()
                .is_err()
            );
        }
        assert!(
            Profile {
                animation_ms: 1001,
                ..Profile::default()
            }
            .validate()
            .is_err()
        );
        assert!(
            Profile {
                display: "2".into(),
                ..Profile::default()
            }
            .validate()
            .is_err()
        );
    }
}
