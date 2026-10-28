//! Roving focus: a group whose items each take focus but that is one tab
//! stop, as WAI-ARIA's tabs and radio group patterns are.
//!
//! The owner gives every item its own focus handle. Only one item is a tab
//! stop ([`stop`]): the focused item, else the selected one, else the
//! first. The owner routes its move commands (the arrow keys, Home, and End
//! by default) through [`move_focus`], which focuses the target item and
//! returns its index, so the owner can select it as well. [`Move::along`]
//! turns an arrow into a move for a group laid out along one axis.

use gpui::{App, Axis, FocusHandle, Window};

/// An arrow key, as the owner's command names it.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Arrow {
    /// Up.
    Up,
    /// Down.
    Down,
    /// Left.
    Left,
    /// Right.
    Right,
}

/// Where focus moves within a group.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Move {
    /// The next item, wrapping from the last to the first.
    Next,
    /// The previous item, wrapping from the first to the last.
    Previous,
    /// The first item.
    First,
    /// The last item.
    Last,
}

impl Move {
    /// The move `arrow` makes in a group laid out along `axis`: Up and Down
    /// move a vertical group, Left and Right a horizontal one, and the
    /// other arrows nothing. A group with no `axis`, such as a radio group,
    /// moves on every arrow: Up and Left back, Down and Right on.
    #[must_use]
    pub fn along(axis: Option<Axis>, arrow: Arrow) -> Option<Self> {
        match (axis, arrow) {
            (Some(Axis::Vertical) | None, Arrow::Up)
            | (Some(Axis::Horizontal) | None, Arrow::Left) => {
                Some(Self::Previous)
            }
            (Some(Axis::Vertical) | None, Arrow::Down)
            | (Some(Axis::Horizontal) | None, Arrow::Right) => Some(Self::Next),
            _ => None,
        }
    }
}

/// The index `step` reaches among `count` items from `from`; from no item,
/// Next and Previous start at the first and last. `None` when the group is
/// empty.
#[must_use]
pub fn target(count: usize, from: Option<usize>, step: Move) -> Option<usize> {
    let last = count.checked_sub(1)?;
    Some(match (step, from) {
        (Move::First, _) | (Move::Next, None) => 0,
        (Move::Last, _) | (Move::Previous, None) => last,
        (Move::Next, Some(from)) => {
            if from >= last {
                0
            } else {
                from + 1
            }
        }
        (Move::Previous, Some(from)) => {
            if from == 0 {
                last
            } else {
                (from - 1).min(last)
            }
        }
    })
}

/// The index of the item in `handles` that has focus.
#[must_use]
pub fn focused(handles: &[FocusHandle], window: &Window) -> Option<usize> {
    handles.iter().position(|handle| handle.is_focused(window))
}

/// The group's one tab stop: the `focused` item, else the `selected` one,
/// else the first.
#[must_use]
pub fn stop(focused: Option<usize>, selected: Option<usize>) -> usize {
    focused.or(selected).unwrap_or(0)
}

/// Moves focus from the focused item in `handles` by `step`, and returns
/// the index it moved to. Nothing moves, and `None` returns, while no item
/// has focus, because only a focused group receives its move commands.
pub fn move_focus(
    handles: &[FocusHandle],
    step: Move,
    window: &mut Window,
    cx: &mut App,
) -> Option<usize> {
    let from = focused(handles, window)?;
    let index = target(handles.len(), Some(from), step)?;
    handles[index].focus(window, cx);
    Some(index)
}

#[cfg(test)]
mod tests {
    use gpui::Axis;

    use super::{Arrow, Move, stop, target};

    #[test]
    fn moves_wrap_at_both_ends_and_jump_to_either_end() {
        assert_eq!(target(3, Some(0), Move::Next), Some(1));
        assert_eq!(target(3, Some(2), Move::Next), Some(0));
        assert_eq!(target(3, Some(0), Move::Previous), Some(2));
        assert_eq!(target(3, Some(2), Move::Previous), Some(1));
        assert_eq!(target(3, Some(1), Move::First), Some(0));
        assert_eq!(target(3, Some(1), Move::Last), Some(2));
        assert_eq!(target(3, None, Move::Next), Some(0));
        assert_eq!(target(3, None, Move::Previous), Some(2));
        assert_eq!(target(0, None, Move::First), None);
    }

    #[test]
    fn arrows_move_only_along_the_groups_axis() {
        let vertical = Some(Axis::Vertical);
        assert_eq!(Move::along(vertical, Arrow::Up), Some(Move::Previous));
        assert_eq!(Move::along(vertical, Arrow::Down), Some(Move::Next));
        assert_eq!(Move::along(vertical, Arrow::Left), None);
        assert_eq!(Move::along(vertical, Arrow::Right), None);
        let horizontal = Some(Axis::Horizontal);
        assert_eq!(Move::along(horizontal, Arrow::Left), Some(Move::Previous));
        assert_eq!(Move::along(horizontal, Arrow::Right), Some(Move::Next));
        assert_eq!(Move::along(horizontal, Arrow::Up), None);
        assert_eq!(Move::along(None, Arrow::Up), Some(Move::Previous));
        assert_eq!(Move::along(None, Arrow::Right), Some(Move::Next));
    }

    #[test]
    fn the_tab_stop_prefers_focus_then_the_selection() {
        assert_eq!(stop(Some(2), Some(1)), 2);
        assert_eq!(stop(None, Some(1)), 1);
        assert_eq!(stop(None, None), 0);
    }
}
