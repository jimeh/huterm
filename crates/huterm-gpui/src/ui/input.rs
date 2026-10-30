//! Routes the catalog's text-editing commands to a `huterm_ui` input, and
//! builds the `TextField` key context its bindings match in.

use gpui::{Context, Div, InteractiveElement as _, KeyContext, div};
use huterm_protocol::{CommandError, CommandId, CommandInvocation, ids};
use huterm_ui::components::input::Input;

use crate::commands::{InvokeTerminal, invoke};
use crate::keymap::TEXT_FIELD_CONTEXT;

/// The field verbs: commands bound under `TextField` whose meaning is the
/// field owner's, such as committing on Enter. An owner that gives them
/// none, as the palette does, leaves them to its own bindings.
pub(crate) const FIELD_VERBS: [CommandId; 6] = [
    ids::TEXT_CONFIRM,
    ids::TEXT_CANCEL,
    ids::TEXT_NEXT,
    ids::TEXT_PREVIOUS,
    ids::TEXT_PAGE_DOWN,
    ids::TEXT_PAGE_UP,
];

/// Whether `id` is one of the [`FIELD_VERBS`].
pub(crate) fn is_field_verb(id: CommandId) -> bool {
    FIELD_VERBS.contains(&id)
}

/// An element in the `TextField` context, to hold a field and anything
/// that sits with it, such as an input group's addons. The `text_*`
/// bindings match only inside one.
///
/// The Edit menu's Copy, Paste, and Select All are terminal commands; inside
/// this element they run as the field's `text_copy`, `text_paste`, and
/// `text_select_all` through the owner's routing, so the menu items act on
/// a focused field and are enabled only there.
pub(crate) fn context() -> Div {
    context_with(&[])
}

/// [`context`] with `extra` identifiers on the same node, as the palette's
/// search adds `Palette`, so a binding whose `when` names both matches
/// there.
pub(crate) fn context_with(extra: &[&str]) -> Div {
    let mut context = KeyContext::default();
    context.add(TEXT_FIELD_CONTEXT);
    for identifier in extra {
        context.add(identifier.to_string());
    }
    div().key_context(context).on_action(
        |action: &InvokeTerminal, window, cx| {
            let Some(id) = edit_menu_command(action.0.id) else {
                cx.propagate();
                return;
            };
            window.dispatch_action(invoke(id).into_boxed(), cx);
        },
    )
}

/// The field command the Edit menu's terminal command `id` stands for.
fn edit_menu_command(id: CommandId) -> Option<CommandId> {
    match id {
        ids::COPY => Some(ids::TEXT_COPY),
        ids::PASTE => Some(ids::TEXT_PASTE),
        ids::SELECT_ALL => Some(ids::TEXT_SELECT_ALL),
        _ => None,
    }
}

/// Runs one text-editing catalog command on `input`.
///
/// # Errors
/// Returns [`CommandError::UnknownCommand`] for commands the field does not
/// own, including the [`FIELD_VERBS`], which are the owner's.
pub(crate) fn run(
    input: &mut Input,
    invocation: &CommandInvocation,
    cx: &mut Context<'_, Input>,
) -> Result<(), CommandError> {
    let select = invocation.bool("select").unwrap_or(false);
    match invocation.id {
        ids::TEXT_DELETE_BACKWARD => input.delete_backward(cx),
        ids::TEXT_DELETE_FORWARD => input.delete_forward(cx),
        ids::TEXT_DELETE_WORD_BACKWARD => input.delete_word_backward(cx),
        ids::TEXT_DELETE_WORD_FORWARD => input.delete_word_forward(cx),
        ids::TEXT_DELETE_LINE_START => input.delete_to_start(cx),
        ids::TEXT_MOVE_LEFT => input.move_left(select, cx),
        ids::TEXT_MOVE_RIGHT => input.move_right(select, cx),
        ids::TEXT_MOVE_WORD_LEFT => input.move_word_left(select, cx),
        ids::TEXT_MOVE_WORD_RIGHT => input.move_word_right(select, cx),
        ids::TEXT_LINE_START => input.move_to_start(select, cx),
        ids::TEXT_LINE_END => input.move_to_end(select, cx),
        ids::TEXT_SELECT_ALL => input.select_all(cx),
        ids::TEXT_CUT => input.cut(cx),
        ids::TEXT_COPY => input.copy(cx),
        ids::TEXT_PASTE => input.paste(cx),
        other => return Err(CommandError::UnknownCommand(other)),
    }
    Ok(())
}
