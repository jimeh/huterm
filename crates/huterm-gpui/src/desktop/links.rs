use huterm_protocol::{LinkLookup, MousePosition, TerminalLink};

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub(super) struct Intent {
    pub epoch: u64,
    pub point: MousePosition,
}

struct Press {
    cell: MousePosition,
    origin: (f32, f32),
    target: Option<TerminalLink>,
}

#[derive(Default)]
pub(super) struct Links {
    epoch: u64,
    point: Option<MousePosition>,
    enabled: bool,
    result: Option<(MousePosition, LinkLookup)>,
    press: Option<Press>,
}

impl Links {
    pub fn forget_press(&mut self) {
        self.press = None;
    }
    pub fn owns_press(&self) -> bool {
        self.press.is_some()
    }
    pub fn hover(&self) -> Option<&TerminalLink> {
        if !self.enabled {
            return None;
        }
        match self.result.as_ref() {
            Some((_, LinkLookup::Match(link))) => Some(link),
            _ => None,
        }
    }
    pub fn intent(&self) -> Option<Intent> {
        if !self.enabled {
            return None;
        }
        let point = match &self.press {
            Some(press) if press.target.is_some() => Some(press.cell),
            Some(_) => None,
            None => self.point,
        }?;
        Some(Intent {
            epoch: self.epoch,
            point,
        })
    }
    pub fn update(
        &mut self,
        point: Option<MousePosition>,
        enabled: bool,
        position: (f32, f32),
    ) -> bool {
        if let Some(press) = &mut self.press {
            let (x, y) = position;
            if (x - press.origin.0).hypot(y - press.origin.1) > 4.0 || !enabled
            {
                press.target = None;
            }
        }
        let previous = self.intent();
        let reusable =
            self.result.as_ref().is_some_and(
                |(queried, result)| match result {
                    LinkLookup::Match(link) => {
                        point.is_some_and(|point| contains(link, point))
                    }
                    _ => Some(*queried) == point,
                },
            );
        if self.enabled != enabled
            || (self.point != point && !reusable && self.press.is_none())
        {
            self.epoch = self.epoch.wrapping_add(1);
            self.result = None;
        }
        self.enabled = enabled;
        self.point = point;
        self.intent().is_some() && previous != self.intent() && !reusable
    }
    pub fn invalidate(&mut self) {
        self.epoch = self.epoch.wrapping_add(1);
        self.result = None;
        self.cancel_press();
    }
    pub fn disable(&mut self) {
        self.invalidate();
        self.enabled = false;
        self.point = None;
    }
    pub fn cancel_press(&mut self) -> bool {
        if let Some(press) = &mut self.press {
            press.target = None;
            true
        } else {
            false
        }
    }
    pub fn publish(
        &mut self,
        intent: Option<Intent>,
        outcome: Option<LinkLookup>,
    ) {
        self.result = None;
        let current = intent.is_some_and(|intent| self.enabled && intent.epoch == self.epoch && self.intent().is_some_and(|desired| {
            desired.point == intent.point || matches!(&outcome, Some(LinkLookup::Match(link)) if contains(link, desired.point))
        }));
        if current && let Some(outcome) = outcome {
            if let Some(press) = &mut self.press
                && !matches!(&outcome, LinkLookup::Match(target) if Some(target) == press.target.as_ref())
            {
                press.target = None;
            }
            self.result = intent.map(|intent| (intent.point, outcome));
            return;
        }
        self.cancel_press();
    }
    pub fn press(
        &mut self,
        point: MousePosition,
        position: (f32, f32),
    ) -> bool {
        if !self.enabled || self.press.is_some() {
            return false;
        }
        let target = match &self.result {
            Some((_, LinkLookup::Match(link))) if contains(link, point) => {
                Some(link.clone())
            }
            Some((queried, LinkLookup::NoMatch)) if *queried == point => {
                return false;
            }
            _ => None,
        };
        self.press = Some(Press {
            cell: point,
            origin: position,
            target,
        });
        true
    }
    pub fn release(
        &mut self,
        point: Option<MousePosition>,
        enabled: bool,
    ) -> (bool, Option<String>) {
        let Some(press) = self.press.take() else {
            return (false, None);
        };
        let destination = press
            .target
            .filter(|target| {
                enabled
                    && point.is_some_and(|point| contains(target, point))
                    && self.hover() == Some(target)
            })
            .map(|target| target.destination);
        (true, destination)
    }
}

fn contains(link: &TerminalLink, point: MousePosition) -> bool {
    link.cells.iter().any(|cell| cell.position == point)
}

#[cfg(test)]
mod tests {
    use super::*;
    use huterm_protocol::{LinkCell, LinkSource};
    fn point(column: u32) -> MousePosition {
        MousePosition { row: 0, column }
    }
    fn link() -> TerminalLink {
        TerminalLink {
            destination: "https://x.test/".into(),
            source: LinkSource::PlainText,
            cells: vec![LinkCell {
                position: point(0),
                text: "x".into(),
            }],
        }
    }
    fn ready() -> Links {
        let mut state = Links::default();
        assert!(state.update(Some(point(0)), true, (0.0, 0.0)));
        state.publish(state.intent(), Some(LinkLookup::Match(link())));
        state
    }
    #[test]
    fn stationary_modifiers_start_lookup_and_inflight_release_rejects_reply() {
        let mut state = Links::default();
        assert!(!state.update(Some(point(0)), false, (0.0, 0.0)));
        assert!(state.update(Some(point(0)), true, (0.0, 0.0)));
        let intent = state.intent();
        assert!(!state.update(Some(point(0)), false, (0.0, 0.0)));
        state.publish(intent, Some(LinkLookup::Match(link())));
        assert!(state.hover().is_none());
    }
    #[test]
    fn owned_press_survives_unrelated_snapshots_and_opens_captured_target_once()
    {
        let mut state = ready();
        assert!(state.press(point(0), (0.0, 0.0)));
        for _ in 0..5 {
            state.publish(state.intent(), Some(LinkLookup::Match(link())));
        }
        assert_eq!(
            state.release(Some(point(0)), true),
            (true, Some("https://x.test/".into()))
        );
        assert_eq!(state.release(Some(point(0)), true), (false, None));
    }
    #[test]
    fn changed_targets_labels_spans_missing_results_and_cancel_never_rearm() {
        let mut changed_target = link();
        changed_target.destination = "https://other.test/".into();
        let mut changed_label = link();
        changed_label.cells[0].text = "y".into();
        let mut moved = link();
        moved.cells[0].position = point(1);
        for outcome in [
            Some(LinkLookup::Match(changed_target)),
            Some(LinkLookup::Match(changed_label)),
            Some(LinkLookup::Match(moved)),
            Some(LinkLookup::NoMatch),
            Some(LinkLookup::ScanLimit),
            Some(LinkLookup::Unavailable),
            None,
        ] {
            let mut state = ready();
            state.press(point(0), (0.0, 0.0));
            state.publish(state.intent(), outcome);
            state.publish(state.intent(), Some(LinkLookup::Match(link())));
            assert_eq!(state.release(Some(point(0)), true), (true, None));
        }
        let mut state = ready();
        state.press(point(0), (0.0, 0.0));
        assert!(state.cancel_press());
        assert_eq!(state.release(Some(point(0)), true), (true, None));
        assert!(!state.cancel_press());
    }
    #[test]
    fn pending_click_is_consumed_and_no_match_allows_selection() {
        let mut state = Links::default();
        state.update(Some(point(0)), true, (0.0, 0.0));
        assert!(state.press(point(0), (0.0, 0.0)));
        state.publish(state.intent(), Some(LinkLookup::Match(link())));
        assert_eq!(state.release(Some(point(0)), true), (true, None));
        state.update(Some(point(0)), true, (0.0, 0.0));
        state.publish(state.intent(), Some(LinkLookup::NoMatch));
        assert!(!state.press(point(0), (0.0, 0.0)));
    }
    #[test]
    fn repeated_cell_motion_reuses_results_and_drag_modifier_loss_cancel() {
        let mut state = ready();
        for _ in 0..1000 {
            assert!(!state.update(Some(point(0)), true, (0.2, 0.2)));
        }
        state.press(point(0), (0.0, 0.0));
        state.update(Some(point(0)), true, (5.0, 0.0));
        assert_eq!(state.release(Some(point(0)), true), (true, None));
        state = ready();
        state.press(point(0), (0.0, 0.0));
        state.update(Some(point(0)), false, (0.0, 0.0));
        assert_eq!(state.release(Some(point(0)), true), (true, None));
    }
}
