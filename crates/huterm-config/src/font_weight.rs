//! Dependency-neutral terminal font weights.

use serde::{Deserialize, Deserializer, Serialize, Serializer};

/// One of the nine CSS font weights, from 100 through 900.
#[derive(Clone, Copy, Debug, Eq, PartialEq, Ord, PartialOrd)]
pub struct FontWeight(u16);

impl FontWeight {
    /// The weights in ascending order.
    pub const ALL: [Self; 9] = [
        Self(100),
        Self(200),
        Self(300),
        Self(400),
        Self(500),
        Self(600),
        Self(700),
        Self(800),
        Self(900),
    ];

    /// The error for an unsupported configuration value.
    pub const ACCEPTED: &str = "font.weight must be thin, extra_light, light, regular, medium, semibold, bold, extra_bold, black, or an integer from 100 through 900 in steps of 100";

    /// The numeric weight.
    #[must_use]
    pub const fn number(self) -> u16 {
        self.0
    }

    /// The canonical configuration spelling.
    #[must_use]
    pub const fn name(self) -> &'static str {
        match self.0 {
            100 => "thin",
            200 => "extra_light",
            300 => "light",
            500 => "medium",
            600 => "semibold",
            700 => "bold",
            800 => "extra_bold",
            900 => "black",
            _ => "regular",
        }
    }

    /// The human-readable weight name.
    #[must_use]
    pub const fn label(self) -> &'static str {
        match self.0 {
            100 => "Thin",
            200 => "Extra light",
            300 => "Light",
            500 => "Medium",
            600 => "Semibold",
            700 => "Bold",
            800 => "Extra bold",
            900 => "Black",
            _ => "Regular",
        }
    }

    /// The heavier weight for SGR bold text.
    #[must_use]
    pub fn bold(self) -> Self {
        Self((self.0 + 300).clamp(700, 900))
    }

    /// Reads either supported TOML spelling without losing its type in the file.
    ///
    /// # Errors
    /// Returns the accepted values for an unsupported name, number, or type.
    pub fn parse(value: &toml::Value) -> Result<Self, &'static str> {
        Self::ALL
            .into_iter()
            .find(|weight| match value {
                toml::Value::String(name) => name == weight.name(),
                toml::Value::Integer(number) => *number == i64::from(weight.0),
                _ => false,
            })
            .ok_or(Self::ACCEPTED)
    }
}

impl Default for FontWeight {
    fn default() -> Self {
        Self(400)
    }
}

impl Serialize for FontWeight {
    fn serialize<S: Serializer>(
        &self,
        serializer: S,
    ) -> Result<S::Ok, S::Error> {
        serializer.serialize_str(self.name())
    }
}

impl<'de> Deserialize<'de> for FontWeight {
    fn deserialize<D: Deserializer<'de>>(
        deserializer: D,
    ) -> Result<Self, D::Error> {
        let value = toml::Value::deserialize(deserializer)?;
        Self::parse(&value).map_err(serde::de::Error::custom)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::RawConfig;

    #[test]
    fn weight_names_numbers_and_default_agree() {
        let default: RawConfig = toml::from_str("").unwrap();
        assert_eq!(default.font.weight.number(), 400);
        for weight in FontWeight::ALL {
            for value in [
                format!("\"{}\"", weight.name()),
                weight.number().to_string(),
            ] {
                let config: RawConfig =
                    toml::from_str(&format!("[font]\nweight = {value}"))
                        .unwrap();
                assert_eq!(config.font.weight, weight);
            }
        }
    }

    #[test]
    fn invalid_weights_name_the_key_and_accepted_values() {
        for value in ["350", "0", "1000", "1.5", "\"heavy\""] {
            let error = toml::from_str::<RawConfig>(&format!(
                "[font]\nweight = {value}"
            ))
            .unwrap_err()
            .to_string();
            assert!(error.contains(FontWeight::ACCEPTED), "{value}: {error}");
        }
    }

    #[test]
    fn bold_is_a_step_heavier_with_a_bold_floor_and_black_ceiling() {
        for (base, bold) in
            [(100, 700), (400, 700), (500, 800), (600, 900), (900, 900)]
        {
            assert_eq!(FontWeight(base).bold().number(), bold);
        }
    }
}
