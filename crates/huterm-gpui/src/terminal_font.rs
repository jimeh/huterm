//! Terminal font identity and the faces GPUI can match.

use std::sync::atomic::{AtomicBool, Ordering};

use gpui::{Font, font};
use huterm_config::FontWeight;
use zed_font_kit::{
    matching::find_best_match,
    properties::{Properties, Weight},
};

#[cfg(target_os = "macos")]
use zed_font_kit::source::SystemSource;

pub(crate) const BUNDLED_FAMILY: &str = "DejaVu Sans Mono";

// The terminal family's faces. `BUNDLED_FAMILIES` is the extension point for
// additional bundled fonts.
const BUNDLED_FACES: &[&[u8]] = &[
    include_bytes!("../assets/fonts/dejavu/DejaVuSansMono.ttf"),
    include_bytes!("../assets/fonts/dejavu/DejaVuSansMono-Bold.ttf"),
    include_bytes!("../assets/fonts/dejavu/DejaVuSansMono-Oblique.ttf"),
    include_bytes!("../assets/fonts/dejavu/DejaVuSansMono-BoldOblique.ttf"),
];

/// The icon font the renderer draws Nerd Font code points from when the
/// terminal font lacks them. It is not a terminal or interface family.
pub(crate) const SYMBOLS_FAMILY: &str = "Symbols Nerd Font";

pub(crate) const SYMBOLS_FACE: &[u8] =
    include_bytes!("../assets/fonts/nerd-fonts/SymbolsNerdFont-Regular.ttf");

/// Each family registers on its own, so one family's failure never hides
/// another.
const BUNDLED_FAMILIES: [(&str, &[&[u8]]); 2] = [
    (BUNDLED_FAMILY, BUNDLED_FACES),
    (SYMBOLS_FAMILY, &[SYMBOLS_FACE]),
];

/// Run before the first font lookup in every native application entrypoint.
pub(crate) fn register_bundled_fonts(cx: &gpui::App) {
    static LOG_FAILURE: [std::sync::Once; BUNDLED_FAMILIES.len()] =
        [const { std::sync::Once::new() }; BUNDLED_FAMILIES.len()];
    for ((family, faces), logged) in BUNDLED_FAMILIES.iter().zip(&LOG_FAILURE) {
        match register_faces(cx.text_system(), family, faces) {
            Ok(()) if *family == SYMBOLS_FAMILY => {
                SYMBOLS_REGISTERED.store(true, Ordering::Relaxed);
            }
            Ok(()) => {}
            Err(error) => logged.call_once(|| {
                eprintln!(
                    "Huterm bundled font {family:?} failed to load: {error:#}"
                );
            }),
        }
    }
}

static SYMBOLS_REGISTERED: AtomicBool = AtomicBool::new(false);

/// Whether the symbols font registered. GPUI's `resolve_font` panics when a
/// family and its whole fallback stack are missing, so the renderer asks
/// for the symbols family only after it loaded.
pub(crate) fn symbols_available() -> bool {
    SYMBOLS_REGISTERED.load(Ordering::Relaxed)
}

/// Whether a family belongs in the terminal and interface font lists.
pub(crate) fn is_selectable_family(family: &str) -> bool {
    family != SYMBOLS_FAMILY
}

#[cfg_attr(
    not(target_os = "linux"),
    expect(unused_variables, reason = "only Linux checks the family")
)]
fn register_faces(
    text_system: &gpui::TextSystem,
    family: &str,
    faces: &[&'static [u8]],
) -> anyhow::Result<()> {
    text_system.add_fonts(
        faces
            .iter()
            .map(|bytes| std::borrow::Cow::Borrowed(*bytes))
            .collect(),
    )?;
    // Linux's add_fonts returns Ok even if fontdb rejected every byte.
    #[cfg(target_os = "linux")]
    anyhow::ensure!(
        text_system
            .all_font_names()
            .iter()
            .any(|name| name == family),
        "registered family is absent from the text system"
    );
    Ok(())
}

pub(crate) fn default_family() -> String {
    huterm_config::FontConfig::default().family
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum FamilyReason {
    Installed,
    MonospaceAlias,
    Missing,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) struct ResolvedFamily {
    pub(crate) configured: String,
    pub(crate) family: String,
    pub(crate) reason: FamilyReason,
    pub(crate) available: bool,
}

impl ResolvedFamily {
    pub(crate) fn resolve(
        configured: &str,
        installed: impl Fn(&str) -> bool,
    ) -> Self {
        let reason = if configured == "monospace" {
            FamilyReason::MonospaceAlias
        } else if installed(configured) {
            FamilyReason::Installed
        } else {
            FamilyReason::Missing
        };
        let family = if reason == FamilyReason::Installed {
            configured.into()
        } else {
            default_family()
        };
        let available = installed(&family);
        Self {
            configured: configured.into(),
            family,
            reason,
            available,
        }
    }

    pub(crate) fn fallback(self) -> Option<Self> {
        (self.reason != FamilyReason::Installed).then_some(self)
    }

    /// The configured family is the platform default, which did not load.
    fn default_unavailable(&self) -> bool {
        !self.available && self.configured == self.family
    }

    pub(crate) fn warning(&self) -> Option<String> {
        (self.reason == FamilyReason::Missing).then(|| {
            if self.default_unavailable() {
                format!("Huterm font {:?} failed to load", self.configured)
            } else if self.available {
                format!("Huterm font {:?} is unavailable; using {}", self.configured, self.family)
            } else {
                format!("Huterm font {:?} is unavailable; the platform default font is unavailable too", self.configured)
            }
        })
    }

    pub(crate) fn note(&self) -> String {
        let reason = match self.reason {
            FamilyReason::MonospaceAlias => "is a font alias",
            FamilyReason::Missing => "is not installed",
            FamilyReason::Installed => return String::new(),
        };
        if self.default_unavailable() {
            format!("{} failed to load.", self.configured)
        } else if self.available {
            format!(
                "{} {reason}, so terminals use {}.",
                self.configured, self.family
            )
        } else {
            format!(
                "{} {reason}. The platform default font is unavailable too.",
                self.configured
            )
        }
    }
}

/// Keep weight with the resolved family at every measurement and shaping site.
#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) struct TerminalFont {
    pub(crate) family: String,
    pub(crate) weight: FontWeight,
    pub(crate) bold_weight: FontWeight,
    /// Whether icons the family lacks draw from the bundled symbols font.
    /// It shapes glyph layouts, so a change re-lays them like a family change.
    pub(crate) nerd_font_symbols: bool,
}

impl TerminalFont {
    pub(crate) fn new(family: String, weight: FontWeight) -> Self {
        Self {
            family,
            weight,
            bold_weight: weight.bold(),
            nerd_font_symbols: true,
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

/// Only background workers may initialize or wait for the system font scan.
#[cfg(target_os = "linux")]
fn font_database() -> &'static std::sync::Arc<fontdb::Database> {
    static DATABASE: std::sync::OnceLock<std::sync::Arc<fontdb::Database>> =
        std::sync::OnceLock::new();
    DATABASE.get_or_init(|| {
        let mut database = fontdb::Database::new();
        database.load_system_fonts();
        append_bundled_faces(&mut database);
        std::sync::Arc::new(database)
    })
}

/// Match GPUI: installed faces precede equally suitable bundled faces.
#[cfg(target_os = "linux")]
fn append_bundled_faces(database: &mut fontdb::Database) {
    for bytes in BUNDLED_FACES {
        database.load_font_data(bytes.to_vec());
    }
}

/// Match GPUI's Linux face-info conversion before CSS face matching.
#[cfg(target_os = "linux")]
fn fontdb_properties(face: &fontdb::FaceInfo) -> Properties {
    use zed_font_kit::properties::{Stretch, Style};
    Properties {
        weight: Weight(f32::from(face.weight.0)),
        style: match face.style {
            fontdb::Style::Normal => Style::Normal,
            fontdb::Style::Italic => Style::Italic,
            fontdb::Style::Oblique => Style::Oblique,
        },
        stretch: match face.stretch {
            fontdb::Stretch::UltraCondensed => Stretch::ULTRA_CONDENSED,
            fontdb::Stretch::ExtraCondensed => Stretch::EXTRA_CONDENSED,
            fontdb::Stretch::Condensed => Stretch::CONDENSED,
            fontdb::Stretch::SemiCondensed => Stretch::SEMI_CONDENSED,
            fontdb::Stretch::Normal => Stretch::NORMAL,
            fontdb::Stretch::SemiExpanded => Stretch::SEMI_EXPANDED,
            fontdb::Stretch::Expanded => Stretch::EXPANDED,
            fontdb::Stretch::ExtraExpanded => Stretch::EXTRA_EXPANDED,
            fontdb::Stretch::UltraExpanded => Stretch::ULTRA_EXPANDED,
        },
    }
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

/// Native properties can panic. Contain failures on the background worker;
/// the caller logs once per family and offers all weights.
#[cfg(target_os = "macos")]
pub(crate) fn enumerate_weights(family: &str) -> Result<WeightFaces, String> {
    catch_enumeration(|| {
        if family == BUNDLED_FAMILY {
            // GPUI's memory source takes precedence over the native source.
            let mut properties = Vec::new();
            for bytes in BUNDLED_FACES {
                let face = zed_font_kit::font::Font::from_bytes(
                    std::sync::Arc::new(bytes.to_vec()),
                    0,
                )
                .map_err(|error| error.to_string())?;
                if candidate(&face) {
                    properties.push(face.properties());
                }
            }
            return WeightFaces::from_properties(&properties);
        }
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

#[cfg(target_os = "linux")]
pub(crate) fn enumerate_weights(family: &str) -> Result<WeightFaces, String> {
    catch_enumeration(|| database_weights(font_database(), family))
}

#[cfg(target_os = "linux")]
fn database_weights(
    database: &fontdb::Database,
    family: &str,
) -> Result<WeightFaces, String> {
    let properties = database
        .faces()
        .filter(|face| face.families.iter().any(|name| name.0 == family))
        .filter(|face| candidate(database, face))
        .map(fontdb_properties)
        .collect::<Vec<_>>();
    WeightFaces::from_properties(&properties)
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
fn candidate(database: &fontdb::Database, face: &fontdb::FaceInfo) -> bool {
    database
        .with_face_data(face.id, |data, index| {
            // font-kit's statically linked FreeType loader, so no direct
            // ttf-parser dependency and no dynamic FreeType.
            let Ok(font) = zed_font_kit::font::Font::from_bytes(
                std::sync::Arc::new(data.to_vec()),
                index,
            ) else {
                return false;
            };
            font.glyph_for_char('m').is_some()
                || matches!(
                    face.post_script_name.as_str(),
                    "SegoeFluentIcons" | "Segoe Fluent Icons"
                )
        })
        .unwrap_or(false)
}

#[cfg(test)]
mod tests {
    use super::*;
    use zed_font_kit::properties::{Stretch, Style};

    #[test]
    fn bundled_family_offers_regular_and_bold() {
        let faces = enumerate_weights("DejaVu Sans Mono").unwrap();
        assert_eq!(
            faces.options.iter().map(|w| w.number()).collect::<Vec<_>>(),
            [400, 700]
        );
    }

    #[test]
    fn bundled_faces_have_the_expected_family_weight_and_style() {
        for (bytes, weight, oblique) in [
            (BUNDLED_FACES[0], 400.0, false),
            (BUNDLED_FACES[1], 700.0, false),
            (BUNDLED_FACES[2], 400.0, true),
            (BUNDLED_FACES[3], 700.0, true),
        ] {
            let font = zed_font_kit::font::Font::from_bytes(
                std::sync::Arc::new(bytes.to_vec()),
                0,
            )
            .unwrap();
            assert_eq!(font.family_name(), BUNDLED_FAMILY);
            assert!(font.is_monospace());
            assert_eq!(font.properties().weight, Weight(weight));
            assert_eq!(font.properties().style != Style::Normal, oblique);
        }
    }

    #[test]
    fn family_resolution_preserves_installed_names_and_resolves_alias_and_missing()
     {
        let default = default_family();
        assert_eq!(default, crate::config::Config::default().font.family);
        assert_eq!(
            default.as_str(),
            huterm_ui::typography::Typography::platform_mono().as_ref()
        );
        let installed =
            |family: &str| family == default || family == "Test Mono";
        let resolved = ResolvedFamily::resolve("Test Mono", installed);
        assert_eq!(resolved.reason, FamilyReason::Installed);
        assert_eq!(resolved.family, "Test Mono");
        assert_eq!(resolved.warning(), None);
        assert_eq!(resolved.fallback(), None);
        let alias = ResolvedFamily::resolve("monospace", installed);
        assert_eq!(alias.reason, FamilyReason::MonospaceAlias);
        assert_eq!(alias.family, default);
        assert_eq!(alias.warning(), None);
        assert_eq!(
            alias.note(),
            format!("monospace is a font alias, so terminals use {default}.")
        );
        let missing = ResolvedFamily::resolve("Missing Mono", installed);
        assert_eq!(missing.reason, FamilyReason::Missing);
        assert_eq!(missing.family, default);
        assert_eq!(
            missing.warning(),
            Some(format!(
                "Huterm font \"Missing Mono\" is unavailable; using {default}"
            ))
        );
        assert_eq!(
            missing.note(),
            format!(
                "Missing Mono is not installed, so terminals use {default}."
            )
        );
    }

    #[test]
    fn an_unregistered_default_is_not_named_as_the_drawn_font() {
        for configured in ["Missing Mono", "monospace"] {
            let resolved = ResolvedFamily::resolve(configured, |_| false);
            assert!(!resolved.available);
            assert!(!resolved.note().contains("terminals use"));
            assert!(
                resolved
                    .note()
                    .contains("platform default font is unavailable")
            );
            assert!(
                !resolved.warning().unwrap_or_default().contains("; using")
            );
        }
        let default = default_family();
        let resolved = ResolvedFamily::resolve(&default, |_| false);
        assert_eq!(resolved.note(), format!("{default} failed to load."));
        assert_eq!(
            resolved.warning(),
            Some(format!("Huterm font {default:?} failed to load"))
        );
    }

    #[cfg(target_os = "linux")]
    #[test]
    fn bundled_faces_alone_offer_regular_and_bold_on_linux() {
        let mut database = fontdb::Database::new();
        append_bundled_faces(&mut database);
        let faces = database_weights(&database, BUNDLED_FAMILY).unwrap();
        assert_eq!(
            faces.options.iter().map(|w| w.number()).collect::<Vec<_>>(),
            [400, 700]
        );
    }

    #[cfg(target_os = "macos")]
    #[test]
    fn native_registration_resolves_all_four_bundled_faces() {
        use gpui::PlatformTextSystem as _;
        let platform = std::sync::Arc::new(gpui_apple::AppleTextSystem::new());
        let system = gpui::TextSystem::new(platform.clone());
        register_faces(&system, BUNDLED_FAMILY, BUNDLED_FACES).unwrap();
        let mut ids = Vec::new();
        for request in [
            font(BUNDLED_FAMILY),
            font(BUNDLED_FAMILY).bold(),
            font(BUNDLED_FAMILY).italic(),
            font(BUNDLED_FAMILY).bold().italic(),
        ] {
            let id = platform.font_id(&request).unwrap();
            assert_eq!(system.resolve_font(&request), id);
            assert!(platform.glyph_for_char(id, 'm').is_some());
            assert!(
                system.em_advance(id, gpui::px(14.0)).unwrap() > gpui::px(0.0)
            );
            assert!(!ids.contains(&id), "every style must select its own face");
            ids.push(id);
        }
        assert_eq!(
            system
                .all_font_names()
                .iter()
                .filter(|name| name.as_str() == BUNDLED_FAMILY)
                .count(),
            1
        );
    }

    #[cfg(all(target_os = "linux", feature = "test-support"))]
    #[test]
    fn native_registration_resolves_all_four_faces_without_system_fonts() {
        use gpui::PlatformTextSystem as _;
        let platform = std::sync::Arc::new(
            gpui_wgpu::CosmicTextSystem::new_without_system_fonts(
                BUNDLED_FAMILY,
            ),
        );
        assert_eq!(platform.all_font_names(), Vec::<String>::new());
        let cx = gpui::TestAppContext::build_with_text_system(
            gpui::TestDispatcher::new(0),
            None,
            platform.clone(),
        );
        cx.update(|cx| {
            register_bundled_fonts(cx);
            let mut ids = Vec::new();
            for (request, weight, style) in [
                (font(BUNDLED_FAMILY), 400.0, gpui::FontStyle::Normal),
                (font(BUNDLED_FAMILY).bold(), 700.0, gpui::FontStyle::Normal),
                (
                    font(BUNDLED_FAMILY).italic(),
                    400.0,
                    gpui::FontStyle::Italic,
                ),
                (
                    font(BUNDLED_FAMILY).bold().italic(),
                    700.0,
                    gpui::FontStyle::Italic,
                ),
            ] {
                let id = platform.font_id(&request).unwrap();
                assert_eq!(cx.text_system().resolve_font(&request), id);
                assert_eq!(
                    platform.font_weight_and_style(id).unwrap(),
                    (gpui::FontWeight(weight), style)
                );
                assert!(
                    cx.text_system().em_advance(id, gpui::px(14.0)).unwrap()
                        > gpui::px(0.0)
                );
                assert!(!ids.contains(&id));
                ids.push(id);
            }
            assert_eq!(
                cx.text_system().all_font_names(),
                [BUNDLED_FAMILY, SYMBOLS_FAMILY]
            );
        });
    }

    #[cfg(target_os = "linux")]
    #[test]
    fn rejected_font_bytes_fail_the_registration_postcondition() {
        let platform = std::sync::Arc::new(
            gpui_wgpu::CosmicTextSystem::new_without_system_fonts(
                BUNDLED_FAMILY,
            ),
        );
        let system = gpui::TextSystem::new(platform);
        // The backend accepts the call, so only the family postcondition catches it.
        system
            .add_fonts(vec![std::borrow::Cow::Borrowed(b"invalid font")])
            .unwrap();
        let error = register_faces(&system, BUNDLED_FAMILY, &[b"invalid font"])
            .unwrap_err();
        assert!(error.to_string().contains("registered family is absent"));
    }

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
            catch_enumeration(|| panic!("font enumeration unavailable"))
                .is_err()
        );
        assert!(
            catch_enumeration(|| WeightFaces::from_properties(&[])).is_err()
        );
    }

    #[cfg(target_os = "linux")]
    fn database_face() -> fontdb::FaceInfo {
        fontdb::FaceInfo {
            id: fontdb::ID::dummy(),
            source: fontdb::Source::Binary(std::sync::Arc::new(
                Vec::<u8>::new(),
            )),
            index: 0,
            families: vec![(
                "Test Mono".into(),
                fontdb::Language::English_UnitedStates,
            )],
            post_script_name: "TestMono".into(),
            style: fontdb::Style::Normal,
            weight: fontdb::Weight(450),
            stretch: fontdb::Stretch::Normal,
            monospaced: true,
        }
    }

    #[cfg(target_os = "linux")]
    #[test]
    fn fontdb_properties_preserve_weight_style_and_all_stretches() {
        let mut face = database_face();
        for (native, expected) in [
            (fontdb::Style::Normal, Style::Normal),
            (fontdb::Style::Italic, Style::Italic),
            (fontdb::Style::Oblique, Style::Oblique),
        ] {
            face.style = native;
            let properties = fontdb_properties(&face);
            assert_eq!(properties.style, expected);
            assert_eq!(properties.weight, Weight(450.0));
        }
        for (native, expected) in [
            (fontdb::Stretch::UltraCondensed, Stretch::ULTRA_CONDENSED),
            (fontdb::Stretch::ExtraCondensed, Stretch::EXTRA_CONDENSED),
            (fontdb::Stretch::Condensed, Stretch::CONDENSED),
            (fontdb::Stretch::SemiCondensed, Stretch::SEMI_CONDENSED),
            (fontdb::Stretch::Normal, Stretch::NORMAL),
            (fontdb::Stretch::SemiExpanded, Stretch::SEMI_EXPANDED),
            (fontdb::Stretch::Expanded, Stretch::EXPANDED),
            (fontdb::Stretch::ExtraExpanded, Stretch::EXTRA_EXPANDED),
            (fontdb::Stretch::UltraExpanded, Stretch::ULTRA_EXPANDED),
        ] {
            face.stretch = native;
            assert_eq!(fontdb_properties(&face).stretch, expected);
        }
    }
}
