//! Terminal font identity and the faces GPUI can match.

use gpui::{Font, font};
use huterm_config::FontWeight;
use zed_font_kit::{
    matching::find_best_match,
    properties::{Properties, Weight},
    source::SystemSource,
};

/// Keep weight with the resolved family at every measurement and shaping site.
#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) struct TerminalFont {
    pub(crate) family: String,
    pub(crate) weight: FontWeight,
    pub(crate) bold_weight: FontWeight,
}

impl TerminalFont {
    pub(crate) fn new(family: String, weight: FontWeight) -> Self {
        Self {
            family,
            weight,
            bold_weight: weight.bold(),
        }
    }

    pub(crate) fn face(&self, bold: bool) -> Font {
        let mut face = font(self.family.clone());
        face.weight = gpui::FontWeight(f32::from(
            if bold { self.bold_weight } else { self.weight }.number(),
        ));
        face
    }
}

impl From<&str> for TerminalFont {
    fn from(family: &str) -> Self {
        Self::new(family.into(), FontWeight::default())
    }
}

impl From<String> for TerminalFont {
    fn from(family: String) -> Self {
        Self::new(family, FontWeight::default())
    }
}

/// Resolve fontconfig's alias before GPUI's exact family-name lookup. Its
/// internal missing-family fallback builds regular faces and loses weight.
/// GPUI's Linux text system reads the fonts once at startup, so the first
/// answer holds for the process and later windows skip fontconfig.
#[cfg(target_os = "linux")]
pub(crate) fn monospace_family() -> Option<String> {
    static FAMILY: std::sync::OnceLock<Option<String>> =
        std::sync::OnceLock::new();
    FAMILY.get_or_init(resolve_monospace_family).clone()
}

#[cfg(target_os = "linux")]
fn resolve_monospace_family() -> Option<String> {
    use zed_font_kit::family_name::FamilyName;
    std::panic::catch_unwind(|| {
        SystemSource::new()
            .select_best_match(&[FamilyName::Monospace], &Properties::new())
            .ok()?
            .load()
            .ok()
            .map(|face| face.family_name())
    })
    .ok()
    .flatten()
}

/// Round a native face to the supported configuration grid.
fn rounded(weight: Weight) -> FontWeight {
    let number = (weight.0 / 100.0).round().clamp(1.0, 9.0) * 100.0;
    FontWeight::ALL
        .into_iter()
        .rev()
        .find(|weight| f32::from(weight.number()) <= number)
        .unwrap_or_default()
}

/// The result of matching all nine requests, preserving style and stretch
/// until font-kit's CSS matching has chosen a face.
#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) struct WeightFaces {
    matched: [FontWeight; 9],
    pub(crate) options: Vec<FontWeight>,
}

impl WeightFaces {
    pub(crate) fn from_properties(
        faces: &[Properties],
    ) -> Result<Self, String> {
        let mut matched = [FontWeight::default(); 9];
        for (index, weight) in FontWeight::ALL.into_iter().enumerate() {
            let mut query = Properties::new();
            query.weight = Weight(f32::from(weight.number()));
            let found = find_best_match(faces, &query)
                .map_err(|error| error.to_string())?;
            matched[index] = rounded(faces[found].weight);
        }
        let mut options = matched.to_vec();
        options.sort_unstable();
        options.dedup();
        Ok(Self { matched, options })
    }

    pub(crate) fn note(
        &self,
        family: &str,
        configured: FontWeight,
    ) -> Option<String> {
        let index = FontWeight::ALL
            .iter()
            .position(|weight| *weight == configured)?;
        let drawn = self.matched[index];
        if self.options.len() == 1 {
            return Some(if drawn == configured {
                format!("{family} has one weight.")
            } else {
                format!(
                    "{family} has one weight, so terminals use {}.",
                    drawn.label()
                )
            });
        }
        (drawn != configured).then(|| {
            format!(
                "{family} has no {} weight, so terminals use {}.",
                configured.label(),
                drawn.label()
            )
        })
    }
}

/// Loading fontconfig and native properties can panic. Contain both on the
/// background worker; the caller logs once per family and offers all weights.
pub(crate) fn enumerate_weights(family: &str) -> Result<WeightFaces, String> {
    catch_enumeration(|| {
        let family = SystemSource::new()
            .select_family_by_name(family)
            .map_err(|error| error.to_string())?;
        let mut properties = Vec::new();
        for handle in family.fonts() {
            let Ok(face) = handle.load() else {
                continue;
            };
            if !candidate(&face) {
                continue;
            }
            properties.push(face.properties());
        }
        WeightFaces::from_properties(&properties)
    })
}

fn catch_enumeration(
    enumerate: impl FnOnce() -> Result<WeightFaces, String> + std::panic::UnwindSafe,
) -> Result<WeightFaces, String> {
    std::panic::catch_unwind(enumerate)
        .unwrap_or_else(|_| Err("font enumeration panicked".into()))
}

#[cfg(target_os = "macos")]
#[allow(unsafe_code)]
fn candidate(face: &zed_font_kit::font::Font) -> bool {
    use core_foundation::number::CFNumber;
    use core_text::font_descriptor::{
        kCTFontSlantTrait, kCTFontSymbolicTrait, kCTFontWeightTrait,
        kCTFontWidthTrait,
    };
    if face.glyph_for_char('m').is_none()
        && face.full_name() != "Segoe Fluent Icons"
    {
        return false;
    }
    let traits = face.native_font().all_traits();
    // SAFETY: CoreText exports these immutable trait dictionary keys. Match
    // GPUI's guards before font-kit's properties() unwraps their numeric types.
    let numeric = unsafe {
        [
            kCTFontSymbolicTrait,
            kCTFontWidthTrait,
            kCTFontWeightTrait,
            kCTFontSlantTrait,
        ]
        .into_iter()
        .all(|key| traits.get(key).downcast::<CFNumber>().is_some())
    };
    numeric && face.postscript_name().is_some()
}

#[cfg(target_os = "linux")]
fn candidate(face: &zed_font_kit::font::Font) -> bool {
    face.glyph_for_char('m').is_some()
        || face.postscript_name().is_some_and(|name| {
            matches!(name.as_str(), "SegoeFluentIcons" | "Segoe Fluent Icons")
        })
}

#[cfg(test)]
mod tests {
    use super::*;
    use zed_font_kit::properties::{Stretch, Style};

    fn face(weight: f32, stretch: Stretch, style: Style) -> Properties {
        Properties {
            weight: Weight(weight),
            stretch,
            style,
        }
    }
    fn weight(number: i64) -> FontWeight {
        FontWeight::parse(&toml::Value::Integer(number)).unwrap()
    }

    #[test]
    fn options_round_native_weights_after_matching_stretch_and_style() {
        let faces = WeightFaces::from_properties(&[
            face(450.0, Stretch::NORMAL, Style::Normal),
            face(700.0, Stretch::NORMAL, Style::Normal),
            face(300.0, Stretch::CONDENSED, Style::Normal),
        ])
        .unwrap();
        assert_eq!(faces.options, [weight(500), weight(700)]);
        assert_eq!(
            faces.note("Test", weight(300)).as_deref(),
            Some("Test has no Light weight, so terminals use Medium.")
        );
        assert_eq!(
            faces.note("Test", weight(900)).as_deref(),
            Some("Test has no Black weight, so terminals use Bold.")
        );
    }

    #[test]
    fn italic_only_families_still_offer_weights() {
        let faces = WeightFaces::from_properties(&[
            face(300.0, Stretch::NORMAL, Style::Italic),
            face(600.0, Stretch::NORMAL, Style::Italic),
        ])
        .unwrap();
        assert_eq!(faces.options, [weight(300), weight(600)]);
        assert_eq!(faces.note("Test", weight(300)), None);
    }

    #[test]
    fn one_face_has_one_weight_note() {
        let faces = WeightFaces::from_properties(&[Properties::new()]).unwrap();
        assert_eq!(faces.options, [weight(400)]);
        assert_eq!(
            faces.note("Monaco", weight(300)).as_deref(),
            Some("Monaco has one weight, so terminals use Regular.")
        );
        assert_eq!(
            faces.note("Monaco", weight(400)).as_deref(),
            Some("Monaco has one weight.")
        );
    }

    #[test]
    fn enumeration_contains_panics_and_empty_candidate_failures() {
        assert!(
            catch_enumeration(|| panic!("fontconfig unavailable")).is_err()
        );
        assert!(
            catch_enumeration(|| WeightFaces::from_properties(&[])).is_err()
        );
    }

    #[cfg(target_os = "linux")]
    #[test]
    fn monospace_resolves_to_a_concrete_loadable_family() {
        let family = monospace_family().expect("fontconfig monospace family");
        assert_ne!(family, "monospace");
        assert!(SystemSource::new().select_family_by_name(&family).is_ok());
    }
}
