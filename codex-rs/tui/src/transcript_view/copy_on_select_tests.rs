//! Automatic copies deselect according to terminal policy and the user's explicit preference.

use super::*;
use crate::clipboard_copy::CopyStatus;
use crate::history_cell::AgentMarkdownCell;
use crate::transcript_view::tests::cell;
use crate::transcript_view::tests::render;
use crate::transcript_view::tests::text;
use crossterm::event::KeyModifiers;
use crossterm::event::MouseButton;
use crossterm::event::MouseEvent;
use crossterm::event::MouseEventKind;
use pretty_assertions::assert_eq;
use ratatui::style::Modifier;
use std::time::Instant;

fn mouse(kind: MouseEventKind, column: u16) -> MouseEvent {
    MouseEvent {
        kind,
        column,
        row: 0,
        modifiers: KeyModifiers::NONE,
    }
}

#[test]
fn herdr_mouse_drag_copies_both_endpoint_cells() {
    for (text, width, from, to) in [
        ("123", 8, (0, 0), (2, 0)),
        ("123", 8, (2, 0), (0, 0)),
        ("123", 8, (0, 0), (7, 0)),
        ("1界3", 8, (0, 0), (3, 0)),
        ("1界3", 8, (3, 0), (0, 0)),
        ("1界", 8, (0, 0), (2, 0)),
        ("1e\u{301}3", 8, (0, 0), (2, 0)),
        ("1e\u{301}3", 8, (2, 0), (0, 0)),
        ("123456", 3, (0, 0), (2, 1)),
    ] {
        let cells = vec![cell(text)];
        let mut view = TranscriptView {
            copy_on_select: true,
            ..Default::default()
        }
        .with_herdr_mouse_selection(true);
        render(&mut view, &cells, width, /*height*/ 3);
        for (kind, (column, row)) in [
            (MouseEventKind::Down(MouseButton::Left), from),
            (MouseEventKind::Drag(MouseButton::Left), to),
        ] {
            view.handle_mouse(
                MouseEvent {
                    row,
                    ..mouse(kind, column)
                },
                &cells,
            );
        }
        let action = view.handle_mouse(
            MouseEvent {
                row: to.1,
                ..mouse(MouseEventKind::Up(MouseButton::Left), to.0)
            },
            &cells,
        );
        let Some(ViewAction::CopyOnSelect(copied)) = action else {
            panic!("drag must request an automatic copy");
        };
        assert_eq!(copied, text, "drag from {from:?} to {to:?}");
    }
}

#[test]
fn herdr_stationary_click_does_not_select_or_copy_text() {
    let cells = vec![cell("123")];
    let mut view = TranscriptView {
        copy_on_select: true,
        ..Default::default()
    }
    .with_herdr_mouse_selection(true);
    render(&mut view, &cells, /*width*/ 8, /*height*/ 1);
    view.handle_mouse(mouse(MouseEventKind::Down(MouseButton::Left), 1), &cells);
    assert!(view.selected_text(&cells).is_none());
    assert!(matches!(
        view.handle_mouse(mouse(MouseEventKind::Up(MouseButton::Left), 1), &cells),
        Some(ViewAction::Changed)
    ));
    assert!(view.selected_text(&cells).is_none());
}

#[test]
fn transcript_reset_preserves_herdr_mouse_selection_policy() {
    let cells = vec![cell("123")];
    let mut view = TranscriptView::default().with_herdr_mouse_selection(true);
    render(&mut view, &cells, /*width*/ 8, /*height*/ 1);
    view.begin_selection(&cells, /*column*/ 0, /*row*/ 0, /*clicks*/ 1);
    view.extend_selection(/*column*/ 2, /*row*/ 0);
    assert_eq!(view.selected_text(&cells).as_deref(), Some("123"));
    view.reset();
    assert!(view.herdr_mouse_selection);
    assert!(view.selected_text(&cells).is_none());
    render(&mut view, &cells, /*width*/ 8, /*height*/ 1);
    view.begin_selection(&cells, /*column*/ 0, /*row*/ 0, /*clicks*/ 1);
    view.extend_selection(/*column*/ 2, /*row*/ 0);
    assert_eq!(view.selected_text(&cells).as_deref(), Some("123"));
}

#[test]
fn herdr_auto_copy_clears_highlight_after_terminal_send() {
    let cells = vec![cell("123")];
    for clear_selection in [false, true] {
        for asynchronous in [false, true] {
            for result in [
                Ok(CopyStatus::Confirmed),
                Ok(CopyStatus::Unconfirmed),
                Err("terminal write failed".to_string()),
            ] {
                let mut view = TranscriptView {
                    copy_on_select: true,
                    copy_on_select_clear_selection: clear_selection,
                    ..Default::default()
                }
                .with_herdr_mouse_selection(true);
                render(&mut view, &cells, /*width*/ 8, /*height*/ 1);
                view.handle_mouse(mouse(MouseEventKind::Down(MouseButton::Left), 0), &cells);
                view.handle_mouse(mouse(MouseEventKind::Drag(MouseButton::Left), 2), &cells);
                let Some(action @ ViewAction::CopyOnSelect(_)) =
                    view.handle_mouse(mouse(MouseEventKind::Up(MouseButton::Left), 2), &cells)
                else {
                    panic!("mouse release must request automatic copying");
                };
                let clear = view.selection_copy(&action).clear_selection();
                let actual = view.copy_selected_text_with(&cells, "123", clear, |text, _| {
                    assert_eq!(text, "123");
                    if asynchronous {
                        Ok(CopyStatus::Pending(7))
                    } else {
                        result.clone()
                    }
                });
                if asynchronous {
                    assert_eq!(actual, Ok(CopyStatus::Pending(7)));
                    assert!(view.has_selection_range());
                    assert_eq!(
                        view.finish_copy(&cells, &(7, result.clone()), true),
                        Some(false)
                    );
                } else {
                    assert_eq!(actual, result);
                }
                let cleared = clear_selection
                    && matches!(result, Ok(CopyStatus::Confirmed | CopyStatus::Unconfirmed));
                assert_eq!(
                    view.selected_text(&cells).as_deref(),
                    (!cleared).then_some("123"),
                    "clear={clear_selection}, async={asynchronous}, result={result:?}"
                );
                let buffer = render(&mut view, &cells, /*width*/ 8, /*height*/ 1);
                let highlighted =
                    (0..3).any(|column| buffer[(column, 0)].modifier.contains(Modifier::REVERSED));
                assert_eq!(highlighted, !cleared);
                assert_eq!(
                    view.copy_feedback.as_ref().map(|feedback| feedback.result),
                    Some(result.map_err(|_| ()))
                );
            }
        }
    }
}

#[test]
fn copy_on_select_waits_for_release_and_confirmed_delivery() {
    let cells = vec![cell("selected text")];
    for (enabled, clear_selection) in [(false, false), (true, false), (true, true)] {
        let mut view = TranscriptView {
            copy_on_select: enabled,
            copy_on_select_clear_selection: clear_selection,
            ..Default::default()
        };
        render(&mut view, &cells, /*width*/ 20, /*height*/ 1);
        for event in [
            mouse(MouseEventKind::Down(MouseButton::Left), /*column*/ 0),
            mouse(MouseEventKind::Drag(MouseButton::Left), /*column*/ 5),
        ] {
            assert!(matches!(
                view.handle_mouse(event, &cells),
                Some(ViewAction::Changed)
            ));
        }
        let release = mouse(MouseEventKind::Up(MouseButton::Left), /*column*/ 8);
        let action = view.handle_mouse(release, &cells);
        let selection_copy = action.as_ref().map(|action| view.selection_copy(action));
        let copied = match action {
            Some(ViewAction::CopyOnSelect(text)) => Some(text),
            Some(ViewAction::Changed) => None,
            _ => panic!("release must finish the selection"),
        };
        assert_eq!(copied.as_deref(), enabled.then_some("selected"));
        assert_eq!(view.selected_text(&cells).as_deref(), Some("selected"));
        assert!(view.handle_mouse(release, &cells).is_none());
        assert!(!view.tick_selection(&cells));
        if let Some(copied) = copied {
            let selection_copy = selection_copy.expect("copy on select policy");
            assert_eq!(selection_copy, SelectionCopy::OnSelect { clear_selection });
            assert!(selection_copy.publishes_primary());
            let buffer = render(&mut view, &cells, /*width*/ 20, /*height*/ 1);
            let highlight = (0..20)
                .map(|column| {
                    if buffer[(column, 0)]
                        .modifier
                        .contains(ratatui::style::Modifier::REVERSED)
                    {
                        '^'
                    } else {
                        '·'
                    }
                })
                .collect::<String>();
            insta::allow_duplicates! {
                insta::assert_snapshot!(
                    format!("{}\n{highlight}", text(&buffer)),
                    @"
                    selected text
                    ^^^^^^^^············
                    "
                );
            }
            for result in [
                Ok(CopyStatus::Unconfirmed),
                Err("clipboard unavailable".to_owned()),
                Ok(CopyStatus::Confirmed),
            ] {
                view.copy_selected_text_with(
                    &cells,
                    &copied,
                    selection_copy.clear_selection(),
                    |_, _format| Ok(CopyStatus::Pending(1)),
                )
                .unwrap();
                assert_eq!(
                    view.finish_copy(&cells, &(1, result.clone()), /*current*/ true),
                    Some(false)
                );
                assert_eq!(
                    view.selected_text(&cells).as_deref(),
                    (!clear_selection || result != Ok(CopyStatus::Confirmed))
                        .then_some(copied.as_str())
                );
                assert_eq!(
                    view.copy_feedback
                        .as_ref()
                        .map(|feedback| (feedback.result, feedback.characters)),
                    Some((result.map_err(|_| ()), copied.chars().count()))
                );
            }
            if clear_selection {
                let buffer = render(&mut view, &cells, /*width*/ 20, /*height*/ 1);
                let highlight = (0..20)
                    .map(|column| {
                        if buffer[(column, 0)]
                            .modifier
                            .contains(ratatui::style::Modifier::REVERSED)
                        {
                            '^'
                        } else {
                            '·'
                        }
                    })
                    .collect::<String>();
                insta::assert_snapshot!(
                    format!("{}\n{highlight}", text(&buffer)),
                    @"
                    selected text
                    ····················
                    "
                );
            }
        }
    }
}

#[test]
fn copy_on_select_copies_word_and_line_on_release_without_dragging() {
    let cells = vec![cell("alpha beta gamma")];
    let mut view = TranscriptView {
        copy_on_select: true,
        ..Default::default()
    };
    render(&mut view, &cells, /*width*/ 24, /*height*/ 1);
    let down = mouse(MouseEventKind::Down(MouseButton::Left), /*column*/ 7);
    let up = MouseEvent {
        kind: MouseEventKind::Up(MouseButton::Left),
        ..down
    };
    for expected in [None, Some("beta"), Some("alpha beta gamma")] {
        if let Some((at, ..)) = &mut view.last_click {
            *at = Instant::now();
        }
        assert!(matches!(
            view.handle_mouse(down, &cells),
            Some(ViewAction::Changed)
        ));
        let copied = match view.handle_mouse(up, &cells) {
            Some(ViewAction::CopyOnSelect(text)) => Some(text),
            Some(ViewAction::Changed) => None,
            _ => panic!("release must finish the repeated click"),
        };
        assert_eq!(
            (copied.as_deref(), view.selected_text(&cells).as_deref()),
            (expected, expected)
        );
        assert!(view.handle_mouse(up, &cells).is_none());
    }
}

#[test]
fn copy_on_select_preserves_link_activation_and_ignores_empty_drags() {
    let cells: Vec<Arc<dyn HistoryCell>> = vec![Arc::new(AgentMarkdownCell::new(
        "[example.com](https://example.com/docs)".into(),
        std::path::Path::new("/"),
    ))];
    for (drag, release) in [(None, 2), (Some(5), 5), (Some(5), 2)] {
        let mut view = TranscriptView {
            copy_on_select: true,
            ..Default::default()
        };
        render(&mut view, &cells, /*width*/ 30, /*height*/ 1);
        let down = mouse(MouseEventKind::Down(MouseButton::Left), /*column*/ 2);
        assert!(matches!(
            view.handle_mouse(down, &cells),
            Some(ViewAction::Changed)
        ));
        if let Some(column) = drag {
            assert!(matches!(
                view.handle_mouse(
                    mouse(MouseEventKind::Drag(MouseButton::Left), column),
                    &cells
                ),
                Some(ViewAction::Changed)
            ));
        }
        let action = view.handle_mouse(
            mouse(MouseEventKind::Up(MouseButton::Left), release),
            &cells,
        );
        match (drag, release, action) {
            (None, _, Some(ViewAction::OpenLink(url))) => {
                assert_eq!(url, "https://example.com/docs");
                assert_eq!(view.selected_text(&cells), None);
            }
            (Some(_), 5, Some(ViewAction::CopyOnSelect(copied))) => {
                assert_eq!(copied, "exa");
                assert_eq!(view.selected_text(&cells).as_deref(), Some("exa"));
            }
            (Some(_), 2, Some(ViewAction::Changed)) => {
                assert_eq!(view.selected_text(&cells), None);
            }
            _ => panic!("only a nonempty link drag should copy"),
        }
    }
}

#[test]
fn x11_release_preserves_primary_when_auto_copy_is_disabled() {
    let cells = vec![cell("visit the café")];
    for auto_copy in [false, true] {
        let mut view = TranscriptView {
            copy_on_select: auto_copy,
            primary_selection: true,
            ..Default::default()
        };
        render(&mut view, &cells, /*width*/ 20, /*height*/ 1);
        for event in [
            mouse(MouseEventKind::Down(MouseButton::Left), /*column*/ 0),
            mouse(MouseEventKind::Drag(MouseButton::Left), /*column*/ 14),
        ] {
            assert!(matches!(
                view.handle_mouse(event, &cells),
                Some(ViewAction::Changed)
            ));
        }
        let release = mouse(MouseEventKind::Up(MouseButton::Left), /*column*/ 14);
        let text = match (auto_copy, view.handle_mouse(release, &cells)) {
            (false, Some(ViewAction::PrimarySelection(text)))
            | (true, Some(ViewAction::CopyOnSelect(text))) => text,
            _ => panic!("mouse release must publish to PRIMARY regardless of auto-copy"),
        };
        assert_eq!(
            (text.as_str(), view.selected_text(&cells).as_deref()),
            ("visit the café", Some("visit the café"))
        );
        assert!(view.handle_mouse(release, &cells).is_none());
    }
}
