//! Artwork textures: scaled to the size they are drawn at, cached, and decoded
//! off the GTK thread.
//!
//! A cover on disk is 600x900 RGBA, about 2 MB once decoded. Decoding it on
//! the main thread for every card on every redraw was most of what a page
//! switch cost: about 4.5 ms a cover, two decodes a card. Here a picture asks
//! for a path at a [`Size`]. A cached texture is set at once; otherwise the
//! picture shows the `art-loading` placeholder while a worker decodes and
//! scales the file, and the texture is set when it lands.
//!
//! Entries are keyed by path and size and checked against the file's
//! modification time and length, so a cover the artwork worker rewrites is
//! decoded again. The cache is bounded by bytes and evicts the least recently
//! used entry first.
use super::*;
use std::collections::{HashMap, VecDeque};
use std::time::SystemTime;

/// What a picture draws at. Each is twice the logical size it is shown at,
/// so a 2x display still gets a sharp image.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub(crate) enum Size {
    /// A portrait cover on a card.
    Cover,
    /// A small square icon in a list row.
    Icon,
    /// A landscape banner.
    Hero,
    /// The account picture.
    Avatar,
}

impl Size {
    /// The box the decoded image must cover, in pixels.
    fn bounds(self) -> (u32, u32) {
        match self {
            Size::Cover => (392, 588),
            Size::Icon => (96, 96),
            Size::Hero => (1280, 720),
            Size::Avatar => (128, 128),
        }
    }

    /// The narrowest source this size accepts. A card's cover must be a real
    /// cover: a catalogue's small square tile stretched over a portrait card is
    /// worse than the placeholder.
    fn min_source_width(self) -> u32 {
        match self {
            Size::Cover => 300,
            _ => 0,
        }
    }
}

/// Scale `source` so it covers `bounds` without being enlarged: the size a
/// `ContentFit::Cover` picture needs, and no more.
fn cover_scale(source: (u32, u32), bounds: (u32, u32)) -> (u32, u32) {
    let (w, h) = (source.0.max(1) as f64, source.1.max(1) as f64);
    let scale = (bounds.0 as f64 / w).max(bounds.1 as f64 / h).min(1.0);
    (
        ((w * scale).round() as u32).max(1),
        ((h * scale).round() as u32).max(1),
    )
}

/// What a cached texture was decoded from. A different modification time or
/// length means the file was rewritten.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
struct Stamp {
    modified: Option<SystemTime>,
    len: u64,
}

fn stamp(path: &Path) -> Option<Stamp> {
    let meta = std::fs::metadata(path).ok()?;
    Some(Stamp {
        modified: meta.modified().ok(),
        len: meta.len(),
    })
}

type Key = (PathBuf, Size);

/// Byte-bounded least-recently-used map. Generic over the value so the
/// eviction rules are tested without a display.
struct Lru<V> {
    entries: HashMap<Key, (Stamp, V, usize, u64)>,
    bytes: usize,
    limit: usize,
    tick: u64,
}

impl<V: Clone> Lru<V> {
    fn new(limit: usize) -> Self {
        Self {
            entries: HashMap::new(),
            bytes: 0,
            limit,
            tick: 0,
        }
    }

    /// The value for `key` if it was made from the file as it is now.
    fn get(&mut self, key: &Key, current: Stamp) -> Option<V> {
        self.tick += 1;
        let tick = self.tick;
        match self.entries.get_mut(key) {
            Some((stamp, value, _, used)) if *stamp == current => {
                *used = tick;
                Some(value.clone())
            }
            Some(_) => {
                self.remove(key);
                None
            }
            None => None,
        }
    }

    fn remove(&mut self, key: &Key) {
        if let Some((_, _, size, _)) = self.entries.remove(key) {
            self.bytes -= size;
        }
    }

    fn insert(&mut self, key: Key, stamp: Stamp, value: V, size: usize) {
        self.remove(&key);
        self.tick += 1;
        self.bytes += size;
        self.entries.insert(key, (stamp, value, size, self.tick));
        // Never evict what was just inserted: one entry larger than the limit
        // is still drawn, and goes first next time.
        while self.bytes > self.limit && self.entries.len() > 1 {
            let oldest = self
                .entries
                .iter()
                .filter(|(_, (_, _, _, used))| *used != self.tick)
                .min_by_key(|(_, (_, _, _, used))| *used)
                .map(|(key, _)| key.clone());
            match oldest {
                Some(key) => self.remove(&key),
                None => break,
            }
        }
    }
}

/// 128 MiB: about 140 covers, several pages of any grid.
const CACHE_BYTES: usize = 128 << 20;
/// Decodes at once. Each is a whole PNG inflate, so more than this only
/// competes with the artwork downloads and the rest of the desktop.
const WORKERS: usize = 4;

/// Somewhere to put a texture when it arrives. Held weakly: a page torn
/// down before its covers decoded does not keep them alive, and a decode that
/// nobody is waiting for any more is skipped.
enum Target {
    Picture(glib::WeakRef<gtk::Picture>),
    Image(glib::WeakRef<gtk::Image>),
    Avatar(glib::WeakRef<adw::Avatar>),
}

impl Target {
    fn alive(&self) -> bool {
        match self {
            Target::Picture(w) => w.upgrade().is_some(),
            Target::Image(w) => w.upgrade().is_some(),
            Target::Avatar(w) => w.upgrade().is_some(),
        }
    }

    fn deliver(&self, texture: Option<&gdk::Texture>) {
        match self {
            Target::Picture(w) => {
                if let Some(picture) = w.upgrade() {
                    if let Some(texture) = texture {
                        picture.set_paintable(Some(texture));
                    }
                    picture.remove_css_class("art-loading");
                }
            }
            Target::Image(w) => {
                if let (Some(image), Some(texture)) = (w.upgrade(), texture) {
                    image.set_paintable(Some(texture));
                }
            }
            Target::Avatar(w) => {
                if let (Some(avatar), Some(texture)) = (w.upgrade(), texture) {
                    avatar.set_custom_image(Some(texture));
                }
            }
        }
    }
}

struct Cache {
    /// `None` is a file that could not be used: unreadable, or too small for
    /// the size asked for. Remembered so it is not decoded on every redraw.
    textures: Lru<Option<gdk::Texture>>,
    waiting: HashMap<Key, Vec<Target>>,
    queue: VecDeque<Key>,
    active: usize,
}

thread_local! {
    static CACHE: RefCell<Cache> = RefCell::new(Cache {
        textures: Lru::new(CACHE_BYTES),
        waiting: HashMap::new(),
        queue: VecDeque::new(),
        active: 0,
    });
}

/// Show `path` in `picture`: now if it is cached, otherwise after a worker
/// has decoded it. Until then the picture carries `art-loading`.
pub(crate) fn set_picture(picture: &gtk::Picture, path: &Path, size: Size) {
    request(path, size, Target::Picture(picture.downgrade()), || {
        picture.add_css_class("art-loading")
    });
}

/// Show `path` in `image`, keeping its icon until the texture is ready.
pub(crate) fn set_image(image: &gtk::Image, path: &Path, size: Size) {
    request(path, size, Target::Image(image.downgrade()), || {});
}

/// Use `path` as `avatar`'s picture, keeping its initials until it is ready.
pub(crate) fn set_avatar(avatar: &adw::Avatar, path: &Path, size: Size) {
    request(path, size, Target::Avatar(avatar.downgrade()), || {});
}

fn request(path: &Path, size: Size, target: Target, pending: impl FnOnce()) {
    let Some(stamp) = stamp(path) else {
        return;
    };
    let key = (path.to_path_buf(), size);
    let hit = CACHE.with(|c| c.borrow_mut().textures.get(&key, stamp));
    if let Some(texture) = hit {
        target.deliver(texture.as_ref());
        return;
    }
    pending();
    let start = CACHE.with(|c| {
        let mut cache = c.borrow_mut();
        let waiters = cache.waiting.entry(key.clone()).or_default();
        let first = waiters.is_empty();
        waiters.push(target);
        if first {
            cache.queue.push_back(key);
        }
        first
    });
    if start {
        pump();
    }
}

/// Start decodes up to [`WORKERS`], skipping any whose pictures are gone.
fn pump() {
    loop {
        let next = CACHE.with(|c| {
            let mut cache = c.borrow_mut();
            while cache.active < WORKERS {
                let key = cache.queue.pop_front()?;
                let alive = cache
                    .waiting
                    .get_mut(&key)
                    .map(|waiters| {
                        waiters.retain(Target::alive);
                        !waiters.is_empty()
                    })
                    .unwrap_or(false);
                if alive {
                    cache.active += 1;
                    return Some(key);
                }
                cache.waiting.remove(&key);
            }
            None
        });
        let Some(key) = next else { return };
        glib::spawn_future_local(async move {
            let (path, size) = key.clone();
            let decoded = gio::spawn_blocking(move || decode(&path, size))
                .await
                .ok()
                .flatten();
            let waiters = CACHE.with(|c| {
                let mut cache = c.borrow_mut();
                cache.active -= 1;
                if let Some((stamp, texture)) = &decoded {
                    let bytes = texture
                        .as_ref()
                        .map_or(0, |t| (t.width() * t.height() * 4) as usize);
                    cache
                        .textures
                        .insert(key.clone(), *stamp, texture.clone(), bytes);
                }
                cache.waiting.remove(&key).unwrap_or_default()
            });
            let texture = decoded.and_then(|(_, texture)| texture);
            for waiter in waiters {
                waiter.deliver(texture.as_ref());
            }
            pump();
        });
    }
}

/// Runs on a worker thread. `gdk::Texture` is `Send`, so the texture is built
/// here and only handed to the main loop. `None` is a file that could not be
/// read at all; `Some((stamp, None))` one that is not usable at this size.
fn decode(path: &Path, size: Size) -> Option<(Stamp, Option<gdk::Texture>)> {
    use gtk::gdk_pixbuf::Pixbuf;
    let stamp = stamp(path)?;
    let Some((_, width, height)) = Pixbuf::file_info(path) else {
        return Some((stamp, None));
    };
    let (width, height) = (width.max(0) as u32, height.max(0) as u32);
    if width < size.min_source_width() {
        return Some((stamp, None));
    }
    let (w, h) = cover_scale((width, height), size.bounds());
    let pixbuf = if (w, h) == (width, height) {
        Pixbuf::from_file(path)
    } else {
        Pixbuf::from_file_at_scale(path, w as i32, h as i32, false)
    };
    let Ok(pixbuf) = pixbuf else {
        return Some((stamp, None));
    };
    let format = if pixbuf.has_alpha() {
        gdk::MemoryFormat::R8g8b8a8
    } else {
        gdk::MemoryFormat::R8g8b8
    };
    let texture = gdk::MemoryTexture::new(
        pixbuf.width(),
        pixbuf.height(),
        format,
        &pixbuf.read_pixel_bytes(),
        pixbuf.rowstride() as usize,
    );
    Some((stamp, Some(texture.upcast())))
}

/// A picture or icon showing a title's art, held weakly.
enum Slot {
    Picture(glib::WeakRef<gtk::Picture>, Size),
    Image(glib::WeakRef<gtk::Image>, Size),
}

impl Slot {
    fn alive(&self) -> bool {
        match self {
            Slot::Picture(w, _) => w.upgrade().is_some(),
            Slot::Image(w, _) => w.upgrade().is_some(),
        }
    }
}

/// Where each title's art is on screen, so art that arrives later is put
/// straight into those widgets. Keys are the upper-case product id for a
/// cover or icon, and `hero-<ID>` for a banner.
#[derive(Default)]
pub(crate) struct Slots(HashMap<String, Vec<Slot>>);

impl Slots {
    pub(crate) fn add_picture(&mut self, key: String, picture: &gtk::Picture, size: Size) {
        self.0
            .entry(key)
            .or_default()
            .push(Slot::Picture(picture.downgrade(), size));
    }

    pub(crate) fn add_image(&mut self, key: String, image: &gtk::Image, size: Size) {
        self.0
            .entry(key)
            .or_default()
            .push(Slot::Image(image.downgrade(), size));
    }

    /// Forget widgets that were destroyed.
    pub(crate) fn prune(&mut self) {
        self.0.retain(|_, slots| {
            slots.retain(Slot::alive);
            !slots.is_empty()
        });
    }

    /// Show `path` in every live widget for `key`. False if there is none,
    /// so the caller knows whether a redraw is still needed.
    pub(crate) fn fill(&mut self, key: &str, path: &Path) -> bool {
        let Some(slots) = self.0.get_mut(key) else {
            return false;
        };
        slots.retain(Slot::alive);
        let targets: Vec<_> = slots
            .iter()
            .filter_map(|slot| match slot {
                Slot::Picture(w, size) => w.upgrade().map(|p| (Some(p), None, *size)),
                Slot::Image(w, size) => w.upgrade().map(|i| (None, Some(i), *size)),
            })
            .collect();
        for (picture, image, size) in &targets {
            if let Some(picture) = picture {
                set_picture(picture, path, *size);
            }
            if let Some(image) = image {
                set_image(image, path, *size);
            }
        }
        !targets.is_empty()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn key(name: &str) -> Key {
        (PathBuf::from(name), Size::Cover)
    }

    fn at(len: u64) -> Stamp {
        Stamp {
            modified: Some(SystemTime::UNIX_EPOCH + std::time::Duration::from_secs(len)),
            len,
        }
    }

    #[test]
    fn evicts_least_recently_used_by_bytes() {
        let mut lru = Lru::new(300);
        lru.insert(key("a"), at(1), 'a', 100);
        lru.insert(key("b"), at(1), 'b', 100);
        lru.insert(key("c"), at(1), 'c', 100);
        // Touch "a", so "b" is now the oldest.
        assert_eq!(lru.get(&key("a"), at(1)), Some('a'));
        lru.insert(key("d"), at(1), 'd', 100);
        assert_eq!(lru.bytes, 300);
        assert_eq!(lru.get(&key("b"), at(1)), None);
        for name in ["a", "c", "d"] {
            assert!(lru.get(&key(name), at(1)).is_some(), "{name} kept");
        }
        // One large entry pushes out as many as it needs to, but stays itself.
        lru.insert(key("big"), at(1), 'B', 1000);
        assert_eq!(lru.entries.len(), 1);
        assert_eq!(lru.get(&key("big"), at(1)), Some('B'));
    }

    #[test]
    fn a_rewritten_file_is_a_miss() {
        let mut lru = Lru::new(1000);
        lru.insert(key("a"), at(1), 'a', 100);
        assert_eq!(
            lru.get(&key("a"), at(2)),
            None,
            "different mtime and length"
        );
        assert_eq!(lru.bytes, 0, "the stale entry is dropped");
        lru.insert(key("a"), at(2), 'A', 100);
        assert_eq!(lru.get(&key("a"), at(2)), Some('A'));
        // The same path at another size is another entry.
        assert_eq!(lru.get(&(PathBuf::from("a"), Size::Icon), at(2)), None);
    }

    #[test]
    fn scales_to_cover_without_enlarging() {
        assert_eq!(cover_scale((600, 900), (392, 588)), (392, 588));
        // Landscape art still covers a portrait box: height decides.
        assert_eq!(cover_scale((1920, 1080), (392, 588)), (1045, 588));
        // Never enlarged.
        assert_eq!(cover_scale((200, 300), (392, 588)), (200, 300));
        assert_eq!(cover_scale((960, 540), Size::Hero.bounds()), (960, 540));
    }
}
