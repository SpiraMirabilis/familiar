//! The map tab: draws the session's map and lets the user walk, track and edit it.

use std::cell::{Cell, RefCell};
use std::rc::Rc;

use adw::prelude::*;
use gtk::{gdk, gio, glib, pango};
use mu_engine::{Engine, Output};
use mu_map::{Atlas, Room, RoomId, GRID_X, GRID_Y};

type Outputs = Box<dyn Fn(Vec<Output>)>;

pub struct MapView {
    pub root: gtk::Box,
    area: gtk::DrawingArea,
    areas: gtk::DropDown,
    area_names: RefCell<Vec<String>>,
    follow: gtk::ToggleButton,
    engine: Rc<RefCell<Engine>>,
    on_outputs: Outputs,
    /// Map units at the view's top-left corner.
    origin: Cell<(f64, f64)>,
    zoom: Cell<f64>,
    selected: Cell<Option<RoomId>>,
    /// The room being dragged, and where in it the pointer grabbed it.
    dragging: Cell<Option<(RoomId, f64, f64)>>,
    pan_start: Cell<(f64, f64)>,
    shown_current: Cell<Option<RoomId>>,
}

fn rgb(hex: &str) -> Option<(f64, f64, f64)> {
    let v = u32::from_str_radix(hex.strip_prefix('#')?, 16).ok()?;
    Some(((v >> 16 & 255) as f64 / 255.0, (v >> 8 & 255) as f64 / 255.0, (v & 255) as f64 / 255.0))
}

fn rounded(cr: &gtk::cairo::Context, x: f64, y: f64, w: f64, h: f64, r: f64) {
    let r = r.min(w / 2.0).min(h / 2.0);
    cr.new_sub_path();
    cr.arc(x + w - r, y + r, r, -90f64.to_radians(), 0.0);
    cr.arc(x + w - r, y + h - r, r, 0.0, 90f64.to_radians());
    cr.arc(x + r, y + h - r, r, 90f64.to_radians(), 180f64.to_radians());
    cr.arc(x + r, y + r, r, 180f64.to_radians(), 270f64.to_radians());
    cr.close_path();
}

impl MapView {
    pub fn new(engine: Rc<RefCell<Engine>>, on_outputs: impl Fn(Vec<Output>) + 'static) -> Rc<Self> {
        let area = gtk::DrawingArea::builder().hexpand(true).vexpand(true).focusable(true).build();
        let areas = gtk::DropDown::from_strings(&[]);
        areas.set_tooltip_text(Some("Area"));
        let search = gtk::SearchEntry::builder().placeholder_text("Find room").width_chars(10).hexpand(true).build();
        let center = gtk::Button::builder().icon_name("find-location-symbolic").tooltip_text("Center on my room").build();
        let follow = gtk::ToggleButton::builder()
            .icon_name("mark-location-symbolic")
            .tooltip_text("Follow my room as I move")
            .active(true)
            .build();
        let bar = gtk::Box::builder().spacing(4).margin_start(6).margin_end(6).margin_top(6).margin_bottom(6).build();
        bar.append(&areas);
        bar.append(&search);
        bar.append(&center);
        bar.append(&follow);
        let root = gtk::Box::new(gtk::Orientation::Vertical, 0);
        root.append(&bar);
        root.append(&area);

        let this = Rc::new(Self {
            root,
            area,
            areas,
            area_names: RefCell::default(),
            follow,
            engine,
            on_outputs: Box::new(on_outputs),
            origin: Cell::new((-40.0, -40.0)),
            zoom: Cell::new(1.0),
            selected: Cell::new(None),
            dragging: Cell::new(None),
            pan_start: Cell::new((0.0, 0.0)),
            shown_current: Cell::new(None),
        });

        let weak = Rc::downgrade(&this);
        this.area.set_draw_func(move |_, cr, w, h| {
            if let Some(this) = weak.upgrade() {
                this.draw(cr, w as f64, h as f64);
            }
        });
        let weak = Rc::downgrade(&this);
        this.areas.connect_selected_notify(move |_| {
            if let Some(this) = weak.upgrade() {
                this.area.queue_draw();
            }
        });
        let weak = Rc::downgrade(&this);
        center.connect_clicked(move |_| {
            if let Some(this) = weak.upgrade() {
                let current = this.engine.borrow().current_room();
                if let Some(room) = current {
                    this.center_on(room);
                }
            }
        });
        let weak = Rc::downgrade(&this);
        search.connect_activate(move |entry| {
            let Some(this) = weak.upgrade() else { return };
            let found = this.engine.borrow().map().search(&entry.text()).first().copied();
            if let Some(room) = found {
                this.selected.set(Some(room));
                this.center_on(room);
            }
        });
        this.setup_input();
        this.refresh();
        this
    }

    fn shown_area(&self) -> String {
        self.area_names.borrow().get(self.areas.selected() as usize).cloned().unwrap_or_default()
    }

    fn to_map(&self, x: f64, y: f64) -> (f64, f64) {
        let (ox, oy) = self.origin.get();
        let z = self.zoom.get();
        (ox + x / z, oy + y / z)
    }

    /// Call after the map changes: updates the area list and follows the current room.
    pub fn refresh(&self) {
        let (areas, current) = {
            let engine = self.engine.borrow();
            let current = engine.current_room().and_then(|id| engine.map().room(id).map(|r| (id, r.area.clone())));
            (engine.map().areas(), current)
        };
        if *self.area_names.borrow() != areas {
            let shown = self.shown_area();
            let labels: Vec<&str> = areas.iter().map(|a| if a.is_empty() { "(no area)" } else { a.as_str() }).collect();
            self.areas.set_model(Some(&gtk::StringList::new(&labels)));
            let keep = areas.iter().position(|a| *a == shown).unwrap_or(0);
            *self.area_names.borrow_mut() = areas;
            self.areas.set_selected(keep as u32);
        }
        if let Some((room, _)) = current
            && self.follow.is_active()
            && self.shown_current.get() != Some(room)
        {
            self.shown_current.set(Some(room));
            self.center_on(room);
        }
        self.area.queue_draw();
    }

    /// Shows the room's area and centres the view on it.
    pub fn center_on(&self, room: RoomId) {
        let Some((area, (cx, cy))) = self.engine.borrow().map().room(room).map(|r| (r.area.clone(), r.center())) else { return };
        if let Some(i) = self.area_names.borrow().iter().position(|a| *a == area) {
            self.areas.set_selected(i as u32);
        }
        let z = self.zoom.get();
        let (w, h) = (self.area.width().max(200) as f64, self.area.height().max(200) as f64);
        self.origin.set((cx - w / 2.0 / z, cy - h / 2.0 / z));
        self.area.queue_draw();
    }

    fn draw(&self, cr: &gtk::cairo::Context, width: f64, height: f64) {
        let dark = adw::StyleManager::default().is_dark();
        let (bg, fg, room_fill, line) = if dark {
            ((0.11, 0.11, 0.13), (0.92, 0.92, 0.92), (0.22, 0.24, 0.30), (0.55, 0.57, 0.62))
        } else {
            ((0.97, 0.97, 0.96), (0.1, 0.1, 0.1), (0.86, 0.89, 0.95), (0.45, 0.47, 0.52))
        };
        cr.set_source_rgb(bg.0, bg.1, bg.2);
        let _ = cr.paint();

        let engine = self.engine.borrow();
        let map: &Atlas = engine.map();
        if map.is_empty() {
            let layout = self.area.create_pango_layout(Some(
                "No rooms yet.\nThe map fills in as you move if the game sends GMCP Room.Info;\notherwise use /map addroom \"Name\" or right-click to add rooms.",
            ));
            layout.set_alignment(pango::Alignment::Center);
            let (lw, lh) = layout.pixel_size();
            cr.set_source_rgb(fg.0 * 0.6 + bg.0 * 0.4, fg.1 * 0.6 + bg.1 * 0.4, fg.2 * 0.6 + bg.2 * 0.4);
            cr.move_to((width - lw as f64) / 2.0, (height - lh as f64) / 2.0);
            pangocairo::functions::show_layout(cr, &layout);
            return;
        }

        let area = self.shown_area();
        let current = engine.current_room();
        let z = self.zoom.get();
        let (ox, oy) = self.origin.get();
        cr.save().ok();
        cr.scale(z, z);
        cr.translate(-ox, -oy);

        // Exits under rooms. Exits to rooms elsewhere (or not mapped yet) get a short stub.
        cr.set_line_width(1.5 / z.max(0.5));
        cr.set_source_rgb(line.0, line.1, line.2);
        let rooms: Vec<&Room> = map.rooms().filter(|r| r.area == area).collect();
        for room in &rooms {
            let (x1, y1) = room.center();
            for exit in &room.exits {
                match exit.to.and_then(|to| map.room(to)).filter(|t| t.area == area) {
                    Some(target) => {
                        // Draw each two-way pair once.
                        if target.id < room.id && target.exits.iter().any(|e| e.to == Some(room.id)) {
                            continue;
                        }
                        let (x2, y2) = target.center();
                        cr.move_to(x1, y1);
                        cr.line_to(x2, y2);
                    }
                    None => {
                        let (dx, dy) = mu_map::offset(&exit.command).unwrap_or((0.0, 0.0));
                        if dx == 0.0 && dy == 0.0 {
                            continue;
                        }
                        cr.move_to(x1, y1);
                        cr.line_to(x1 + dx * GRID_X * 0.45, y1 + dy * GRID_Y * 0.45);
                    }
                }
            }
        }
        let _ = cr.stroke();

        let font = self.area.pango_context().font_description().unwrap_or_default();
        for room in &rooms {
            let is_current = Some(room.id) == current;
            let is_selected = Some(room.id) == self.selected.get();
            let fill = room.color.as_deref().and_then(rgb).unwrap_or(room_fill);
            rounded(cr, room.x, room.y, room.w, room.h, 6.0);
            if is_current {
                cr.set_source_rgb(0.21, 0.52, 0.89);
            } else {
                cr.set_source_rgb(fill.0, fill.1, fill.2);
            }
            let _ = cr.fill_preserve();
            if is_selected {
                cr.set_source_rgb(0.96, 0.76, 0.07);
                cr.set_line_width(3.0 / z.max(0.5));
            } else {
                cr.set_source_rgb(line.0, line.1, line.2);
                cr.set_line_width(1.0 / z.max(0.5));
            }
            let _ = cr.stroke();

            let layout = pangocairo::functions::create_layout(cr);
            let mut desc = font.clone();
            desc.set_size(9 * pango::SCALE);
            layout.set_font_description(Some(&desc));
            layout.set_text(&room.name);
            layout.set_width(((room.w - 8.0) * pango::SCALE as f64) as i32);
            layout.set_height(((room.h - 4.0) * pango::SCALE as f64) as i32);
            layout.set_ellipsize(pango::EllipsizeMode::End);
            layout.set_wrap(pango::WrapMode::WordChar);
            layout.set_alignment(pango::Alignment::Center);
            let (_, lh) = layout.pixel_size();
            if is_current {
                cr.set_source_rgb(1.0, 1.0, 1.0);
            } else {
                cr.set_source_rgb(fg.0, fg.1, fg.2);
            }
            cr.move_to(room.x + 4.0, room.y + (room.h - lh as f64) / 2.0);
            pangocairo::functions::show_layout(cr, &layout);
        }
        cr.restore().ok();
    }

    fn setup_input(self: &Rc<Self>) {
        // Wheel: zoom around the pointer.
        let scroll = gtk::EventControllerScroll::new(gtk::EventControllerScrollFlags::VERTICAL);
        let motion_pos = Rc::new(Cell::new((0.0, 0.0)));
        let motion = gtk::EventControllerMotion::new();
        {
            let pos = motion_pos.clone();
            motion.connect_motion(move |_, x, y| pos.set((x, y)));
        }
        self.area.add_controller(motion);
        let weak = Rc::downgrade(self);
        scroll.connect_scroll(move |_, _, dy| {
            let Some(this) = weak.upgrade() else { return glib::Propagation::Proceed };
            let (px, py) = motion_pos.get();
            let before = this.to_map(px, py);
            let zoom = (this.zoom.get() * if dy < 0.0 { 1.15 } else { 1.0 / 1.15 }).clamp(0.2, 4.0);
            this.zoom.set(zoom);
            this.origin.set((before.0 - px / zoom, before.1 - py / zoom));
            this.area.queue_draw();
            glib::Propagation::Stop
        });
        self.area.add_controller(scroll);

        // Drag: move a room, or pan when starting on empty space (or with the middle button).
        let drag = gtk::GestureDrag::builder().button(0).build();
        let weak = Rc::downgrade(self);
        drag.connect_drag_begin(move |gesture, x, y| {
            let Some(this) = weak.upgrade() else { return };
            this.area.grab_focus();
            let (mx, my) = this.to_map(x, y);
            let hit = this.engine.borrow().map().room_at(&this.shown_area(), mx, my);
            this.pan_start.set(this.origin.get());
            this.dragging.set(None);
            if gesture.current_button() == gdk::BUTTON_PRIMARY
                && let Some(id) = hit
                && let Some(room) = this.engine.borrow().map().room(id)
            {
                this.dragging.set(Some((id, mx - room.x, my - room.y)));
                this.selected.set(Some(id));
                this.area.queue_draw();
            }
        });
        let weak = Rc::downgrade(self);
        drag.connect_drag_update(move |gesture, dx, dy| {
            let Some(this) = weak.upgrade() else { return };
            let z = this.zoom.get();
            match this.dragging.get() {
                Some((id, grab_x, grab_y)) => {
                    let Some((sx, sy)) = gesture.start_point() else { return };
                    let (mx, my) = this.to_map(sx + dx, sy + dy);
                    let (mut x, mut y) = (mx - grab_x, my - grab_y);
                    // Snap to half grid cells unless Ctrl is held, as in BeipMU.
                    if !gesture.current_event_state().contains(gdk::ModifierType::CONTROL_MASK) {
                        x = (x / (GRID_X / 2.0)).round() * (GRID_X / 2.0);
                        y = (y / (GRID_Y / 2.0)).round() * (GRID_Y / 2.0);
                    }
                    // Its only output is a redraw, which happens here anyway.
                    this.engine.borrow_mut().map_move_room(id, x, y);
                    this.area.queue_draw();
                }
                None => {
                    if gesture.current_button() == gdk::BUTTON_SECONDARY {
                        return;
                    }
                    let (px, py) = this.pan_start.get();
                    this.origin.set((px - dx / z, py - dy / z));
                    this.area.queue_draw();
                }
            }
        });
        let weak = Rc::downgrade(self);
        drag.connect_drag_end(move |_, _, _| {
            if let Some(this) = weak.upgrade() {
                this.dragging.set(None);
            }
        });
        self.area.add_controller(drag);

        // Double-click a room to walk there.
        let click = gtk::GestureClick::builder().button(gdk::BUTTON_PRIMARY).build();
        let weak = Rc::downgrade(self);
        click.connect_pressed(move |_, n, x, y| {
            let Some(this) = weak.upgrade() else { return };
            if n != 2 {
                return;
            }
            let (mx, my) = this.to_map(x, y);
            let hit = this.engine.borrow().map().room_at(&this.shown_area(), mx, my);
            if let Some(room) = hit {
                let out = this.engine.borrow_mut().map_walk_to(room);
                (this.on_outputs)(out);
            }
        });
        self.area.add_controller(click);

        // Right-click menu.
        let menu_click = gtk::GestureClick::builder().button(gdk::BUTTON_SECONDARY).build();
        let weak = Rc::downgrade(self);
        menu_click.connect_pressed(move |_, _, x, y| {
            if let Some(this) = weak.upgrade() {
                this.context_menu(x, y);
            }
        });
        self.area.add_controller(menu_click);
    }

    fn context_menu(self: &Rc<Self>, x: f64, y: f64) {
        let (mx, my) = self.to_map(x, y);
        let area = self.shown_area();
        let hit = self.engine.borrow().map().room_at(&area, mx, my);
        let actions = gio::SimpleActionGroup::new();
        let menu = gio::Menu::new();
        let add = |name: &str, label: &str, f: Box<dyn Fn(&Rc<Self>)>| {
            let action = gio::SimpleAction::new(name, None);
            let weak = Rc::downgrade(self);
            action.connect_activate(move |_, _| {
                if let Some(this) = weak.upgrade() {
                    f(&this);
                }
            });
            actions.add_action(&action);
            menu.append(Some(label), Some(&format!("map.{name}")));
        };
        match hit {
            Some(room) => {
                self.selected.set(Some(room));
                add("walk", "Walk Here", Box::new(move |this| {
                    let out = this.engine.borrow_mut().map_walk_to(room);
                    (this.on_outputs)(out);
                }));
                add("here", "I'm Here", Box::new(move |this| {
                    let out = this.engine.borrow_mut().map_set_current(Some(room));
                    (this.on_outputs)(out);
                }));
                add("rename", "Rename…", Box::new(move |this| this.rename(room)));
                add("delete", "Delete Room", Box::new(move |this| {
                    let out = this.engine.borrow_mut().map_delete_room(room);
                    (this.on_outputs)(out);
                }));
            }
            None => {
                add("add", "Add Room Here", Box::new(move |this| {
                    let name = format!("Room {}", this.engine.borrow().map().len() + 1);
                    let (x, y) = (mx - mu_map::ROOM_W / 2.0, my - mu_map::ROOM_H / 2.0);
                    let (id, out) = this.engine.borrow_mut().map_add_room_at(&name, &area, x, y);
                    this.selected.set(Some(id));
                    (this.on_outputs)(out);
                    this.rename(id);
                }));
            }
        }
        self.area.insert_action_group("map", Some(&actions));
        let popover = gtk::PopoverMenu::from_model(Some(&menu));
        popover.set_parent(&self.area);
        popover.set_has_arrow(false);
        popover.set_pointing_to(Some(&gdk::Rectangle::new(x as i32, y as i32, 1, 1)));
        popover.connect_closed(|p| {
            let p = p.clone();
            glib::idle_add_local_once(move || p.unparent());
        });
        popover.popup();
        self.area.queue_draw();
    }

    fn rename(self: &Rc<Self>, room: RoomId) {
        let current = self.engine.borrow().map().room(room).map(|r| r.name.clone()).unwrap_or_default();
        let entry = gtk::Entry::builder().text(&current).activates_default(true).build();
        let dialog = adw::AlertDialog::new(Some("Rename Room"), None);
        dialog.set_extra_child(Some(&entry));
        dialog.add_response("cancel", "Cancel");
        dialog.add_response("rename", "Rename");
        dialog.set_response_appearance("rename", adw::ResponseAppearance::Suggested);
        dialog.set_default_response(Some("rename"));
        dialog.set_close_response("cancel");
        let weak = Rc::downgrade(self);
        dialog.connect_response(Some("rename"), move |_, _| {
            let Some(this) = weak.upgrade() else { return };
            let name = entry.text().trim().to_string();
            if !name.is_empty() {
                let out = this.engine.borrow_mut().map_rename_room(room, &name);
                (this.on_outputs)(out);
            }
        });
        dialog.present(Some(&self.area));
    }
}
