//! Routes the catalog's text-editing commands to a `huterm_ui` input.

use gpui::Context;
use huterm_protocol::{CommandError, CommandInvocation, ids};
use huterm_ui::components::input::Input;

/// Runs one text-editing catalog command on `input`.
///
/// # Errors
/// Returns [`CommandError::UnknownCommand`] for commands the field does not
/// own.
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
