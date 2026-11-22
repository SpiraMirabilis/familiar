//! Renders a Beip.Stats pane: one row per stat, with a name and a value, number, text or bar.

use std::sync::atomic::{AtomicUsize, Ordering};

use gtk::glib;
use gtk::prelude::*;
use familiar_engine::stats::{Align, BarColors, StatColor, StatPane, StatValue};

pub struct StatsPane {
    pub root: gtk::ScrolledWindow,
    grid: gtk::Grid,
    css: gtk::CssProvider,
    class: String,
}

fn hex(c: StatColor) -> Option<String> {
    match c {
        StatColor::Rgb(r, g, b) => Some(format!("#{r:02x}{g:02x}{b:02x}")),
        StatColor::Transparent => None,
    }
}

fn markup(text: &str, color: Option<StatColor>) -> String {
    let text = glib::markup_escape_text(text);
    match color.and_then(hex) {
        Some(c) => format!("<span foreground=\"{c}\">{text}</span>"),
        None => text.to_string(),
    }
}

fn rgba(c: Option<StatColor>, default: (f64, f64, f64, f64)) -> (f64, f64, f64, f64) {
    match c {
        Some(StatColor::Rgb(r, g, b)) => (r as f64 / 255.0, g as f64 / 255.0, b as f64 / 255.0, 1.0),
        Some(StatColor::Transparent) => (0.0, 0.0, 0.0, 0.0),
        None => default,
    }
}

/// A bar with the value drawn on top, for range and progress stats.
fn bar(fraction: f64, label: &str, colors: &BarColors, text_color: Option<StatColor>) -> gtk::Widget {
    let area = gtk::DrawingArea::builder().content_height(20).hexpand(true).build();
    let fill = rgba(colors.fill, (0.20, 0.52, 0.89, 1.0));
    let empty = rgba(colors.empty, (0.25, 0.25, 0.28, 1.0));
    let outline = rgba(colors.outline, (0.45, 0.45, 0.50, 1.0));
    area.set_draw_func(move |_, cr, w, h| {
        let (w, h) = (w as f64, h as f64);
        let rect = |cr: &gtk::cairo::Context, width: f64| {
            let r = 3.0_f64.min(h / 2.0);
            cr.new_sub_path();
            cr.arc(0.5 + width - r, 0.5 + r, r, -90f64.to_radians(), 0.0);
            cr.arc(0.5 + width - r, h - 0.5 - r, r, 0.0, 90f64.to_radians());
            cr.arc(0.5 + r, h - 0.5 - r, r, 90f64.to_radians(), 180f64.to_radians());
            cr.arc(0.5 + r, 0.5 + r, r, 180f64.to_radians(), 270f64.to_radians());
            cr.close_path();
        };
        let full = (w - 1.0).max(0.0);
        rect(cr, full);
        cr.set_source_rgba(empty.0, empty.1, empty.2, empty.3);
        let _ = cr.fill();
        let filled = full * fraction;
        if filled >= 1.0 {
            rect(cr, filled.max(6.0).min(full));
            cr.set_source_rgba(fill.0, fill.1, fill.2, fill.3);
            let _ = cr.fill();
        }
        rect(cr, full);
        cr.set_source_rgba(outline.0, outline.1, outline.2, outline.3);
        cr.set_line_width(1.0);
        let _ = cr.stroke();
    });
    let text = gtk::Label::builder().use_markup(true).css_classes(["caption"]).build();
    text.set_markup(&markup(label, text_color));
    let overlay = gtk::Overlay::builder().child(&area).hexpand(true).build();
    overlay.add_overlay(&text);
    overlay.upcast()
}

impl StatsPane {
    pub fn new() -> Self {
        // Each pane gets its own CSS class for its background colour.
        static NEXT_ID: AtomicUsize = AtomicUsize::new(0);
        let id = NEXT_ID.fetch_add(1, Ordering::Relaxed);
        let grid = gtk::Grid::builder()
            .column_spacing(12)
            .row_spacing(6)
            .margin_start(10)
            .margin_end(10)
            .margin_top(10)
            .margin_bottom(10)
            .build();
        let class = format!("mu-stats-{id}");
        let root = gtk::ScrolledWindow::builder()
            .child(&grid)
            .hscrollbar_policy(gtk::PolicyType::Never)
            .css_classes([class.as_str()])
            .build();
        let css = gtk::CssProvider::new();
        if let Some(display) = gtk::gdk::Display::default() {
            gtk::style_context_add_provider_for_display(&display, &css, gtk::STYLE_PROVIDER_PRIORITY_APPLICATION);
        }
        Self { root, grid, css, class }
    }

    pub fn update(&self, pane: &StatPane) {
        let background = pane.background.and_then(hex);
        self.css.load_from_string(&match background {
            Some(c) => format!(".{} {{ background-color: {c}; }}", self.class),
            None => String::new(),
        });

        while let Some(child) = self.grid.first_child() {
            self.grid.remove(&child);
        }
        for (row, (key, stat)) in pane.values.iter().enumerate() {
            let row = row as i32;
            let name = gtk::Label::builder()
                .use_markup(true)
                .xalign(match stat.name_alignment {
                    Align::Left => 0.0,
                    Align::Center => 0.5,
                    Align::Right => 1.0,
                })
                .css_classes(["dim-label"])
                .build();
            name.set_markup(&markup(stat.display_name(key), stat.name_color));
            if stat.name_color.is_some() {
                name.remove_css_class("dim-label");
            }
            let value: gtk::Widget = match &stat.value {
                StatValue::Range { colors, .. } | StatValue::Progress { colors, .. } => {
                    bar(stat.value.fraction().unwrap_or(0.0), &stat.value.text(), colors, stat.value_color)
                }
                other => {
                    let label = gtk::Label::builder().use_markup(true).xalign(0.0).wrap(true).selectable(true).hexpand(true).build();
                    label.set_markup(&markup(&other.text(), stat.value_color));
                    label.upcast()
                }
            };
            self.grid.attach(&name, 0, row, 1, 1);
            self.grid.attach(&value, 1, row, 1, 1);
        }
        if pane.values.is_empty() {
            let empty = gtk::Label::builder().label("No stats").css_classes(["dim-label"]).build();
            self.grid.attach(&empty, 0, 0, 2, 1);
        }
    }
}

impl Drop for StatsPane {
    fn drop(&mut self) {
        if let Some(display) = gtk::gdk::Display::default() {
            gtk::style_context_remove_provider_for_display(&display, &self.css);
        }
    }
}
