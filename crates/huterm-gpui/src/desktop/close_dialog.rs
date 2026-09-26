//! The close confirmation for a tab, several tabs, a window, or Quit.
//!
//! [`build_close_dialog`] turns the assessed processes into the exact copy
//! and rows the dialog shows, and [`render_close_dialog`] draws that model.
//! The owner keeps the focused button, routes the `dialog_*` catalog commands
//! under its `confirming` context, and maps core's close assessment onto
//! [`CloseDialogInput`].

use gpui::{App, ClickEvent, Div, Pixels, Size, Window, div, prelude::*, px};

use super::overlay::{
    Swatch, key_cap, mix, mono_font_family, raised_panel, scrim, severity_mark,
};

/// What the confirmation closes.
#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) enum CloseDialogTarget {
    Tab { title: String },
    Tabs { count: usize },
    Window,
    Application,
}

/// One process the close would end.
#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) struct ProcessRow {
    /// The display command, such as `cargo` or `nvim`.
    pub(crate) command: String,
    pub(crate) pid: u32,
    /// Holds the terminal foreground rather than running as a background
    /// job.
    pub(crate) foreground: bool,
    pub(crate) command_line: Option<String>,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) enum ProcessGroupState {
    Known(Vec<ProcessRow>),
    /// The tab's process table could not be read.
    Unknown,
}

/// The processes of one busy tab. `tab_title` is set for dialogs covering
/// more than one tab, where rows group under headings.
#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) struct ProcessGroup {
    pub(crate) tab_title: Option<String>,
    pub(crate) state: ProcessGroupState,
}

/// Every group is a busy tab: idle tabs are not passed.
#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) struct CloseDialogInput {
    pub(crate) target: CloseDialogTarget,
    pub(crate) groups: Vec<ProcessGroup>,
}

/// The header mark: `!` for known processes, `?` when any state is unknown.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum DialogMark {
    Exclamation,
    Question,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) enum DialogRow {
    Heading(String),
    Process(ProcessRow),
    Unknown { tab_title: String },
    More(usize),
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) struct CloseDialogModel {
    pub(crate) title: String,
    pub(crate) subtitle: String,
    pub(crate) primary_label: String,
    pub(crate) mark: DialogMark,
    pub(crate) rows: Vec<DialogRow>,
}

/// Process and unknown-state rows shown before "and N more".
pub(crate) const MAX_PROCESS_ROWS: usize = 6;

fn plural(count: usize, one: &str, many: &str) -> String {
    format!("{count} {}", if count == 1 { one } else { many })
}

fn unknown_sentence(unknown_tabs: usize, tail: &str) -> Option<String> {
    match unknown_tabs {
        0 => None,
        1 => Some(format!(
            "One tab's process state is unavailable, so it may have running \
             jobs{tail}."
        )),
        count => Some(format!(
            "{count} tabs' process state is unavailable, so they may have \
             running jobs{tail}."
        )),
    }
}

/// The title, subtitle, and primary label for `target`, given how many
/// tabs are busy, how many processes are known, and how many tabs have an
/// unknown state.
fn dialog_copy(
    target: &CloseDialogTarget,
    busy: usize,
    processes: usize,
    unknown_tabs: usize,
) -> (String, String, &'static str) {
    let mut sentences: Vec<String> = Vec::new();
    let (title, primary_label) = match target {
        CloseDialogTarget::Tab { title } => {
            if unknown_tabs > 0 {
                sentences.push(
                    "Huterm could not read this tab's process state, so it \
                     may have running jobs."
                        .to_owned(),
                );
            } else {
                sentences.push(format!(
                    "Closing this tab ends {}.",
                    plural(processes, "running process", "running processes")
                ));
            }
            (format!("Close \"{title}\"?"), "Close Tab")
        }
        CloseDialogTarget::Tabs { count } => {
            if busy >= *count {
                sentences.push(format!("All {count} have running processes."));
            } else {
                sentences.push(format!(
                    "{busy} of them {} running processes.",
                    if busy == 1 { "has" } else { "have" }
                ));
            }
            if processes > 0 {
                sentences.push(format!(
                    "Closing ends {}.",
                    plural(processes, "process", "processes")
                ));
            }
            sentences.extend(unknown_sentence(unknown_tabs, ""));
            (format!("Close {count} tabs?"), "Close Tabs")
        }
        CloseDialogTarget::Window => {
            if processes > 0 {
                sentences.push(format!(
                    "This ends {} and closes the window's session.",
                    plural(processes, "running process", "running processes")
                ));
            } else {
                sentences.push("This closes the window's session.".to_owned());
            }
            sentences.extend(unknown_sentence(unknown_tabs, ""));
            ("Close this window?".to_owned(), "Close Window")
        }
        CloseDialogTarget::Application => {
            if processes > 0 {
                sentences.push(format!(
                    "This ends {} in every window.",
                    plural(processes, "running process", "running processes")
                ));
            }
            sentences.extend(unknown_sentence(
                unknown_tabs,
                if processes > 0 { " too" } else { "" },
            ));
            ("Quit Huterm?".to_owned(), "Quit")
        }
    };
    (title, sentences.join(" "), primary_label)
}

/// Headings and at most [`MAX_PROCESS_ROWS`] process or unknown rows, then
/// "and N more". A group whose rows are all hidden gets no heading.
fn dialog_rows(
    groups: &[&ProcessGroup],
    fallback_title: &str,
) -> Vec<DialogRow> {
    let mut rows = Vec::new();
    let mut shown = 0;
    let mut hidden = 0;
    for group in groups {
        if let Some(heading) = &group.tab_title
            && shown < MAX_PROCESS_ROWS
        {
            rows.push(DialogRow::Heading(heading.clone()));
        }
        match &group.state {
            ProcessGroupState::Unknown => {
                if shown < MAX_PROCESS_ROWS {
                    shown += 1;
                    rows.push(DialogRow::Unknown {
                        tab_title: group
                            .tab_title
                            .clone()
                            .unwrap_or_else(|| fallback_title.to_owned()),
                    });
                } else {
                    hidden += 1;
                }
            }
            ProcessGroupState::Known(processes) => {
                for process in processes {
                    if shown < MAX_PROCESS_ROWS {
                        shown += 1;
                        rows.push(DialogRow::Process(process.clone()));
                    } else {
                        hidden += 1;
                    }
                }
            }
        }
    }
    if hidden > 0 {
        rows.push(DialogRow::More(hidden));
    }
    rows
}

/// Builds the dialog copy and rows from the assessed processes. Every
/// group is a busy tab; unknown-state tabs come first.
pub(crate) fn build_close_dialog(input: &CloseDialogInput) -> CloseDialogModel {
    let mut groups: Vec<&ProcessGroup> = input.groups.iter().collect();
    // The sort is stable, so known groups keep their tab order.
    groups.sort_by_key(|group| group.state != ProcessGroupState::Unknown);
    let unknown_tabs = groups
        .iter()
        .filter(|group| group.state == ProcessGroupState::Unknown)
        .count();
    let processes: usize = groups
        .iter()
        .map(|group| match &group.state {
            ProcessGroupState::Known(rows) => rows.len(),
            ProcessGroupState::Unknown => 0,
        })
        .sum();
    // Only tabs with known processes are busy; unknown-state tabs get their
    // own sentence.
    let busy = groups.len() - unknown_tabs;
    let (title, subtitle, primary_label) =
        dialog_copy(&input.target, busy, processes, unknown_tabs);
    let fallback_title = match &input.target {
        CloseDialogTarget::Tab { title } => title.as_str(),
        _ => "This tab",
    };
    CloseDialogModel {
        title,
        subtitle,
        primary_label: primary_label.to_owned(),
        mark: if unknown_tabs > 0 {
            DialogMark::Question
        } else {
            DialogMark::Exclamation
        },
        rows: dialog_rows(&groups, fallback_title),
    }
}

// ---- presentation ---------------------------------------------------------

/// Which footer button holds keyboard focus. The primary button has it when
/// a confirmation opens, so Enter confirms and Escape cancels.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub(crate) enum DialogFocus {
    Cancel,
    #[default]
    Primary,
}

impl DialogFocus {
    /// The other button: with two buttons, next and previous both wrap to it.
    pub(crate) fn toggled(self) -> Self {
        match self {
            Self::Cancel => Self::Primary,
            Self::Primary => Self::Cancel,
        }
    }
}

const PANEL_WIDTH: f32 = 540.0;
const LIST_MAX_HEIGHT: f32 = 280.0;

fn dialog_row(row: &DialogRow, swatch: Swatch) -> Div {
    let base = div()
        .relative()
        .flex()
        .flex_none()
        .items_center()
        .gap(px(10.0))
        .pl(px(14.0))
        .pr(px(10.0))
        .rounded(px(6.0));
    let bar = |color| {
        div()
            .absolute()
            .left(px(3.0))
            .top(px(10.0))
            .bottom(px(10.0))
            .w(px(3.0))
            .rounded(px(2.0))
            .bg(color)
    };
    let main = || div().flex_basis(px(0.0)).flex_grow().min_w_0();
    let detail = |text: String| {
        div()
            .text_size(px(11.5))
            .text_color(swatch.muted)
            .truncate()
            .child(text)
    };
    match row {
        DialogRow::Heading(title) => base
            .min_h(px(24.0))
            .pt(px(6.0))
            .text_size(px(11.5))
            .text_color(swatch.muted)
            .child(title.clone()),
        DialogRow::Process(process) => {
            let role = if process.foreground {
                "Foreground"
            } else {
                "Background job"
            };
            let line = match &process.command_line {
                Some(command_line) => format!("{role} · {command_line}"),
                None => role.to_owned(),
            };
            base.min_h(px(40.0))
                .py(px(4.0))
                .when(process.foreground, |row| row.child(bar(swatch.accent)))
                .child(
                    main()
                        .child(
                            div()
                                .font_family(mono_font_family())
                                .font_weight(gpui::FontWeight::SEMIBOLD)
                                .text_size(px(13.0))
                                .child(process.command.clone()),
                        )
                        .child(detail(line)),
                )
                .child(
                    div()
                        .flex_none()
                        .font_family(mono_font_family())
                        .text_size(px(11.5))
                        .text_color(swatch.muted)
                        .child(process.pid.to_string()),
                )
        }
        DialogRow::Unknown { tab_title } => base
            .min_h(px(40.0))
            .py(px(4.0))
            .child(bar(swatch.warning))
            .child(
                main()
                    .child(div().text_size(px(13.0)).child(tab_title.clone()))
                    .child(detail(
                        "Process state unavailable; it may have running jobs"
                            .to_owned(),
                    )),
            )
            .child(
                div()
                    .flex_none()
                    .px(px(6.0))
                    .py(px(1.0))
                    .rounded(px(4.0))
                    .border_1()
                    .border_color(swatch.warning.opacity(0.45))
                    .text_size(px(10.5))
                    .text_color(swatch.warning)
                    .child("unknown"),
            ),
        DialogRow::More(count) => base
            .min_h(px(28.0))
            .text_size(px(12.0))
            .text_color(swatch.muted)
            .child(format!("and {count} more")),
    }
}

fn button(swatch: Swatch, focused: bool) -> Div {
    div()
        .flex()
        .flex_none()
        .items_center()
        .gap(px(8.0))
        .h(px(28.0))
        .pl(px(12.0))
        .pr(px(8.0))
        .rounded(px(6.0))
        .text_size(px(12.5))
        .font_weight(gpui::FontWeight::MEDIUM)
        .cursor_pointer()
        .when(focused, |button| button.shadow(swatch.focus_ring()))
}

/// `Cancel esc` and the destructive primary button, nothing else.
fn dialog_footer(
    model: &CloseDialogModel,
    focus: DialogFocus,
    swatch: Swatch,
    on_cancel: impl Fn(&ClickEvent, &mut Window, &mut App) + 'static,
    on_confirm: impl Fn(&ClickEvent, &mut Window, &mut App) + 'static,
) -> Div {
    let danger = mix(swatch.danger, swatch.bg, 0.16);
    div()
        .flex()
        .flex_none()
        .items_center()
        .justify_end()
        .gap(px(8.0))
        .pl(px(14.0))
        .pr(px(10.0))
        .py(px(8.0))
        .border_t_1()
        .border_color(swatch.line)
        .bg(swatch.fg.opacity(0.03))
        .child(
            button(swatch, focus == DialogFocus::Cancel)
                .id("cancel-close")
                .border_1()
                .border_color(swatch.line_strong)
                .text_color(swatch.fg)
                .hover(move |button| button.bg(swatch.hover))
                .on_click(on_cancel)
                .child("Cancel")
                .child(key_cap("esc", swatch)),
        )
        .child(
            button(swatch, focus == DialogFocus::Primary)
                .id("confirm-close")
                .bg(danger)
                .text_color(swatch.bg)
                .hover(move |button| button.bg(swatch.danger))
                .on_click(on_confirm)
                .child(model.primary_label.clone())
                .child(
                    div()
                        .px(px(5.0))
                        .py(px(1.0))
                        .rounded(px(4.0))
                        .bg(swatch.bg.opacity(0.18))
                        .border_1()
                        .border_color(swatch.bg.opacity(0.35))
                        .text_size(px(10.5))
                        .child("↩"),
                ),
        )
}

/// The dialog over its scrim, filling the parent. The panel has an explicit
/// viewport-clamped width, and its text and button containers do not
/// shrink, so wrapped text measures correctly. `on_cancel` and
/// `on_confirm` receive the clicks on `cancel-close` and `confirm-close`.
pub(crate) fn render_close_dialog(
    model: &CloseDialogModel,
    focus: DialogFocus,
    viewport: Size<Pixels>,
    swatch: Swatch,
    on_cancel: impl Fn(&ClickEvent, &mut Window, &mut App) + 'static,
    on_confirm: impl Fn(&ClickEvent, &mut Window, &mut App) + 'static,
) -> Div {
    let width = (viewport.width - px(32.0)).clamp(px(0.0), px(PANEL_WIDTH));
    let (glyph, fill) = match model.mark {
        DialogMark::Exclamation => ("!", swatch.danger),
        DialogMark::Question => ("?", swatch.warning),
    };
    let header = div()
        .flex()
        .flex_none()
        .items_start()
        .gap(px(10.0))
        .px(px(14.0))
        .py(px(12.0))
        .border_b_1()
        .border_color(swatch.line)
        .child(severity_mark(glyph, fill, swatch))
        .child(
            div()
                .flex_basis(px(0.0))
                .flex_grow()
                .min_w_0()
                .child(
                    div()
                        .text_size(px(14.0))
                        .font_weight(gpui::FontWeight::SEMIBOLD)
                        .child(model.title.clone()),
                )
                .child(
                    div()
                        .mt(px(1.0))
                        .text_size(px(12.0))
                        .text_color(swatch.muted)
                        .child(model.subtitle.clone()),
                ),
        );
    let list = div()
        .id("close-dialog-list")
        .flex()
        .flex_col()
        .flex_none()
        .p(px(6.0))
        .max_h(px(LIST_MAX_HEIGHT))
        .overflow_y_scroll()
        .children(model.rows.iter().map(|row| dialog_row(row, swatch)));
    scrim(swatch)
        .flex()
        .items_center()
        .justify_center()
        .on_mouse_down(gpui::MouseButton::Left, |_, _, cx| {
            cx.stop_propagation();
        })
        .on_mouse_down(gpui::MouseButton::Right, |_, _, cx| {
            cx.stop_propagation();
        })
        .on_mouse_up(gpui::MouseButton::Left, |_, _, cx| {
            cx.stop_propagation();
        })
        .child(
            raised_panel(swatch, 10.0)
                .w(width)
                .flex_none()
                .flex()
                .flex_col()
                .text_color(swatch.fg)
                .child(header)
                .child(list)
                .child(dialog_footer(
                    model, focus, swatch, on_cancel, on_confirm,
                )),
        )
}

#[cfg(test)]
mod tests {
    use super::*;

    fn process(command: &str, pid: u32, foreground: bool) -> ProcessRow {
        ProcessRow {
            command: command.to_owned(),
            pid,
            foreground,
            command_line: Some(format!("{command} --flag")),
        }
    }

    fn known(title: Option<&str>, rows: Vec<ProcessRow>) -> ProcessGroup {
        ProcessGroup {
            tab_title: title.map(str::to_owned),
            state: ProcessGroupState::Known(rows),
        }
    }

    fn unknown(title: Option<&str>) -> ProcessGroup {
        ProcessGroup {
            tab_title: title.map(str::to_owned),
            state: ProcessGroupState::Unknown,
        }
    }

    fn build(
        target: CloseDialogTarget,
        groups: Vec<ProcessGroup>,
    ) -> CloseDialogModel {
        build_close_dialog(&CloseDialogInput { target, groups })
    }

    #[test]
    fn one_tab_names_the_tab_and_counts_its_processes() {
        let model = build(
            CloseDialogTarget::Tab {
                title: "cargo build".to_owned(),
            },
            vec![known(
                None,
                vec![
                    process("cargo", 48213, true),
                    process("tail", 48190, false),
                ],
            )],
        );
        assert_eq!(model.title, "Close \"cargo build\"?");
        assert_eq!(model.primary_label, "Close Tab");
        assert_eq!(
            model.subtitle,
            "Closing this tab ends 2 running processes."
        );
        assert_eq!(model.mark, DialogMark::Exclamation);
        assert_eq!(model.rows.len(), 2, "no heading for a single tab");
        assert!(
            matches!(&model.rows[0], DialogRow::Process(row) if row.foreground)
        );

        let single = build(
            CloseDialogTarget::Tab {
                title: "nvim".to_owned(),
            },
            vec![known(None, vec![process("nvim", 1, true)])],
        );
        assert_eq!(single.subtitle, "Closing this tab ends 1 running process.");
    }

    #[test]
    fn an_unknown_tab_explains_itself_and_uses_the_question_mark() {
        let model = build(
            CloseDialogTarget::Tab {
                title: "ssh build-host".to_owned(),
            },
            vec![unknown(None)],
        );
        assert_eq!(model.mark, DialogMark::Question);
        assert_eq!(
            model.subtitle,
            "Huterm could not read this tab's process state, so it may have \
             running jobs."
        );
        assert_eq!(
            model.rows,
            [DialogRow::Unknown {
                tab_title: "ssh build-host".to_owned()
            }]
        );
    }

    #[test]
    fn several_tabs_say_how_many_are_busy() {
        let some = build(
            CloseDialogTarget::Tabs { count: 3 },
            vec![
                known(Some("cargo build"), vec![process("cargo", 1, true)]),
                known(Some("nvim"), vec![process("nvim", 2, true)]),
            ],
        );
        assert_eq!(some.title, "Close 3 tabs?");
        assert_eq!(some.primary_label, "Close Tabs");
        assert_eq!(
            some.subtitle,
            "2 of them have running processes. Closing ends 2 processes."
        );
        let one = build(
            CloseDialogTarget::Tabs { count: 2 },
            vec![known(Some("nvim"), vec![process("nvim", 2, true)])],
        );
        assert_eq!(
            one.subtitle,
            "1 of them has running processes. Closing ends 1 process."
        );
        let all = build(
            CloseDialogTarget::Tabs { count: 2 },
            vec![
                known(Some("cargo build"), vec![process("cargo", 1, true)]),
                known(Some("nvim"), vec![process("nvim", 2, true)]),
            ],
        );
        assert_eq!(
            all.subtitle,
            "All 2 have running processes. Closing ends 2 processes."
        );
        // An unknown-state tab is not counted as busy: it has its own
        // sentence.
        let mixed = build(
            CloseDialogTarget::Tabs { count: 2 },
            vec![
                known(Some("cargo build"), vec![process("cargo", 1, true)]),
                unknown(Some("ssh build-host")),
            ],
        );
        assert_eq!(
            mixed.subtitle,
            "1 of them has running processes. Closing ends 1 process. One \
             tab's process state is unavailable, so it may have running jobs."
        );
        assert_eq!(mixed.mark, DialogMark::Question);
    }

    #[test]
    fn window_and_quit_describe_their_scope() {
        let window = build(
            CloseDialogTarget::Window,
            vec![known(Some("cargo build"), vec![process("cargo", 1, true)])],
        );
        assert_eq!(window.title, "Close this window?");
        assert_eq!(window.primary_label, "Close Window");
        assert_eq!(
            window.subtitle,
            "This ends 1 running process and closes the window's session."
        );
        let window_unknown = build(
            CloseDialogTarget::Window,
            vec![unknown(Some("ssh")), unknown(Some("mosh"))],
        );
        assert_eq!(
            window_unknown.subtitle,
            "This closes the window's session. 2 tabs' process state is \
             unavailable, so they may have running jobs."
        );
        let quit = build(
            CloseDialogTarget::Application,
            vec![
                known(
                    Some("cargo build"),
                    vec![process("cargo", 1, true), process("rustc", 2, true)],
                ),
                unknown(Some("ssh")),
            ],
        );
        assert_eq!(quit.title, "Quit Huterm?");
        assert_eq!(quit.primary_label, "Quit");
        assert_eq!(
            quit.subtitle,
            "This ends 2 running processes in every window. One tab's process \
             state is unavailable, so it may have running jobs too."
        );
    }

    #[test]
    fn unknown_groups_come_first_under_their_headings() {
        let model = build(
            CloseDialogTarget::Window,
            vec![
                known(Some("cargo build"), vec![process("cargo", 1, true)]),
                unknown(Some("ssh build-host")),
            ],
        );
        assert_eq!(
            model.rows,
            [
                DialogRow::Heading("ssh build-host".to_owned()),
                DialogRow::Unknown {
                    tab_title: "ssh build-host".to_owned()
                },
                DialogRow::Heading("cargo build".to_owned()),
                DialogRow::Process(process("cargo", 1, true)),
            ]
        );
    }

    #[test]
    fn rows_cap_at_six_and_count_the_rest() {
        let model = build(
            CloseDialogTarget::Application,
            vec![
                known(
                    Some("busy"),
                    (1..=5).map(|pid| process("job", pid, pid == 1)).collect(),
                ),
                known(
                    Some("also busy"),
                    vec![process("a", 10, true), process("b", 11, false)],
                ),
                known(Some("hidden"), vec![process("c", 20, true)]),
            ],
        );
        let headings = model
            .rows
            .iter()
            .filter(|row| matches!(row, DialogRow::Heading(_)))
            .count();
        let processes = model
            .rows
            .iter()
            .filter(|row| matches!(row, DialogRow::Process(_)))
            .count();
        assert_eq!(processes, MAX_PROCESS_ROWS);
        assert_eq!(headings, 2, "a fully hidden group has no heading");
        assert_eq!(model.rows.last(), Some(&DialogRow::More(2)));
        assert_eq!(
            model.subtitle,
            "This ends 8 running processes in every window."
        );
    }
}
