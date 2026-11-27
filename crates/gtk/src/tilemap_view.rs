//! Shows a Beip.Tilemap map: tiles cut from one image, scaled to fit with sharp pixels.

use std::cell::RefCell;
use std::rc::Rc;

use gtk::gdk::prelude::GdkCairoContextExt;
use gtk::gdk_pixbuf::Pixbuf;
use gtk::glib;
use gtk::prelude::*;
use familiar_engine::tilemap::Tilemap;

pub struct TilemapView {
    pub root: gtk::DrawingArea,
    state: RefCell<State>,
}

#[derive(Default)]
struct State {
    map: Option<Tilemap>,
    tiles: Option<Pixbuf>,
    /// The URL `tiles` came from.
    tiles_url: String,
    /// The whole map at 1:1, redrawn when the map changes.
    rendered: Option<gtk::cairo::ImageSurface>,
}

impl TilemapView {
    pub fn new() -> Rc<Self> {
        let root = gtk::DrawingArea::builder().hexpand(true).vexpand(true).build();
        let this = Rc::new(Self { root, state: RefCell::default() });
        let weak = Rc::downgrade(&this);
        this.root.set_draw_func(move |_, cr, w, h| {
            if let Some(this) = weak.upgrade() {
                this.draw(cr, w as f64, h as f64);
            }
        });
        this
    }

    /// Shows new map data, downloading the tile image if it changed. `notice` reports problems.
    pub fn update(self: &Rc<Self>, map: Tilemap, notice: Rc<dyn Fn(String)>) {
        let url = map.info.tile_url.clone();
        let mut state = self.state.borrow_mut();
        state.map = Some(map);
        if state.tiles_url == url {
            state.rendered = None;
            drop(state);
            self.root.queue_draw();
            return;
        }
        state.tiles_url = url.clone();
        drop(state);
        let fetch_url = url.clone();
        let job = crate::runtime().spawn_blocking(move || familiar_engine::media::fetch(&fetch_url));
        let weak = Rc::downgrade(self);
        glib::spawn_future_local(async move {
            let path = match job.await {
                Ok(Ok(path)) => path,
                Ok(Err(e)) => return notice(format!("Couldn't download the tiles: {e}")),
                Err(e) => return notice(format!("Couldn't download the tiles: {e}")),
            };
            let Some(this) = weak.upgrade() else { return };
            if this.state.borrow().tiles_url != url {
                return;
            }
            match Pixbuf::from_file(&path) {
                Ok(pixbuf) => {
                    let mut state = this.state.borrow_mut();
                    if state.tiles_url != url {
                        return;
                    }
                    state.tiles = Some(pixbuf);
                    state.rendered = None;
                }
                Err(e) => return notice(format!("Couldn't read the tile image: {e}")),
            }
            this.root.queue_draw();
        });
    }

    /// Draws every tile into one surface at the tiles' own size.
    fn render(state: &State) -> Option<gtk::cairo::ImageSurface> {
        let map = state.map.as_ref()?;
        let tiles = state.tiles.as_ref()?;
        let (tw, th) = (map.info.tile_width as i32, map.info.tile_height as i32);
        let (mw, mh) = (map.info.width as i32, map.info.height as i32);
        if tw <= 0 || th <= 0 || mw <= 0 || mh <= 0 {
            return None;
        }
        let surf_w = mw.checked_mul(tw)?;
        let surf_h = mh.checked_mul(th)?;
        if surf_w > 8192 || surf_h > 8192 {
            return None;
        }
        let per_row = (tiles.width() / tw).max(1);
        let surface = gtk::cairo::ImageSurface::create(gtk::cairo::Format::ARgb32, surf_w, surf_h).ok()?;
        let cr = gtk::cairo::Context::new(&surface).ok()?;
        for (i, &tile) in map.tiles.iter().enumerate() {
            let (x, y) = ((i as i32 % mw) * tw, (i as i32 / mw) * th);
            let (sx, sy) = ((tile as i32 % per_row) * tw, (tile as i32 / per_row) * th);
            if sy + th > tiles.height() {
                continue; // The tile image doesn't have this many tiles.
            }
            cr.set_source_pixbuf(tiles, (x - sx) as f64, (y - sy) as f64);
            cr.rectangle(x as f64, y as f64, tw as f64, th as f64);
            let _ = cr.fill();
        }
        drop(cr);
        Some(surface)
    }

    fn draw(&self, cr: &gtk::cairo::Context, width: f64, height: f64) {
        let mut state = self.state.borrow_mut();
        if state.rendered.is_none() {
            state.rendered = Self::render(&state);
        }
        let Some(surface) = state.rendered.as_ref() else { return };
        let (sw, sh) = (surface.width() as f64, surface.height() as f64);
        if sw <= 0.0 || sh <= 0.0 || width <= 0.0 || height <= 0.0 {
            return;
        }
        let scale = (width / sw).min(height / sh);
        cr.translate((width - sw * scale) / 2.0, (height - sh * scale) / 2.0);
        cr.scale(scale, scale);
        if cr.set_source_surface(surface, 0.0, 0.0).is_ok() {
            // Keep pixel art sharp when enlarging; smooth it when shrinking, where nearest
            // neighbour would sparkle.
            cr.source().set_filter(if scale >= 1.0 { gtk::cairo::Filter::Nearest } else { gtk::cairo::Filter::Good });
            let _ = cr.paint();
        }
    }
}
