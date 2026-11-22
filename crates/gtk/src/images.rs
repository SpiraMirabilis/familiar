//! Downloads images (avatars, inline images) off the UI thread and shares them by URL.

use std::cell::RefCell;
use std::collections::{HashMap, HashSet};
use std::rc::Rc;

use gtk::{gdk, glib};

type Waiting = Vec<Box<dyn FnOnce(&gdk::Texture)>>;

#[derive(Default)]
pub struct ImageCache {
    loaded: RefCell<HashMap<String, gdk::Texture>>,
    /// Downloads in progress, with what to do when each finishes.
    pending: RefCell<HashMap<String, Waiting>>,
    /// URLs that failed, so they aren't fetched again every time.
    failed: RefCell<HashSet<String>>,
}

impl ImageCache {
    /// Calls `ready` with the image, now if it's cached or later once downloaded. Images that
    /// can't be fetched or decoded are skipped quietly: a broken avatar shouldn't fill the
    /// output with errors.
    pub fn get(self: &Rc<Self>, url: &str, ready: impl FnOnce(&gdk::Texture) + 'static) {
        if let Some(texture) = self.loaded.borrow().get(url) {
            return ready(texture);
        }
        if self.failed.borrow().contains(url) {
            return;
        }
        let mut pending = self.pending.borrow_mut();
        if let Some(waiting) = pending.get_mut(url) {
            waiting.push(Box::new(ready));
            return;
        }
        pending.insert(url.to_string(), vec![Box::new(ready)]);
        drop(pending);

        let fetch_url = url.to_string();
        let job = crate::runtime().spawn_blocking(move || familiar_engine::media::fetch(&fetch_url));
        let this = Rc::downgrade(self);
        let url = url.to_string();
        glib::spawn_future_local(async move {
            let texture = match job.await {
                Ok(Ok(path)) => gdk::Texture::from_filename(&path).ok(),
                _ => None,
            };
            let Some(this) = this.upgrade() else { return };
            let waiting = this.pending.borrow_mut().remove(&url).unwrap_or_default();
            match texture {
                Some(texture) => {
                    this.loaded.borrow_mut().insert(url, texture.clone());
                    for ready in waiting {
                        ready(&texture);
                    }
                }
                None => {
                    this.failed.borrow_mut().insert(url);
                }
            }
        });
    }
}
