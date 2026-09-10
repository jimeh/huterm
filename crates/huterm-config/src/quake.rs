//! Portable quake profile configuration.

use std::collections::BTreeMap;

use serde::{Deserialize, Serialize};

#[derive(Clone, Debug, Default, Deserialize, Serialize, PartialEq)]
#[cfg_attr(feature = "schema", derive(schemars::JsonSchema))]
#[serde(default, deny_unknown_fields)]
pub struct Config {
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
    pub position: Position,
    pub display: String,
    pub width: f64,
    pub height: f64,
    pub fullscreen: bool,
    pub hide_on_focus_loss: bool,
    pub animation: Animation,
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
    Top,
    Bottom,
    Left,
    Right,
    Center,
}
#[derive(Clone, Copy, Debug, Deserialize, Serialize, Eq, PartialEq)]
#[cfg_attr(feature = "schema", derive(schemars::JsonSchema))]
#[serde(rename_all = "snake_case")]
pub enum Animation {
    Auto,
    None,
    Fade,
    Slide,
    SlideTop,
    SlideBottom,
    SlideLeft,
    SlideRight,
    FadeSlideTop,
    FadeSlideBottom,
    FadeSlideLeft,
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
