//! Filtered, keyboard-navigable list state shared by palette pickers.

#[derive(Clone, Debug)]
pub(crate) struct PickerItem<T> {
    pub(crate) value: T,
    pub(crate) label: String,
    pub(crate) detail: String,
}

#[derive(Clone, Debug)]
pub(crate) struct PickerList<T> {
    items: Vec<PickerItem<T>>,
    filtered: Vec<usize>,
    selected: usize,
}

impl<T> PickerList<T> {
    pub(crate) fn new(items: Vec<PickerItem<T>>) -> Self {
        let filtered = (0..items.len()).collect();
        Self {
            items,
            filtered,
            selected: 0,
        }
    }

    pub(crate) fn filter(&mut self, query: &str) {
        let terms: Vec<_> =
            query.split_whitespace().map(str::to_lowercase).collect();
        self.filtered = self
            .items
            .iter()
            .enumerate()
            .filter_map(|(index, item)| {
                let haystack =
                    format!("{} {}", item.label, item.detail).to_lowercase();
                terms
                    .iter()
                    .all(|term| haystack.contains(term))
                    .then_some(index)
            })
            .collect();
        self.selected =
            self.selected.min(self.filtered.len().saturating_sub(1));
    }

    pub(crate) fn move_selection(&mut self, delta: isize) {
        if self.filtered.is_empty() {
            self.selected = 0;
            return;
        }
        self.selected = self
            .selected
            .saturating_add_signed(delta)
            .min(self.filtered.len() - 1);
    }

    pub(crate) fn selected(&self) -> Option<&PickerItem<T>> {
        self.filtered
            .get(self.selected)
            .and_then(|index| self.items.get(*index))
    }

    pub(crate) fn selected_index(&self) -> usize {
        self.selected
    }

    pub(crate) fn select_where(&mut self, predicate: impl Fn(&T) -> bool) {
        if let Some(row) = self.filtered.iter().position(|index| {
            self.items
                .get(*index)
                .is_some_and(|item| predicate(&item.value))
        }) {
            self.selected = row;
        }
    }

    pub(crate) fn visible(
        &self,
    ) -> impl Iterator<Item = (bool, &PickerItem<T>)> {
        self.filtered.iter().enumerate().filter_map(|(row, index)| {
            self.items
                .get(*index)
                .map(|item| (row == self.selected, item))
        })
    }

    pub(crate) fn is_empty(&self) -> bool {
        self.filtered.is_empty()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn filtering_matches_all_terms_and_clamps_selection() {
        let mut picker = PickerList::new(vec![
            PickerItem {
                value: 1,
                label: "Build".into(),
                detail: "First session".into(),
            },
            PickerItem {
                value: 2,
                label: "Test".into(),
                detail: "Second session".into(),
            },
        ]);
        picker.move_selection(1);
        picker.filter("build first");
        assert_eq!(picker.selected().map(|item| item.value), Some(1));
        picker.filter("missing");
        assert!(picker.is_empty());
    }
}
