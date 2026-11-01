//! The terminal weight select and its generation-checked background scan.

use gpui::{AppContext as _, Context, Window};
use huterm_config::{FontWeight, settings::Setting};
use toml::Value;

use super::{Desktop, MenuFor, SettingsView};
use crate::terminal_font::{WeightFaces, enumerate_weights};

#[cfg(test)]
pub(super) type Enumerator = fn(&str) -> Result<WeightFaces, String>;

#[derive(Default)]
pub(super) struct Weights {
    pub(super) family: String,
    pub(super) generation: u64,
    pub(super) faces: Option<WeightFaces>,
    fallback: bool,
    failed: std::collections::HashSet<String>,
    #[cfg(test)]
    pub(super) enumerator: Option<Enumerator>,
}

impl SettingsView {
    pub(super) fn weight_current(&self, setting: &Setting) -> FontWeight {
        self.value(setting)
            .as_ref()
            .and_then(|value| FontWeight::parse(value).ok())
            .unwrap_or_default()
    }

    /// What the select shows: the file's weight by name, or a value the
    /// file holds that is not a weight as written, so an invalid file shows
    /// what it says rather than the default.
    pub(super) fn weight_shown(&self, setting: &Setting) -> String {
        match self.value(setting) {
            Some(value) => FontWeight::parse(&value).map_or_else(
                |_| match value {
                    Value::String(text) => text,
                    other => other.to_string(),
                },
                weight_label,
            ),
            None => weight_label(FontWeight::default()),
        }
    }

    pub(super) fn weight_choices(&self, setting: &Setting) -> Vec<FontWeight> {
        let current = self.weight_current(setting);
        let mut options = self
            .weights
            .faces
            .as_ref()
            .map_or_else(|| vec![current], |faces| faces.options.clone());
        // A failed enumeration has all nine options, while an in-flight one
        // keeps only the configured value. Both stay usable.
        if self.weights.fallback {
            options = FontWeight::ALL.to_vec();
        }
        if !options.contains(&current) {
            options.push(current);
            options.sort_unstable();
        }
        options
    }

    pub(super) fn weight_disabled(&self) -> bool {
        self.weights
            .faces
            .as_ref()
            .is_some_and(|faces| faces.options.len() == 1)
    }

    pub(super) fn weight_note(&self, setting: &Setting) -> Option<String> {
        if setting.path != "font.weight" {
            return None;
        }
        self.weights
            .faces
            .as_ref()?
            .note(&self.weights.family, self.weight_current(setting))
    }

    pub(super) fn pick_weight(
        &mut self,
        setting: &'static Setting,
        picked: &str,
        cx: &mut Context<'_, Self>,
    ) {
        let value = Value::String(picked.into());
        let Ok(weight) = FontWeight::parse(&value) else {
            return;
        };
        if self.weight_disabled()
            || !self.weight_choices(setting).contains(&weight)
        {
            return;
        }
        // Numeric spellings are semantically equal; preserve the file until
        // the user chooses a different weight.
        if weight != self.weight_current(setting) {
            self.write(setting, Some(value), cx);
        }
    }

    pub(super) fn open_weights(
        &mut self,
        setting: &'static Setting,
        window: &mut Window,
        cx: &mut Context<'_, Self>,
    ) {
        use huterm_ui::listbox::{Item, Model, Row};
        if self.weight_disabled() || self.read_only_reason(setting).is_some() {
            return;
        }
        let rows = self
            .weight_choices(setting)
            .into_iter()
            .map(|weight| {
                Row::Item(Item::new(weight.name(), weight_label(weight)))
            })
            .collect();
        let opener = self.stop(setting.path.as_str(), cx);
        self.show_menu(
            MenuFor::Setting(setting),
            Model::new(rows),
            opener,
            window,
            cx,
        );
    }

    pub(super) fn scan_weights(&mut self, cx: &mut Context<'_, Self>) {
        let desktop = cx.global::<Desktop>();
        let family = desktop
            .applied_terminal_family
            .clone()
            .unwrap_or_else(|| desktop.config.font.family.clone());
        let setting = self.setting_at("font.weight").expect("weight setting");
        let before = self.weight_choices(setting);
        let family_changed = self.weights.family != family;
        self.weights.family.clone_from(&family);
        self.weights.faces = None;
        self.weights.fallback = false;
        if family_changed || before != self.weight_choices(setting) {
            self.close_weight_menu();
        }
        cx.notify();
        self.weights.generation += 1;
        let generation = self.weights.generation;
        #[cfg(test)]
        let enumerator = self.weights.enumerator.unwrap_or(enumerate_weights);
        #[cfg(not(test))]
        let enumerator = enumerate_weights;
        let scanned_family = family.clone();
        let task =
            cx.background_spawn(async move { enumerator(&scanned_family) });
        cx.spawn(async move |view, cx| {
            let result = task.await;
            let _ = view.update(cx, |view, cx| {
                view.weights_scanned(&family, generation, result, cx);
            });
        })
        .detach();
    }

    fn close_weight_menu(&mut self) {
        if self
            .menu
            .as_ref()
            .is_some_and(|(_, target, _)| target.key() == "font.weight")
        {
            self.drop_menu();
        }
    }

    pub(super) fn weights_scanned(
        &mut self,
        family: &str,
        generation: u64,
        result: Result<WeightFaces, String>,
        cx: &mut Context<'_, Self>,
    ) {
        if self.weights.family != family
            || self.weights.generation != generation
        {
            return;
        }
        let setting = self.setting_at("font.weight").expect("weight setting");
        let before = self.weight_choices(setting);
        self.weights.fallback = result.is_err();
        let faces = match result {
            Ok(faces) => Some(faces),
            Err(error) => {
                if self.weights.failed.insert(family.into()) {
                    eprintln!(
                        "Huterm cannot enumerate weights for {family:?}: {error}"
                    );
                }
                None
            }
        };
        self.weights.faces = faces;
        if before != self.weight_choices(setting) || self.weight_disabled() {
            self.close_weight_menu();
        }
        cx.notify();
    }
}

pub(super) fn weight_label(weight: FontWeight) -> String {
    format!("{} ({})", weight.label(), weight.number())
}
