//! Host mode: a page that frames VectorCraft (`?host=parent`) or runs it in its own document
//! (`data-host="page"` on the canvas) opens documents in it and stores what it saves instead of
//! the browser downloading. The messages and both transports are `craft-web-host`'s, the same
//! contract as the other Craft apps' bridges; this module is VectorCraft's part of it.
//!
//! - `vectorcraft:ready` once the app is running and listening. VectorCraft reads whole files, so
//!   it says `file: false` and a host sends `bytes`.
//! - `vectorcraft:open` `{ name, bytes }` opens as the host's file: a document gets its name as
//!   its path, so File › Save (and autosave, `crate::autosave`) writes it back to the host with no
//!   dialog ([`open_files`]). Anything else File › Open takes (plug-ins, presets, libraries…)
//!   goes into the page's open inbox, where `window.vectorcraft.openFile` puts its files, and
//!   opens as they do.
//! - `vectorcraft:save` `{ name, bytes, saveAs }` for every file the app would download: Save and
//!   exports with `saveAs: false` (an export under its own extension); Save As…, Save a Copy… and
//!   Save as Template… with `true`.
//! - `vectorcraft:dirty` whenever unsaved changes appear or go away, and `vectorcraft:failed` when
//!   the app couldn't start or lost its graphics for good.

use std::cell::RefCell;
use std::rc::Rc;

use std::sync::PoisonError;

use craft_web_host::web::{FileData, Host, Message};
use vectorcraft_ui_egui::{VectorcraftApp, io};

use crate::web::Inbox;

/// The app id in the host messages (`vectorcraft:ready`).
const APP: &str = "vectorcraft";

#[derive(Clone)]
pub struct Bridge {
    host: Rc<Host>,
    /// The running app's context, woken when a file arrives. A graphics restart makes a new one.
    ctx: Rc<RefCell<Option<egui::Context>>>,
}

/// The bridge for this page, or `None` when VectorCraft isn't hosted (no `?host=parent` or
/// `data-host="page"`, no framing parent, or a parent the page doesn't allow): then saves stay
/// browser downloads. Files the host sends go to `inbox` from now on; the app drains it once it
/// runs, and the host sends none before `ready` anyway.
pub fn connect(canvas: &web_sys::Element, inbox: &Inbox) -> Option<Bridge> {
    let bridge = Bridge { host: Rc::new(Host::connect(APP, canvas)?), ctx: Rc::default() };
    let (inbox, ctx) = (inbox.clone(), bridge.ctx.clone());
    bridge.host.listen(move |message| {
        if let Message::Open { name, data } = message {
            deliver_open(&inbox, ctx.borrow().as_ref(), name, &data);
        }
    });
    Some(bridge)
}

impl Bridge {
    /// The context to wake for a file: a message is no input event, so egui would not paint (and
    /// drain the inbox) before the next one.
    pub fn wake(&self, ctx: &egui::Context) {
        *self.ctx.borrow_mut() = Some(ctx.clone());
    }

    pub fn ready(&self) {
        let _ = self.host.post_ready(env!("CARGO_PKG_VERSION"), false);
    }

    /// The app couldn't start or lost its graphics for good: let the host say so.
    pub fn failed(&self, error: &str) {
        let _ = self.host.post_failed(error, None);
    }

    /// Hand the host a file the app would otherwise download. The host names it by the last
    /// component of `name`, so an export keeps its own extension.
    pub fn save(&self, name: &str, bytes: &[u8], save_as: bool) -> Result<(), String> {
        self.host.post_save(name, bytes, None, save_as)
    }

    /// Report unsaved changes; the crate sends only changes of state, so this can run every frame.
    pub fn dirty(&self, dirty: bool) {
        self.host.post_dirty(dirty);
    }
}

/// Open the files the host sent (`inbox`, [`connect`]'s). A document opens with the host's name
/// for it as its path, so a Save writes it back as that file without asking for a name and
/// format; anything else File › Open takes goes into `page`, the page script's inbox, and opens
/// as it always did.
pub fn open_files(app: &mut VectorcraftApp, inbox: &Inbox, page: &Inbox) {
    let files = std::mem::take(&mut *inbox.lock().unwrap_or_else(PoisonError::into_inner));
    for (name, bytes) in files {
        if io::opens_as(&name).is_some() {
            page.lock().unwrap_or_else(PoisonError::into_inner).push((name, bytes));
        } else if let Err(e) = io::open_bytes(app, &name, &bytes, Some(name.clone())) {
            io::report_open_error(app, &name, &e);
        }
    }
}

fn deliver_open(inbox: &Inbox, ctx: Option<&egui::Context>, name: String, data: &FileData) {
    let FileData::Bytes(array) = data else {
        log::warn!("host file {name} came as a File; VectorCraft reads bytes only; not opened");
        return;
    };
    inbox.lock().unwrap_or_else(PoisonError::into_inner).push((name, array.to_vec()));
    if let Some(ctx) = ctx {
        ctx.request_repaint();
    }
}
