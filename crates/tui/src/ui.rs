//! Drawing. Everything here reads the app's state and records where clickable things went
//! (`App::hits`); behaviour lives in `app.rs`.

use familiar_engine::Status;
use familiar_engine::stats::StatPane;
use ratatui::Frame;
use ratatui::layout::{Constraint, Layout, Margin, Rect};
use ratatui::style::{Color, Modifier, Style};
use ratatui::symbols::Marker;
use ratatui::text::{Line, Span};
use ratatui::widgets::canvas::{Canvas, Line as CanvasLine, Rectangle};
use ratatui::widgets::{Block, BorderType, Borders, Clear, Paragraph};
use unicode_width::UnicodeWidthStr;

use crate::app::{App, DOCK_MIN_WIDTH, Find, Hits, Overlay, Picker, PickerKind, RowHit, TUI_COMMANDS, Tab, dock_width};
use crate::keyboard::KeysView;
use crate::form::{FieldKind, WorldForm};
use crate::rules::{self, RuleForm, RulesView};
use familiar_config::Config;
use crate::buffer::{self, Buffer, Selection};
use crate::session::{Panel, Session};
use crate::theme::{self, ACCENT, ACCENT_2, BAD, FAINT, GOOD, MUTED, SELECTED_BG, WARN};

const SIDEBAR_WIDTH: u16 = 38;
const LOGO: [&str; 6] = [
    "███████╗ █████╗ ███╗   ███╗██╗██╗     ██╗ █████╗ ██████╗ ",
    "██╔════╝██╔══██╗████╗ ████║██║██║     ██║██╔══██╗██╔══██╗",
    "█████╗  ███████║██╔████╔██║██║██║     ██║███████║██████╔╝",
    "██╔══╝  ██╔══██║██║╚██╔╝██║██║██║     ██║██╔══██║██╔══██╗",
    "██║     ██║  ██║██║ ╚═╝ ██║██║███████╗██║██║  ██║██║  ██║",
    "╚═╝     ╚═╝  ╚═╝╚═╝     ╚═╝╚═╝╚══════╝╚═╝╚═╝  ╚═╝╚═╝  ╚═╝",
];

pub fn draw(f: &mut Frame, app: &mut App) {
    app.hits = Hits::default();
    let area = f.area();
    if app.tabs.is_empty() {
        draw_welcome(f, app, area);
        draw_overlay(f, app, area);
        return;
    }
    app.mark_seen();
    app.screen = (area.width, area.height);
    if let Some(Tab::Editor(id)) = app.tabs.get(app.active).cloned() {
        let [tabs, body, status] = Layout::vertical([Constraint::Length(1), Constraint::Min(1), Constraint::Length(1)]).areas(area);
        draw_tabs(f, app, tabs);
        draw_editor(f, app, id, body, true);
        draw_editor_status(f, app, id, status);
        draw_overlay(f, app, area);
        return;
    }
    let input_height = input_height(app, area.width);
    let [tabs, body, input, status] =
        Layout::vertical([Constraint::Length(1), Constraint::Min(3), Constraint::Length(input_height), Constraint::Length(1)]).areas(area);
    draw_tabs(f, app, tabs);

    // An editor beside this world has the sidebar's place, for as long as there's room for it.
    let mut docked = app.visible_editor().map(|e| (e.id, e.focused));
    if let Some((id, _)) = docked
        && body.width < DOCK_MIN_WIDTH
    {
        app.undock(id, false);
        app.toast("The editor moved to a tab of its own: the terminal is too narrow to show it beside the world");
        docked = None;
    }
    let (main, side) = if docked.is_some() {
        let [m, s] = Layout::horizontal([Constraint::Min(30), Constraint::Length(dock_width(body.width))]).areas(body);
        (m, Some(s))
    } else if app.sidebar && body.width >= DOCK_MIN_WIDTH && app.current().is_some_and(|(i, p)| p.is_none() && !app.sessions[i].panels().is_empty()) {
        let [m, s] = Layout::horizontal([Constraint::Min(30), Constraint::Length(SIDEBAR_WIDTH)]).areas(body);
        (m, Some(s))
    } else {
        (body, None)
    };
    draw_output(f, app, main);
    if let (Some(side), None) = (side, docked) {
        draw_sidebar(f, app, side);
    }
    draw_toasts(f, app, main);
    draw_input(f, app, input);
    draw_suggestions(f, app, input, main);
    // After the input, so that the cursor is the editor's while it has the keyboard.
    if let (Some(side), Some((id, focused))) = (side, docked) {
        draw_docked_editor(f, app, id, focused, side);
    }
    draw_status(f, app, status);
    draw_overlay(f, app, area);
}

fn spinner(app: &App) -> &'static str {
    theme::SPINNER[(app.started.elapsed().as_millis() / 120) as usize % theme::SPINNER.len()]
}

// ----- Welcome -----

fn draw_welcome(f: &mut Frame, app: &mut App, area: Rect) {
    let logo_height = if area.height >= 30 && area.width >= 64 { LOGO.len() as u16 + 3 } else { 2 };
    let [top, rest, status] = Layout::vertical([Constraint::Length(logo_height), Constraint::Min(5), Constraint::Length(1)]).areas(area);
    let mut lines = Vec::new();
    if logo_height > 2 {
        lines.push(Line::raw(""));
        for (i, row) in LOGO.iter().enumerate() {
            // A violet-to-cyan gradient, row by row.
            let t = i as f32 / (LOGO.len() - 1) as f32;
            let mix = |a: u8, b: u8| (a as f32 + (b as f32 - a as f32) * t) as u8;
            let color = Color::Rgb(mix(0xc0, 0x7d), mix(0x9a, 0xcf), mix(0xf0, 0xff));
            lines.push(Line::styled(*row, Style::new().fg(color)).centered());
        }
    }
    lines.push(
        Line::from(vec![Span::styled("✻ ", theme::accent()), Span::styled(familiar_config::APP_NAME, theme::bold()), Span::styled("  for MUDs, MUSHes, MUCKs and MOOs", theme::muted())]).centered(),
    );
    f.render_widget(Paragraph::new(lines), top);
    let width = rest.width.min(76);
    let picker_area = Rect { x: rest.x + (rest.width - width) / 2, width, ..rest };
    if let Some(Overlay::Picker(p)) = &app.overlay
        && p.kind == PickerKind::Connect
    {
        let mut hits = Vec::new();
        draw_picker(f, p, picker_area, false, &mut hits);
        app.hits.picker_items = hits;
    }
    let hint = if app.quit_armed.is_some() {
        Line::styled("Press Ctrl+C again to quit", Style::new().fg(WARN))
    } else {
        Line::from(vec![
            key_hint("type"),
            Span::styled(" to filter · ", theme::muted()),
            key_hint("host:port"),
            Span::styled(" for a new world · ", theme::muted()),
            key_hint("Ctrl+C"),
            Span::styled(" twice to quit", theme::muted()),
        ])
    };
    f.render_widget(Paragraph::new(hint.centered()), status);
}

fn key_hint(text: &str) -> Span<'_> {
    Span::styled(text, Style::new().fg(ACCENT_2))
}

fn key_hint_owned(text: String) -> Span<'static> {
    Span::styled(text, Style::new().fg(ACCENT_2))
}

/// All of an action's keys, for the help screen: "Ctrl+K · Ctrl+P", or "—" if it has none.
fn keys_of(app: &App, id: &str) -> String {
    let keys: Vec<String> = app.keymap.keys(id).iter().map(crate::keys::pretty).collect();
    if keys.is_empty() { "—".into() } else { keys.join(" · ") }
}

/// The first keys of several actions, for one line of the help screen: "Alt+← Alt+→".
fn first_keys(app: &App, ids: &[&str]) -> String {
    let keys: Vec<String> = ids.iter().filter_map(|id| app.key_for(id)).collect();
    if keys.is_empty() { "—".into() } else { keys.join(" ") }
}

// ----- Tabs -----

fn draw_tabs(f: &mut Frame, app: &mut App, area: Rect) {
    let pill = Span::styled(" ✦ Familiar ", Style::new().fg(Color::Black).bg(ACCENT).add_modifier(Modifier::BOLD));
    let mut x = area.x + pill.width() as u16 + 1;
    let mut spans = vec![pill, Span::raw(" ")];
    for (i, tab) in app.tabs.iter().enumerate() {
        let active = i == app.active;
        let (unread, important, status) = match tab {
            Tab::Main(id) => app.session_index(*id).map_or((0, false, None), |s| {
                let s = &app.sessions[s];
                (s.unread, s.important, Some(s.status.clone()))
            }),
            Tab::Puppet(id, name) => app.session_index(*id).map_or((0, false, None), |s| {
                let unread = app.sessions[s].puppets.iter().find(|p| &p.name == name).map_or(0, |p| p.unread);
                (unread, false, None)
            }),
            Tab::Editor(_) => (0, false, None),
        };
        let dot = match status {
            Some(Status::Connected) => Span::styled("● ", Style::new().fg(GOOD)),
            Some(Status::Connecting) => Span::styled(format!("{} ", spinner(app)), Style::new().fg(WARN)),
            Some(Status::Disconnected(_)) => Span::styled("○ ", Style::new().fg(BAD)),
            None => Span::raw(""),
        };
        let title = app.tab_title(tab);
        let style = if active { Style::new().fg(Color::White).bg(SELECTED_BG).add_modifier(Modifier::BOLD) } else { theme::muted() };
        let mut tab_spans = vec![Span::styled(" ", style), dot.patch_style(if active { Style::new().bg(SELECTED_BG) } else { Style::new() }), Span::styled(title, style)];
        if important {
            tab_spans.push(Span::styled(" ★", Style::new().fg(WARN).bg(if active { SELECTED_BG } else { Color::Reset })));
        } else if unread > 0 {
            tab_spans.push(Span::styled(format!(" {}", unread.min(999)), Style::new().fg(ACCENT_2).bg(if active { SELECTED_BG } else { Color::Reset })));
        }
        tab_spans.push(Span::styled(" ", style));
        let width: u16 = tab_spans.iter().map(|s| s.width() as u16).sum();
        app.hits.tabs.push((Rect { x, y: area.y, width, height: 1 }, i));
        x += width + 1;
        spans.extend(tab_spans);
        spans.push(Span::raw(" "));
    }
    f.render_widget(Paragraph::new(Line::from(spans)), area);
}

// ----- Editor tabs -----

fn term_color(color: vt100::Color) -> Color {
    match color {
        vt100::Color::Default => Color::Reset,
        vt100::Color::Idx(i) => Color::Indexed(i),
        vt100::Color::Rgb(r, g, b) => Color::Rgb(r, g, b),
    }
}

/// The screen of the editor running in a tab.
/// An editor beside a world: a rule down its left, its name along the top, its screen below.
fn draw_docked_editor(f: &mut Frame, app: &mut App, id: u64, focused: bool, area: Rect) {
    let [rule, rest] = Layout::horizontal([Constraint::Length(1), Constraint::Min(1)]).areas(area);
    let [heading, screen] = Layout::vertical([Constraint::Length(1), Constraint::Min(1)]).areas(rest);
    let border = if focused { theme::accent() } else { theme::faint() };
    let lines: Vec<Line> = (0..rule.height).map(|_| Line::styled("│", border)).collect();
    f.render_widget(Paragraph::new(lines), rule);
    let (title, label) = pane_label(app, id);
    let name = if focused { Style::new().fg(ACCENT_2).add_modifier(Modifier::BOLD) } else { theme::muted() };
    let other = if focused { " world  " } else if label.1 { " agent  " } else { " editor  " };
    let hint = Line::from(vec![key_hint("F6"), Span::styled(other, theme::faint()), key_hint("F7"), Span::styled(" to a tab ", theme::faint())]);
    let hint_width = hint.width() as u16;
    let [l, r] = Layout::horizontal([Constraint::Min(1), Constraint::Length(hint_width)]).areas(heading);
    f.render_widget(Paragraph::new(Line::from(vec![Span::styled(format!(" {title}"), name), Span::styled(label.0, theme::faint())])), l);
    f.render_widget(Paragraph::new(hint.right_aligned()), r);
    draw_editor(f, app, id, screen, focused);
}

/// An editor's or agent's title with its mark, the hint after it, and whether it's an agent.
fn pane_label(app: &App, id: u64) -> (String, (&'static str, bool)) {
    match app.editors.iter().find(|e| e.id == id) {
        Some(e) if e.is_agent() => (format!("✦ {}", e.title), ("  /agent stop ends it", true)),
        Some(e) => (format!("✎ {}", e.title), ("  save and quit to finish", false)),
        None => (String::new(), ("", false)),
    }
}

/// Draws an editor's screen. `cursor` is whether the keyboard is the editor's.
fn draw_editor(f: &mut Frame, app: &mut App, id: u64, area: Rect, cursor: bool) {
    app.hits.editor = area;
    let Some(editor) = app.editors.iter_mut().find(|e| e.id == id) else { return };
    editor.term.resize(area.width, area.height);
    let screen = editor.term.screen();
    let buffer = f.buffer_mut();
    for row in 0..area.height {
        for col in 0..area.width {
            let Some(cell) = screen.cell(row, col) else { continue };
            // The second half of a wide character belongs to the cell before.
            if cell.is_wide_continuation() {
                continue;
            }
            let mut style = Style::new().fg(term_color(cell.fgcolor())).bg(term_color(cell.bgcolor()));
            for (on, modifier) in [(cell.bold(), Modifier::BOLD), (cell.dim(), Modifier::DIM), (cell.italic(), Modifier::ITALIC), (cell.underline(), Modifier::UNDERLINED), (cell.inverse(), Modifier::REVERSED)] {
                if on {
                    style = style.add_modifier(modifier);
                }
            }
            let target = &mut buffer[(area.x + col, area.y + row)];
            target.set_symbol(if cell.has_contents() { cell.contents() } else { " " });
            target.set_style(style);
        }
    }
    if cursor && !screen.hide_cursor() {
        let (row, col) = screen.cursor_position();
        f.set_cursor_position((area.x + col.min(area.width.saturating_sub(1)), area.y + row.min(area.height.saturating_sub(1))));
    }
}

fn draw_editor_status(f: &mut Frame, app: &App, id: u64, area: Rect) {
    let (title, label) = pane_label(app, id);
    let left = Line::from(vec![Span::styled(format!(" {title}"), Style::new().fg(ACCENT_2)), Span::styled(label.0, theme::faint())]);
    let tabs = [app.key_for("prev-tab"), app.key_for("next-tab")].into_iter().flatten().collect::<Vec<_>>().join(" ");
    let hints = [(app.key_for("move-editor"), " to the sidebar  "), ((!tabs.is_empty()).then_some(tabs), " other tabs ")];
    let right = Line::from(hints.into_iter().filter_map(|(key, what)| Some([key_hint_owned(key?), Span::styled(what, theme::faint())])).flatten().collect::<Vec<_>>());
    let right_width = right.width() as u16;
    let [l, r] = Layout::horizontal([Constraint::Min(1), Constraint::Length(right_width)]).areas(area);
    f.render_widget(Paragraph::new(left), l);
    f.render_widget(Paragraph::new(right.right_aligned()), r);
}

// ----- Output -----

/// What's marked in a buffer as it's drawn: the mouse selection, and what the search bar is
/// looking for with the match it's showing (entry, start).
#[derive(Clone, Copy)]
struct Marks<'a> {
    selection: Option<&'a Selection>,
    find: Option<(&'a str, Option<(u64, usize)>)>,
}

/// Draws the rows of `buffer` that are `scroll` rows up from its end. `live` marks them as the
/// tail under a scrolled-back view.
fn draw_buffer(f: &mut Frame, buffer: &mut Buffer, area: Rect, scroll: usize, live: bool, marks: Marks, hits: &mut Hits) {
    let Marks { selection, find } = marks;
    let id = buffer.id;
    let selection = selection.filter(|s| s.buffer == id);
    let rows = buffer.visible(area.width, area.height, scroll);
    let top = area.y + area.height - rows.len() as u16;
    let mut lines = Vec::with_capacity(rows.len());
    for (i, (entry, row)) in rows.iter().enumerate() {
        let found: Vec<(std::ops::Range<usize>, bool)> = match find {
            Some((query, current)) => buffer::find_in(entry.plain(), query).into_iter().map(|r| (r.clone(), current == Some((entry.seq, r.start)))).collect(),
            None => Vec::new(),
        };
        let (line, row_links) = entry.row(area.width, *row, selection, &found);
        for (x0, x1, uri) in row_links {
            hits.links.push((top + i as u16, area.x + x0, area.x + x1, uri));
        }
        hits.rows.push(RowHit { y: top + i as u16, x: area.x, width: area.width, buffer: id, seq: entry.seq, row: *row, live });
        lines.push(line);
    }
    f.render_widget(Paragraph::new(lines), Rect { y: top, height: rows.len() as u16, ..area });
}

fn draw_output(f: &mut Frame, app: &mut App, area: Rect) {
    let area = area.inner(Margin { horizontal: 1, vertical: 0 });
    app.hits.output = area;
    app.set_view(area.width, area.height);
    let Some((i, puppet)) = app.current() else { return };
    let selection = app.selection;
    let find = match &app.overlay {
        Some(Overlay::Find(find)) => Some((find.buffer, find.query.text.clone(), find.current.as_ref().map(|(seq, r)| (*seq, r.start)))),
        _ => None,
    };
    let s = &mut app.sessions[i];
    let buffer = match &puppet {
        Some(name) => &mut s.puppet_mut(name).buffer,
        None => &mut s.output,
    };
    let find = find.as_ref().filter(|(id, _, _)| *id == buffer.id).map(|(_, query, current)| (query.as_str(), *current));
    if buffer.len() == 0 {
        let hint = Line::from(vec![Span::styled("Waiting for the server… ", theme::faint()), Span::styled("/help", Style::new().fg(ACCENT_2)), Span::styled(" lists commands", theme::faint())]);
        f.render_widget(Paragraph::new(hint), Rect { y: area.y + area.height.saturating_sub(1), height: 1, ..area });
        return;
    }
    let scroll = buffer.scroll;
    if scroll > 0
        && let Some(tail) = buffer::live_tail(area.height)
    {
        // Split view: history on top, the live tail underneath.
        let [history, rule, live] = Layout::vertical([Constraint::Min(1), Constraint::Length(1), Constraint::Length(tail)]).areas(area);
        let max = buffer.total_rows(history.width).saturating_sub(history.height as usize);
        buffer.scroll = scroll.min(max);
        app.hits.scrolling = Some((buffer.id, history));
        draw_buffer(f, buffer, history, buffer.scroll + tail as usize + 1, false, Marks { selection: selection.as_ref(), find }, &mut app.hits);
        let unseen = buffer.unseen;
        draw_buffer(f, buffer, live, 0, true, Marks { selection: selection.as_ref(), find }, &mut app.hits);
        let label = if unseen > 0 { format!(" ▲ scrolled back · {unseen} new · Esc to jump down ") } else { " ▲ scrolled back · Esc to jump down ".to_string() };
        let pad = (rule.width as usize).saturating_sub(label.width() + 2);
        let line = Line::from(vec![Span::styled("──", theme::faint()), Span::styled(label, Style::new().fg(ACCENT_2)), Span::styled("─".repeat(pad), theme::faint())]);
        f.render_widget(Paragraph::new(line), rule);
    } else {
        app.hits.scrolling = Some((buffer.id, area));
        draw_buffer(f, buffer, area, scroll, false, Marks { selection: selection.as_ref(), find }, &mut app.hits);
        if scroll > 0 {
            let pill = format!(" ↓ {} new · Esc ", buffer.unseen);
            let w = pill.width() as u16;
            let r = Rect { x: area.x + area.width.saturating_sub(w + 1), y: area.y + area.height - 1, width: w.min(area.width), height: 1 };
            f.render_widget(Clear, r);
            f.render_widget(Paragraph::new(Span::styled(pill, Style::new().fg(Color::Black).bg(ACCENT_2))), r);
        }
    }
}

fn draw_toasts(f: &mut Frame, app: &App, area: Rect) {
    for (n, (text, _)) in app.toasts.iter().rev().take(3).enumerate() {
        let text = format!(" ✦ {text} ");
        let w = (text.width() as u16).min(area.width.saturating_sub(4));
        let r = Rect { x: area.x + area.width.saturating_sub(w + 2), y: area.y + n as u16, width: w, height: 1 };
        f.render_widget(Clear, r);
        f.render_widget(Paragraph::new(Span::styled(text, Style::new().fg(Color::White).bg(SELECTED_BG))), r);
    }
}

// ----- Sidebar -----

fn draw_sidebar(f: &mut Frame, app: &mut App, area: Rect) {
    let Some((i, _)) = app.current() else { return };
    app.hits.sidebar = area;
    let block = Block::new().borders(Borders::LEFT).border_style(theme::faint()).border_type(BorderType::Rounded);
    let inner = block.inner(area);
    f.render_widget(block, area);
    let inner = inner.inner(Margin { horizontal: 1, vertical: 0 });
    let s = &mut app.sessions[i];
    let panels = s.panels();
    s.panel = s.panel.min(panels.len().saturating_sub(1));
    let [tabs, body] = Layout::vertical([Constraint::Length(2), Constraint::Min(1)]).areas(inner);

    let mut spans = Vec::new();
    let mut x = tabs.x;
    for (n, panel) in panels.iter().enumerate() {
        let unread = matches!(panel, Panel::Spawn(name) if s.spawns.iter().any(|p| &p.name == name && p.unread));
        let label = format!("{}{}", panel.title(), if unread { " •" } else { "" });
        let style = if n == s.panel { Style::new().fg(ACCENT).add_modifier(Modifier::BOLD | Modifier::UNDERLINED) } else { theme::muted() };
        let w = label.width() as u16;
        if x + w > tabs.x + tabs.width {
            break;
        }
        app.hits.panels.push((Rect { x, y: tabs.y, width: w, height: 1 }, n));
        spans.push(Span::styled(label, style));
        spans.push(Span::styled("  ", theme::faint()));
        x += w + 2;
    }
    f.render_widget(Paragraph::new(Line::from(spans)), tabs);

    match panels.get(s.panel) {
        Some(Panel::Map) => draw_map(f, s, body, &mut app.hits),
        Some(Panel::Stats(name)) => {
            if let Some(pane) = s.stats.get(name) {
                draw_stats(f, pane, body);
            }
        }
        Some(Panel::Spawn(name)) => {
            let pane = s.spawn_mut(name).0;
            pane.unread = false;
            let scroll = pane.buffer.scroll;
            draw_buffer(f, &mut pane.buffer, body, scroll, false, Marks { selection: app.selection.as_ref(), find: None }, &mut app.hits);
        }
        None => {}
    }
}

fn draw_stats(f: &mut Frame, pane: &StatPane, area: Rect) {
    let name_width = pane.values.iter().map(|(k, s)| s.display_name(k).width()).max().unwrap_or(0).min(12);
    let mut lines = Vec::new();
    for (key, stat) in &pane.values {
        let name = stat.display_name(key);
        let name_style = Style::new().fg(theme::stat_color(stat.name_color).unwrap_or(MUTED));
        let value_style = Style::new().fg(theme::stat_color(stat.value_color).unwrap_or(Color::White)).add_modifier(Modifier::BOLD);
        let label = Span::styled(format!("{:<name_width$} ", truncate(name, name_width)), name_style);
        let text = stat.value.text();
        match stat.value.fraction() {
            Some(fraction) => {
                let colors = match &stat.value {
                    familiar_engine::stats::StatValue::Range { colors, .. } | familiar_engine::stats::StatValue::Progress { colors, .. } => colors.clone(),
                    _ => Default::default(),
                };
                let fill = theme::stat_color(colors.fill).unwrap_or_else(|| gradient(fraction));
                let empty = theme::stat_color(colors.empty).unwrap_or(FAINT);
                let value = format!(" {text}");
                let bar = (area.width as usize).saturating_sub(name_width + 1 + value.width()).max(4);
                lines.push(Line::from([vec![label], bar_spans(fraction, bar, fill, empty), vec![Span::styled(value, value_style)]].concat()));
            }
            None => lines.push(Line::from(vec![label, Span::styled(text, value_style)])),
        }
    }
    f.render_widget(Paragraph::new(lines), area);
}

/// Red to yellow to green as a bar fills.
fn gradient(fraction: f64) -> Color {
    if fraction < 0.25 {
        BAD
    } else if fraction < 0.5 {
        WARN
    } else {
        GOOD
    }
}

/// A smooth bar with eighth-block resolution.
fn bar_spans(fraction: f64, width: usize, fill: Color, empty: Color) -> Vec<Span<'static>> {
    const PARTS: [&str; 8] = ["", "▏", "▎", "▍", "▌", "▋", "▊", "▉"];
    let eighths = (fraction.clamp(0.0, 1.0) * width as f64 * 8.0).round() as usize;
    let full = eighths / 8;
    let part = PARTS[eighths % 8];
    let rest = width - full - usize::from(!part.is_empty());
    vec![
        Span::styled("█".repeat(full), Style::new().fg(fill)),
        Span::styled(part, Style::new().fg(fill).bg(empty)),
        Span::styled("█".repeat(rest), Style::new().fg(empty)),
    ]
}

fn draw_map(f: &mut Frame, s: &Session, area: Rect, hits: &mut Hits) {
    let map = s.engine.map();
    let current = s.engine.current_room().and_then(|id| map.room(id));
    let Some(center_room) = current.or_else(|| map.rooms().next()) else { return };
    let area_name = center_room.area.clone();
    let [canvas_area, info] = Layout::vertical([Constraint::Min(4), Constraint::Length(3)]).areas(area);
    // Map units per terminal cell: rooms come out about 10 cells wide and 2 tall.
    let (sx, sy) = (10.0, 16.0);
    let (cx, cy) = center_room.center();
    let (hw, hh) = (canvas_area.width as f64 * sx / 2.0, canvas_area.height as f64 * sy / 2.0);
    let (x0, x1, y0, y1) = (cx - hw, cx + hw, cy - hh, cy + hh);
    hits.map = Some((canvas_area, (x0, y0, x1, y1), area_name.clone()));
    let rooms: Vec<_> = map.rooms().filter(|r| r.area == area_name && r.x + r.w >= x0 && r.x <= x1 && r.y + r.h >= y0 && r.y <= y1).collect();
    let current_id = current.map(|r| r.id);
    let canvas = Canvas::default().x_bounds([x0, x1]).y_bounds([-y1, -y0]).marker(Marker::Braille).paint(|ctx| {
        for r in &rooms {
            let (ax, ay) = r.center();
            for e in &r.exits {
                if let Some(to) = e.to.and_then(|id| map.room(id))
                    && to.area == area_name
                {
                    let (bx, by) = to.center();
                    ctx.draw(&CanvasLine { x1: ax, y1: -ay, x2: bx, y2: -by, color: FAINT });
                }
            }
        }
        ctx.layer();
        for r in &rooms {
            let here = Some(r.id) == current_id;
            let color = if here { ACCENT } else { r.color.as_deref().and_then(theme::hex).unwrap_or(MUTED) };
            ctx.draw(&Rectangle { x: r.x, y: -(r.y + r.h), width: r.w, height: r.h, color });
        }
        ctx.layer();
        for r in &rooms {
            let here = Some(r.id) == current_id;
            // Labels may spill past the box, up to most of the space between grid rooms.
            let cells = ((r.w.max(familiar_map::GRID_X - 24.0) / sx) as usize).max(3);
            let label = truncate(&r.name, cells);
            let style = if here { Style::new().fg(ACCENT).add_modifier(Modifier::BOLD) } else { Style::new().fg(Color::Gray) };
            let (center_x, _) = r.center();
            // Keep labels inside the canvas: text that starts outside it isn't drawn at all.
            let label_w = label.width() as f64 * sx;
            let lx = (center_x - label_w / 2.0).clamp(x0, (x1 - label_w).max(x0));
            ctx.print(lx, -(r.y + r.h / 2.0), Line::styled(label, style));
        }
    });
    f.render_widget(canvas, canvas_area);

    let mut lines = vec![Line::from(vec![Span::styled(area_name.clone(), theme::faint()), Span::styled(format!(" · {} rooms", map.len()), theme::faint())])];
    match current {
        Some(r) => {
            lines.push(Line::from(vec![Span::styled("◆ ", theme::accent()), Span::styled(r.name.clone(), theme::bold())]));
            let exits: Vec<&str> = r.exits.iter().map(|e| e.command.as_str()).collect();
            lines.push(Line::styled(if exits.is_empty() { "no known exits".to_string() } else { format!("exits: {}", exits.join(" ")) }, theme::muted()));
        }
        None => lines.push(Line::styled("Location unknown · click a room to walk", theme::muted())),
    }
    f.render_widget(Paragraph::new(lines), info);
}

fn truncate(text: &str, width: usize) -> String {
    if text.width() <= width {
        return text.to_string();
    }
    let mut out = String::new();
    for c in text.chars() {
        if out.width() + 2 > width {
            break;
        }
        out.push(c);
    }
    out.push('…');
    out
}

// ----- Input -----

fn input_height(app: &mut App, width: u16) -> u16 {
    let inner = width.saturating_sub(6);
    let rows = app.current_input().map_or(1, |i| i.layout(inner).0.len() as u16);
    rows.clamp(1, 8) + 2
}

fn draw_input(f: &mut Frame, app: &mut App, area: Rect) {
    let Some((i, puppet)) = app.current() else { return };
    let s = &mut app.sessions[i];
    let password = s.password && puppet.is_none();
    let connected = s.status == Status::Connected;
    let border = if connected { theme::muted() } else { theme::faint() };
    let mut block = Block::bordered().border_type(BorderType::Rounded).border_style(border);
    // The server's prompt rides on the top border.
    if puppet.is_none()
        && let Some(prompt) = &s.prompt
    {
        let mut spans = vec![Span::raw(" ")];
        for span in &prompt.spans {
            spans.push(Span::styled(prompt.span_text(span).to_string(), theme::style(&span.style)));
        }
        spans.push(Span::raw(" "));
        block = block.title(Line::from(spans));
    }
    let badge = if let Some(name) = &puppet {
        Some(Span::styled(format!(" as {name} "), Style::new().fg(Color::Black).bg(ACCENT_2)))
    } else if password {
        Some(Span::styled(" 🔒 password ", Style::new().fg(Color::Black).bg(WARN)))
    } else {
        (!s.status_text.is_empty()).then(|| Span::styled(format!(" {} ", s.status_text), theme::muted()))
    };
    if let Some(badge) = badge {
        block = block.title(Line::from(badge).right_aligned());
    }
    let inner = block.inner(area);
    f.render_widget(block, area);
    let [glyph, text_area] = Layout::horizontal([Constraint::Length(2), Constraint::Min(1)]).areas(inner);
    f.render_widget(Paragraph::new(Span::styled("›", theme::accent().add_modifier(Modifier::BOLD))), glyph);
    let input = match &puppet {
        Some(name) => &s.puppet_mut(name).input,
        None => &s.input,
    };
    if input.is_empty() {
        let placeholder = if matches!(s.status, Status::Disconnected(_)) && puppet.is_none() {
            "Disconnected — press Enter to reconnect".to_string()
        } else if password {
            "Password (hidden)".to_string()
        } else {
            match app.keymap.keys("palette").first() {
                Some(key) => format!("Type a command · / for client commands · {} for everything else", crate::keys::pretty(key)),
                None => "Type a command · / for client commands".to_string(),
            }
        };
        f.render_widget(Paragraph::new(Span::styled(placeholder, theme::faint().add_modifier(Modifier::ITALIC))), text_area);
        f.set_cursor_position((text_area.x, text_area.y));
        return;
    }
    let (rows, (cx, cy)) = input.layout(text_area.width);
    let first = (cy as usize + 1).saturating_sub(text_area.height as usize);
    let lines: Vec<Line> = rows
        .iter()
        .skip(first)
        .take(text_area.height as usize)
        .map(|r| if password { Line::raw("•".repeat(r.width())) } else { highlight_command(r) })
        .collect();
    f.render_widget(Paragraph::new(lines), text_area);
    f.set_cursor_position((text_area.x + cx, text_area.y + cy - first as u16));
}

/// Colours a client command's name as you type it.
fn highlight_command(row: &str) -> Line<'static> {
    if let Some(rest) = row.strip_prefix('/')
        && !rest.starts_with('/')
    {
        let end = rest.find(' ').map_or(row.len(), |i| i + 1);
        return Line::from(vec![Span::styled(row[..end].to_string(), Style::new().fg(ACCENT_2)), Span::raw(row[end..].to_string())]);
    }
    Line::raw(row.to_string())
}

fn draw_suggestions(f: &mut Frame, app: &mut App, input: Rect, output: Rect) {
    let suggestions = app.suggestions();
    if suggestions.is_empty() {
        return;
    }
    let n = suggestions.len();
    app.suggestion = app.suggestion.min(n - 1);
    let visible = n.min(8);
    let first = app.suggestion.saturating_sub(visible - 1);
    let height = visible as u16 + 2;
    let right = (output.x + output.width).min(input.x + input.width);
    let outer = Rect { x: input.x + 1, y: input.y.saturating_sub(height), width: right.saturating_sub(input.x + 2), height };
    f.render_widget(Clear, outer);
    let more = if n > visible { format!(" {}/{} ", app.suggestion + 1, n) } else { String::new() };
    let block = Block::bordered()
        .border_type(BorderType::Rounded)
        .border_style(theme::faint())
        .title_bottom(Line::from(vec![Span::styled(" Tab", Style::new().fg(ACCENT_2)), Span::styled(" complete · ", theme::faint()), Span::styled("Esc", Style::new().fg(ACCENT_2)), Span::styled(" hide ", theme::faint())]))
        .title_bottom(Line::styled(more, theme::faint()).right_aligned());
    let area = block.inner(outer);
    f.render_widget(block, outer);
    let label_width = suggestions.iter().map(|s| s.label.width()).max().unwrap_or(0).min(area.width as usize / 2);
    let lines: Vec<Line> = suggestions
        .iter()
        .enumerate()
        .skip(first)
        .take(visible)
        .map(|(i, s)| {
            let selected = i == app.suggestion;
            let bg = if selected { SELECTED_BG } else { Color::Reset };
            Line::from(vec![
                Span::styled(if selected { "▌" } else { " " }, Style::new().fg(ACCENT).bg(bg)),
                Span::styled(format!("{:<label_width$}  ", truncate(&s.label, label_width)), Style::new().fg(if selected { ACCENT_2 } else { Color::Gray }).bg(bg)),
                Span::styled(s.detail.clone(), Style::new().fg(MUTED).bg(bg)),
            ])
            .style(Style::new().bg(bg))
        })
        .collect();
    f.render_widget(Paragraph::new(lines), area);
}

// ----- Status line -----

fn draw_status(f: &mut Frame, app: &mut App, area: Rect) {
    let Some((i, _)) = app.current() else { return };
    let s = &app.sessions[i];
    let mut left = vec![Span::raw(" ")];
    match &s.status {
        Status::Connected => left.push(Span::styled("● connected", Style::new().fg(GOOD))),
        Status::Connecting => left.push(Span::styled(format!("{} connecting…", spinner(app)), Style::new().fg(WARN))),
        Status::Disconnected(_) => left.push(Span::styled("○ disconnected", Style::new().fg(BAD))),
    }
    let world = s.engine.world();
    left.push(Span::styled(format!("  {}:{}", world.host, world.port), theme::faint()));
    if world.tls {
        left.push(Span::styled(" 🔒", theme::faint()));
    }
    if s.logging {
        left.push(Span::styled("  ⏺ logging", Style::new().fg(BAD)));
    }
    let elsewhere: usize = app.sessions.iter().enumerate().filter(|(n, _)| *n != i).map(|(_, s)| s.unread).sum();
    if elsewhere > 0 {
        left.push(Span::styled(format!("  ◆ {elsewhere} unread in other tabs"), Style::new().fg(ACCENT_2)));
    }
    let right = if app.quit_armed.is_some() {
        Line::styled("Press Ctrl+C again to quit ", Style::new().fg(WARN))
    } else {
        let hints = [("palette", "commands"), ("connect", "connect"), ("toggle-sidebar", "sidebar"), ("keyboard", "keys")];
        let mut spans = Vec::new();
        for (id, what) in hints {
            if let Some(key) = app.keymap.keys(id).first() {
                spans.push(key_hint_owned(crate::keys::compact(key)));
                spans.push(Span::styled(format!(" {what}  "), theme::faint()));
            }
        }
        Line::from(spans)
    };
    let right_width = right.width() as u16;
    let [l, r] = Layout::horizontal([Constraint::Min(1), Constraint::Length(right_width)]).areas(area);
    f.render_widget(Paragraph::new(Line::from(left)), l);
    f.render_widget(Paragraph::new(right.right_aligned()), r);
}

// ----- Overlays -----

fn centered(area: Rect, width: u16, height: u16) -> Rect {
    let w = width.min(area.width.saturating_sub(4));
    let h = height.min(area.height.saturating_sub(2));
    Rect { x: area.x + (area.width - w) / 2, y: area.y + (area.height - h) / 3, width: w, height: h }
}

fn draw_overlay(f: &mut Frame, app: &mut App, area: Rect) {
    if let Some(Overlay::Rules(view)) = &mut app.overlay {
        return draw_rules(f, view, &app.config, centered(area, 104, 34));
    }
    if let Some(Overlay::Keys(view)) = &app.overlay {
        return draw_keys(f, app, view, centered(area, 100, 36));
    }
    match &app.overlay {
        // Over the status line.
        Some(Overlay::Find(find)) => draw_find(f, app, find, Rect { y: area.y + area.height.saturating_sub(1), height: 1.min(area.height), ..area }),
        Some(Overlay::Picker(p)) if !(app.tabs.is_empty() && p.kind == PickerKind::Connect) => {
            let r = centered(area, 80, 24);
            f.render_widget(Clear, r);
            let mut hits = Vec::new();
            draw_picker(f, p, r, true, &mut hits);
            app.hits.picker_items = hits;
        }
        Some(Overlay::Help) => draw_help(f, app, centered(area, 96, 29)),
        Some(Overlay::World(form)) => draw_world_form(f, form, centered(area, 72, 21)),
        _ => {}
    }
}

fn draw_picker(f: &mut Frame, p: &Picker, area: Rect, closable: bool, hits: &mut Vec<(Rect, usize)>) {
    let footer = match p.kind {
        PickerKind::Connect if !closable => " Enter connect · ^N new · ^E edit · ^D delete · Esc clear ",
        PickerKind::Connect => " Enter connect · ^N new · ^E edit · ^D delete · Esc close ",
        PickerKind::Palette => " ↑↓ choose · Enter run · Esc close ",
        PickerKind::History => " ↑↓ choose · Enter use · Esc close ",
    };
    let block = Block::bordered()
        .border_type(BorderType::Rounded)
        .border_style(theme::accent())
        .title(Line::from(vec![Span::raw(" "), Span::styled(p.title, theme::accent().add_modifier(Modifier::BOLD)), Span::raw(" ")]))
        .title_bottom(Line::styled(footer, theme::faint()).centered());
    let inner = block.inner(area).inner(Margin { horizontal: 1, vertical: 0 });
    f.render_widget(block, area);
    let [query, _, list] = Layout::vertical([Constraint::Length(1), Constraint::Length(1), Constraint::Min(1)]).areas(inner);
    let placeholder = match p.kind {
        PickerKind::Connect => "Search worlds, or type host:port",
        PickerKind::Palette => "Search commands",
        PickerKind::History => "Search history",
    };
    let q = if p.query.is_empty() { Span::styled(placeholder, theme::faint().add_modifier(Modifier::ITALIC)) } else { Span::raw(p.query.text.clone()) };
    f.render_widget(Paragraph::new(Line::from(vec![Span::styled("⌕ ", theme::accent()), q])), query);
    f.set_cursor_position((query.x + 2 + p.query.text[..p.query.cursor].width() as u16, query.y));

    // Rows, with a header whenever the group changes (when not filtering).
    let grouped = p.query.text.trim().is_empty() && p.kind != PickerKind::History;
    let mut rows: Vec<(Option<usize>, Line)> = Vec::new();
    let mut last_group = None;
    for (n, (idx, positions)) in p.matches.iter().enumerate() {
        let item = &p.items[*idx];
        if grouped && last_group != Some(item.group) && !item.group.is_empty() {
            rows.push((None, Line::styled(item.group.to_uppercase(), theme::faint().add_modifier(Modifier::BOLD))));
            last_group = Some(item.group);
        }
        let selected = n == p.selected;
        let bg = if selected { SELECTED_BG } else { Color::Reset };
        let mut spans = vec![Span::styled(if selected { "▌ " } else { "  " }, Style::new().fg(ACCENT).bg(bg))];
        for (ci, c) in item.label.chars().enumerate() {
            let hit = positions.contains(&ci);
            let style = if hit { Style::new().fg(ACCENT).add_modifier(Modifier::BOLD) } else if selected { Style::new().fg(Color::White) } else { Style::new().fg(Color::Gray) };
            spans.push(Span::styled(c.to_string(), style.bg(bg)));
        }
        let used: usize = spans.iter().map(Span::width).sum();
        let detail = truncate(&item.detail, (list.width as usize).saturating_sub(used + 2));
        let pad = (list.width as usize).saturating_sub(used + detail.width());
        spans.push(Span::styled(" ".repeat(pad), Style::new().bg(bg)));
        spans.push(Span::styled(detail, Style::new().fg(MUTED).bg(bg)));
        rows.push((Some(n), Line::from(spans)));
    }
    if p.matches.is_empty() {
        rows.push((None, Line::styled("No matches", theme::muted())));
    }
    // Keep the selection in view.
    let height = list.height as usize;
    let sel_row = rows.iter().position(|(n, _)| *n == Some(p.selected)).unwrap_or(0);
    let first = (sel_row + 1).saturating_sub(height);
    for (y, (n, line)) in rows.into_iter().skip(first).take(height).enumerate() {
        let r = Rect { x: list.x, y: list.y + y as u16, width: list.width, height: 1 };
        if let Some(n) = n {
            hits.push((r, n));
        }
        f.render_widget(Paragraph::new(line), r);
    }
}

fn draw_world_form(f: &mut Frame, form: &WorldForm, area: Rect) {
    f.render_widget(Clear, area);
    let title = if form.editing.is_some() { " Edit world " } else { " New world " };
    let block = Block::bordered()
        .border_type(BorderType::Rounded)
        .border_style(theme::accent())
        .title(Line::styled(title, theme::accent().add_modifier(Modifier::BOLD)))
        .title_bottom(Line::styled(" Tab next · Enter save & connect · ^S save · Esc cancel ", theme::faint()).centered());
    let inner = block.inner(area).inner(Margin { horizontal: 2, vertical: 1 });
    f.render_widget(block, area);
    let label_w = 11u16;
    let mut y = inner.y;
    for (i, field) in form.fields.iter().enumerate() {
        if y + 2 > inner.y + inner.height {
            break;
        }
        let focused = i == form.focus;
        let label_style = if focused { theme::accent().add_modifier(Modifier::BOLD) } else { theme::muted() };
        f.render_widget(Paragraph::new(Line::styled(format!("{:>w$} ", field.label, w = label_w as usize - 1), label_style)), Rect { x: inner.x, y, width: label_w, height: 1 });
        let value_area = Rect { x: inner.x + label_w, y, width: inner.width.saturating_sub(label_w), height: 1 };
        let marker = if focused { Span::styled("▌", theme::accent()) } else { Span::styled("│", theme::faint()) };
        let value = match field.kind {
            FieldKind::Toggle => Span::styled(
                if field.on { "● on " } else { "○ off" },
                Style::new().fg(if field.on { GOOD } else { MUTED }).add_modifier(if focused { Modifier::BOLD } else { Modifier::empty() }),
            ),
            FieldKind::Secret if !focused && !field.input.is_empty() => Span::styled("•".repeat(field.input.text.chars().count().min(24)), Style::new().fg(Color::Gray)),
            _ if field.input.is_empty() && !focused => Span::styled("—", theme::faint()),
            _ => Span::styled(field.input.text.clone(), Style::new().fg(if focused { Color::White } else { Color::Gray })),
        };
        f.render_widget(Paragraph::new(Line::from(vec![marker, Span::raw(" "), value])), value_area);
        if focused && field.kind != FieldKind::Toggle {
            let col = field.input.text[..field.input.cursor].width() as u16;
            f.set_cursor_position((value_area.x + 2 + col.min(value_area.width.saturating_sub(3)), y));
        }
        if !field.hint.is_empty() {
            f.render_widget(Paragraph::new(Span::styled(format!("  {}", field.hint), theme::faint().add_modifier(Modifier::ITALIC))), Rect { y: y + 1, ..value_area });
        }
        y += 2;
    }
    if let Some(error) = &form.error {
        let r = Rect { x: inner.x, y: inner.y + inner.height - 1, width: inner.width, height: 1 };
        f.render_widget(Paragraph::new(Span::styled(format!("✗ {error}"), Style::new().fg(BAD))), r);
    }
}

// ----- Rules -----

fn draw_rules(f: &mut Frame, view: &mut RulesView, config: &Config, area: Rect) {
    f.render_widget(Clear, area);
    let scope = view.scope().label();
    if let Some(form) = &mut view.form {
        return draw_rule_form(f, form, &scope, area);
    }
    let footer = if view.delete_armed {
        Line::styled(" Press d again to delete it · any other key keeps it ", Style::new().fg(WARN))
    } else if let Some(notice) = &view.notice {
        Line::styled(format!(" {notice} "), theme::accent())
    } else {
        Line::styled(" Enter edit · n new · x examples · Space on/off · d delete · Shift+↑↓ reorder · Tab section · [ ] scope · Esc close ", theme::faint())
    };
    let block = Block::bordered()
        .border_type(BorderType::Rounded)
        .border_style(theme::accent())
        .title(Line::styled(" Triggers, aliases and macros ", theme::accent().add_modifier(Modifier::BOLD)))
        .title_bottom(footer.centered());
    let inner = block.inner(area).inner(Margin { horizontal: 2, vertical: 0 });
    f.render_widget(block, area);
    let [_, tabs, _, list, about] = Layout::vertical([Constraint::Length(1), Constraint::Length(1), Constraint::Length(1), Constraint::Min(1), Constraint::Length(2)]).areas(inner);

    // The sections on the left, the scope on the right.
    let counts = view.counts(config);
    let mut spans = Vec::new();
    for (i, (name, _, _)) in rules::SECTIONS.iter().enumerate() {
        let style = if i == view.section { Style::new().fg(ACCENT).add_modifier(Modifier::BOLD | Modifier::UNDERLINED) } else { theme::muted() };
        spans.push(Span::styled(*name, style));
        if counts[i] > 0 {
            spans.push(Span::styled(format!(" {}", counts[i]), theme::faint()));
        }
        spans.push(Span::raw("   "));
    }
    f.render_widget(Paragraph::new(Line::from(spans)), tabs);
    let applies = Line::from(vec![Span::styled("Applies to ", theme::faint()), Span::styled("‹ ", theme::faint()), Span::styled(scope, Style::new().fg(ACCENT_2)), Span::styled(" ›", theme::faint())]);
    f.render_widget(Paragraph::new(applies.right_aligned()), tabs);

    let rows = view.rows(config);
    view.selected = view.selected.min(rows.len().saturating_sub(1));
    if rows.is_empty() {
        let noun = rules::SECTIONS[view.section].2;
        let line = Line::from(vec![Span::styled("Nothing here yet. Press ", theme::muted()), key_hint("n"), Span::styled(format!(" to add a{} {noun}, or ", if noun == "alias" { "n" } else { "" }), theme::muted()), key_hint("x"), Span::styled(" for some examples to learn from.", theme::muted())]);
        f.render_widget(Paragraph::new(line), list);
    }
    let title_width = rows.iter().map(|r| r.title.width()).max().unwrap_or(0).min(list.width as usize * 2 / 5);
    let height = list.height as usize;
    let first = (view.selected + 1).saturating_sub(height);
    let lines: Vec<Line> = rows
        .iter()
        .enumerate()
        .skip(first)
        .take(height)
        .map(|(i, row)| {
            let selected = i == view.selected;
            let bg = if selected { SELECTED_BG } else { Color::Reset };
            let off = row.enabled == Some(false);
            let dot = match row.enabled {
                Some(true) => Span::styled("● ", Style::new().fg(GOOD).bg(bg)),
                Some(false) => Span::styled("○ ", Style::new().fg(MUTED).bg(bg)),
                None => Span::styled("  ", Style::new().bg(bg)),
            };
            let title_style = if off { theme::muted() } else if selected { Style::new().fg(Color::White).add_modifier(Modifier::BOLD) } else { Style::new().fg(Color::Gray) };
            let used = 4 + title_width + 2;
            let detail = truncate(&row.detail, (list.width as usize).saturating_sub(used));
            let pad = (list.width as usize).saturating_sub(used + detail.width());
            Line::from(vec![
                Span::styled(if selected { "▌ " } else { "  " }, Style::new().fg(ACCENT).bg(bg)),
                dot,
                Span::styled(format!("{:<title_width$}  ", truncate(&row.title, title_width)), title_style.bg(bg)),
                Span::styled(detail, Style::new().fg(MUTED).bg(bg)),
                Span::styled(" ".repeat(pad), Style::new().bg(bg)),
            ])
        })
        .collect();
    f.render_widget(Paragraph::new(lines), list);
    f.render_widget(Paragraph::new(Span::styled(rules::SECTIONS[view.section].1, theme::faint().add_modifier(Modifier::ITALIC))).wrap(ratatui::widgets::Wrap { trim: true }), about);
}

fn draw_rule_form(f: &mut Frame, form: &mut RuleForm, scope: &str, area: Rect) {
    let block = Block::bordered()
        .border_type(BorderType::Rounded)
        .border_style(theme::accent())
        .title(Line::from(vec![Span::styled(format!(" {} ", form.title()), theme::accent().add_modifier(Modifier::BOLD)), Span::styled(format!("· {scope} "), theme::faint())]))
        .title_bottom(Line::styled(" Tab next · Space switch · Enter save · Esc back ", theme::faint()).centered());
    let inner = block.inner(area).inner(Margin { horizontal: 2, vertical: 0 });
    f.render_widget(block, area);
    let [_, body, result, hint] = Layout::vertical([Constraint::Length(1), Constraint::Min(1), Constraint::Length(1), Constraint::Length(1)]).areas(inner);

    let label_width = form.fields.iter().map(|field| field.label.width()).max().unwrap_or(0) + 1;
    let value_width = (body.width as usize).saturating_sub(label_width + 2);
    // Every row of the form, and where the cursor goes among them.
    let mut lines: Vec<Line> = Vec::new();
    let mut focus_rows = 0..0;
    let mut cursor = None;
    for (i, field) in form.fields.iter().enumerate() {
        if !field.group.is_empty() {
            if !lines.is_empty() {
                lines.push(Line::raw(""));
            }
            lines.push(Line::styled(field.group.to_uppercase(), theme::faint().add_modifier(Modifier::BOLD)));
        }
        let focused = i == form.focus;
        let label_style = if focused { theme::accent().add_modifier(Modifier::BOLD) } else { theme::muted() };
        let marker = if focused { Span::styled("▌ ", theme::accent()) } else { Span::styled("│ ", theme::faint()) };
        let text_style = Style::new().fg(if focused { Color::White } else { Color::Gray });
        let start = lines.len();
        match field.kind {
            FieldKind::Toggle => {
                let style = Style::new().fg(if field.on { GOOD } else { MUTED }).add_modifier(if focused { Modifier::BOLD } else { Modifier::empty() });
                lines.push(Line::from(vec![Span::styled(format!("{:>label_width$} ", field.label), label_style), marker, Span::styled(if field.on { "● on " } else { "○ off" }, style)]));
            }
            _ => {
                // One row per line of text; only a multi-line field has more than one.
                let mut offset = 0;
                for (n, text) in field.input.text.split('\n').enumerate() {
                    let label = if n == 0 { field.label } else { "" };
                    let mut spans = vec![Span::styled(format!("{label:>label_width$} "), label_style), marker.clone()];
                    if text.is_empty() && !focused && n == 0 {
                        spans.push(Span::styled("—", theme::faint()));
                    } else {
                        spans.push(Span::styled(truncate(text, value_width), text_style));
                    }
                    if rules::is_color(text.trim())
                        && let Ok(rgb) = u32::from_str_radix(&text.trim()[1..], 16)
                    {
                        spans.push(Span::styled("  ██", Style::new().fg(Color::Rgb((rgb >> 16) as u8, (rgb >> 8) as u8, rgb as u8))));
                    }
                    let at = field.input.cursor;
                    if focused && (offset..=offset + text.len()).contains(&at) {
                        let col = text[..at - offset].width().min(value_width.saturating_sub(1));
                        cursor = Some((lines.len(), (label_width + 3 + col) as u16));
                    }
                    offset += text.len() + 1;
                    lines.push(Line::from(spans));
                }
            }
        }
        if focused {
            focus_rows = start..lines.len();
        }
    }
    // Keep the focused field in view, with its heading when that fits.
    let height = body.height as usize;
    let first = focus_rows.end.saturating_sub(height).min(focus_rows.start);
    if let Some((row, col)) = cursor
        && row >= first
        && row < first + height
        && !form.recording
    {
        f.set_cursor_position((body.x + col, body.y + (row - first) as u16));
    }
    let more = lines.len() > first + height;
    f.render_widget(Paragraph::new(lines.into_iter().skip(first).take(height).collect::<Vec<_>>()), body);
    if more {
        f.render_widget(Paragraph::new(Line::styled("▼ more", theme::faint()).right_aligned()), Rect { y: body.y + body.height - 1, height: 1, ..body });
    }

    if let Some(text) = form.test_result() {
        f.render_widget(Paragraph::new(Line::from(vec![Span::styled("Result  ", theme::faint()), Span::styled(text, Style::new().fg(ACCENT_2))])), result);
    }
    let line = if form.recording {
        Line::styled("Press the key for this macro… (Esc to cancel)", Style::new().fg(WARN))
    } else if let Some(error) = &form.error {
        Line::styled(format!("✗ {error}"), Style::new().fg(BAD))
    } else {
        Line::styled(form.fields[form.focus].hint, theme::faint().add_modifier(Modifier::ITALIC))
    };
    f.render_widget(Paragraph::new(line), hint);
}

/// The search bar, over the status line.
fn draw_find(f: &mut Frame, app: &App, find: &Find, area: Rect) {
    f.render_widget(Clear, area);
    let (n, total) = find.position;
    let count = match (find.query.is_empty(), total) {
        (true, _) => Span::raw(""),
        (false, 0) => Span::styled(" no matches ", Style::new().fg(BAD)),
        (false, _) => Span::styled(format!(" {n} of {total} "), theme::muted()),
    };
    // Find Next goes to newer matches, as ↓ does.
    let newer = match app.key_for("find-next") {
        Some(key) => key_hint_owned(format!("↓ {key}")),
        None => key_hint("↓"),
    };
    let right = vec![count, key_hint("Enter ↑"), Span::styled(" older  ", theme::faint()), newer, Span::styled(" newer  ", theme::faint()), key_hint("Esc"), Span::styled(" close ", theme::faint())];
    let right = Line::from(right);
    let [l, r] = Layout::horizontal([Constraint::Min(10), Constraint::Length(right.width() as u16)]).areas(area);
    let label = " Find in the output: ";
    let query = if find.query.is_empty() { Span::styled("type to search", theme::faint().add_modifier(Modifier::ITALIC)) } else { Span::raw(find.query.text.clone()) };
    f.render_widget(Paragraph::new(Line::from(vec![Span::styled(label, theme::accent().add_modifier(Modifier::BOLD)), query])), l);
    f.render_widget(Paragraph::new(right.right_aligned()), r);
    let cursor = label.width() + find.query.text[..find.query.cursor].width();
    f.set_cursor_position((l.x + (cursor as u16).min(l.width.saturating_sub(1)), l.y));
}

/// The keyboard shortcuts window: the actions in their groups, each with its keys; under the
/// list, what else uses the selected key.
fn draw_keys(f: &mut Frame, app: &App, view: &KeysView, area: Rect) {
    f.render_widget(Clear, area);
    let views = app.keymap.describe(&app.config);
    let selected = view.selected.min(views.len().saturating_sub(1));
    let footer = if view.recording {
        let label = views.get(selected).map_or("", |v| v.label);
        Line::styled(format!(" Press the keys for {label} · Esc to cancel "), Style::new().fg(WARN))
    } else if view.reset_armed {
        Line::styled(" Press R again to give every shortcut its usual keys · any other key keeps them ", Style::new().fg(WARN))
    } else if let Some(notice) = &view.notice {
        Line::styled(format!(" {notice} "), theme::accent())
    } else {
        Line::styled(" a add a key · d remove it · ← → choose a key · r reset · R reset all · ? other keys · Esc close ", theme::faint())
    };
    let block = Block::bordered()
        .border_type(BorderType::Rounded)
        .border_style(theme::accent())
        .title(Line::styled(" Keyboard shortcuts ", theme::accent().add_modifier(Modifier::BOLD)))
        .title_bottom(footer.centered());
    let inner = block.inner(area).inner(Margin { horizontal: 2, vertical: 0 });
    f.render_widget(block, area);
    let [_, list, _, about] = Layout::vertical([Constraint::Length(1), Constraint::Min(1), Constraint::Length(1), Constraint::Length(3)]).areas(inner);

    // Rows, with a heading for each group.
    let label_width = views.iter().map(|v| v.label.width()).max().unwrap_or(0).min(list.width as usize / 2) + 2;
    let mut rows: Vec<(Option<usize>, Line)> = Vec::new();
    for (i, v) in views.iter().enumerate() {
        if i == 0 || views[i - 1].group != v.group {
            if i > 0 {
                rows.push((None, Line::raw("")));
            }
            rows.push((None, Line::styled(v.group.to_uppercase(), theme::faint().add_modifier(Modifier::BOLD))));
        }
        let on = i == selected;
        let bg = if on { SELECTED_BG } else { Color::Reset };
        let label_style = if on { Style::new().fg(Color::White).add_modifier(Modifier::BOLD) } else { Style::new().fg(Color::Gray) };
        let mut spans = vec![
            Span::styled(if on { "▌ " } else { "  " }, Style::new().fg(ACCENT).bg(bg)),
            Span::styled(format!("{:<label_width$}", truncate(v.label, label_width - 2)), label_style.bg(bg)),
        ];
        if v.keys.is_empty() {
            spans.push(Span::styled("no key", theme::faint().bg(bg)));
        }
        for (k, key) in v.keys.iter().enumerate() {
            let text = crate::keys::pretty_text(&key.key);
            let clash = !key.clashes.is_empty() || !key.macros.is_empty();
            let mut style = Style::new().fg(if clash { WARN } else { ACCENT_2 }).bg(bg);
            if on && k == view.key.min(v.keys.len() - 1) {
                style = Style::new().fg(Color::Black).bg(if clash { WARN } else { ACCENT_2 });
            }
            spans.push(Span::styled(format!(" {text}{} ", if clash { " !" } else { "" }), style));
            spans.push(Span::styled(" ", Style::new().bg(bg)));
        }
        if v.changed {
            spans.push(Span::styled("changed", theme::faint().add_modifier(Modifier::ITALIC).bg(bg)));
        }
        let used: usize = spans.iter().map(Span::width).sum();
        spans.push(Span::styled(" ".repeat((list.width as usize).saturating_sub(used)), Style::new().bg(bg)));
        rows.push((Some(i), Line::from(spans)));
    }
    // Keep the selection in view.
    let height = list.height as usize;
    let at = rows.iter().position(|(n, _)| *n == Some(selected)).unwrap_or(0);
    let first = (at + 1).saturating_sub(height);
    f.render_widget(Paragraph::new(rows.into_iter().skip(first).take(height).map(|(_, l)| l).collect::<Vec<_>>()), list);

    // About the selected key: what else has it, and its usual keys.
    let mut lines = Vec::new();
    if let Some(v) = views.get(selected) {
        if let Some(key) = v.keys.get(view.key.min(v.keys.len().saturating_sub(1))) {
            let text = crate::keys::pretty_text(&key.key);
            if !key.clashes.is_empty() {
                lines.push(Line::styled(format!("{text} is also {}: the first in this list wins.", key.clashes.join(", ")), Style::new().fg(WARN)));
            }
            if !key.macros.is_empty() {
                let whose: Vec<String> = key.macros.iter().map(|m| format!("Macro: {m}")).collect();
                lines.push(Line::styled(format!("{text} is also a keyboard macro ({}), which won't run while this shortcut has the key.", whose.join(", ")), Style::new().fg(WARN)));
            }
        }
        if v.changed {
            let usual: Vec<String> = v.defaults.iter().map(|d| crate::keys::pretty_text(d)).collect();
            let usual = if usual.is_empty() { "no key".to_string() } else { usual.join(", ") };
            lines.push(Line::styled(format!("Usually {usual}. Press r to go back to that."), theme::faint().add_modifier(Modifier::ITALIC)));
        }
    }
    if lines.is_empty() {
        lines.push(Line::styled("Shortcuts come before keyboard macros on the same key. Changes are saved at once.", theme::faint().add_modifier(Modifier::ITALIC)));
    }
    f.render_widget(Paragraph::new(lines).wrap(ratatui::widgets::Wrap { trim: true }), about);
}

/// The keys that aren't shortcuts (typing and reading), and the shortcuts' current keys.
fn draw_help(f: &mut Frame, app: &App, area: Rect) {
    f.render_widget(Clear, area);
    let block = Block::bordered()
        .border_type(BorderType::Rounded)
        .border_style(theme::accent())
        .title(Line::styled(" Keys ", theme::accent().add_modifier(Modifier::BOLD)))
        .title_bottom(Line::styled(" any key to close ", theme::faint()).centered());
    let inner = block.inner(area).inner(Margin { horizontal: 2, vertical: 1 });
    f.render_widget(block, area);
    let keys = |id: &str| keys_of(app, id);
    let first = |ids: &[&str]| first_keys(app, ids);
    let sections: Vec<(&str, Vec<(String, &str)>)> = vec![
        ("Typing", vec![
            ("Enter".into(), "send · on an empty line, reconnect"),
            ("Shift+Enter, Ctrl+J".into(), "new line"),
            (format!("↑ ↓ · {}", keys("history-search")), "history · search it"),
            ("Tab".into(), "complete a name from the output"),
            (first(&["edit-input", "reopen-editor"]), "$EDITOR: the input · fix the last program"),
            (keys("editor-focus"), "between the world and an editor beside it"),
            (keys("move-editor"), "move the editor between a tab and the sidebar"),
            ("Ctrl+A E U K W Y".into(), "readline editing"),
            ("Esc Esc · Ctrl+C".into(), "clear the input"),
        ]),
        ("Reading", vec![
            ("PgUp PgDn · wheel".into(), "scroll back (splits the view)"),
            ("Shift+↑ ↓".into(), "scroll a line"),
            ("Esc".into(), "jump back to the bottom"),
            (first(&["find", "find-next", "find-previous"]), "search the output · next · previous"),
            ("click".into(), "links, tabs and map rooms"),
            ("drag".into(), "select text; copied on release"),
        ]),
        ("Everything else", vec![
            (keys("palette"), "command palette"),
            (keys("connect"), "connect to a world"),
            (first(&["tab-1", "tab-9", "prev-tab", "next-tab"]), "tab 1 · 9 · switch tabs"),
            (first(&["toggle-sidebar", "prev-panel", "next-panel"]), "sidebar · its panels"),
            (keys("rules"), "triggers, aliases and macros"),
            (keys("keyboard"), "change these keys"),
            ("Ctrl+C Ctrl+C".into(), "quit"),
        ]),
    ];
    let [left, right] = Layout::horizontal([Constraint::Percentage(60), Constraint::Percentage(40)]).areas(inner);
    let mut lines = Vec::new();
    for (title, keys) in sections {
        if !lines.is_empty() {
            lines.push(Line::raw(""));
        }
        lines.push(Line::styled(title, theme::faint().add_modifier(Modifier::BOLD)));
        for (k, d) in keys {
            lines.push(Line::from(vec![Span::styled(format!("{:<22}", truncate(&k, 21)), Style::new().fg(ACCENT_2)), Span::styled(d, Style::new().fg(Color::Gray))]));
        }
    }
    f.render_widget(Paragraph::new(lines), left);
    let mut names: Vec<&str> = familiar_engine::COMMANDS.iter().chain(TUI_COMMANDS).map(|(u, _)| u.split([' ', '<']).next().unwrap_or(u)).collect();
    names.dedup();
    let mut cmds = vec![Line::styled("Client commands", theme::faint().add_modifier(Modifier::BOLD))];
    cmds.push(Line::from(names.iter().flat_map(|n| [Span::styled(*n, Style::new().fg(Color::Gray)), Span::raw("  ")]).collect::<Vec<_>>()));
    cmds.push(Line::raw(""));
    cmds.push(Line::from(vec![Span::styled("Type ", theme::muted()), Span::styled("/", Style::new().fg(ACCENT_2)), Span::styled(" to see what each does, or ", theme::muted()), Span::styled("/help", Style::new().fg(ACCENT_2)), Span::styled(". ", theme::muted()), Span::styled("//text", Style::new().fg(ACCENT_2)), Span::styled(" sends a line starting with /.", theme::muted())]));
    cmds.push(Line::raw(""));
    cmds.push(Line::from(vec![Span::styled("Help: ", theme::muted()), Span::styled("github.com/SpiraMirabilis/familiar", Style::new().fg(ACCENT_2))]));
    f.render_widget(Paragraph::new(cmds).wrap(ratatui::widgets::Wrap { trim: true }), right);
}
