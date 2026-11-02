//! A bounded artwork queue. Visible covers take priority over catalogue prefetch.
use super::*;
use std::collections::{BTreeSet, VecDeque};

const WORKERS: usize = 4;

#[derive(Clone)]
struct Request {
    product: catalog::Product,
    hero: bool,
}

#[derive(Default)]
pub(super) struct Queue {
    pending: VecDeque<Request>,
    visible: BTreeSet<String>,
    attempted: BTreeSet<String>,
    active: usize,
    generation: u64,
    redraw_pending: bool,
}
impl Queue {
    fn add(&mut self, product: catalog::Product, visible: bool, hero: bool) {
        let id = &product.product_id;
        if let Some(index) = self
            .pending
            .iter()
            .position(|r| r.product.product_id == *id)
        {
            if visible {
                let mut request = self.pending.remove(index).unwrap();
                request.hero |= hero;
                self.pending.push_front(request);
            }
        } else if self.attempted.insert(id.clone()) {
            let request = Request { product, hero };
            if visible {
                self.pending.push_front(request);
            } else {
                self.pending.push_back(request);
            }
        }
    }
}

pub(super) fn visible(ui: &Ui, ids: impl Iterator<Item = String>) {
    ui.artwork.borrow_mut().visible = ids.map(|id| id.to_ascii_uppercase()).collect();
}

pub(super) fn refresh(ui: &Ui) {
    let mut queue = ui.artwork.borrow_mut();
    queue.generation = queue.generation.wrapping_add(1);
    queue.pending.clear();
    queue.attempted.clear();
}

pub(super) fn request(ui: &Ui, ids: Vec<String>, visible: bool) {
    {
        let model = ui.model.borrow();
        let mut queue = ui.artwork.borrow_mut();
        // Reversing a priority insertion keeps the visible page's order.
        for (index, id) in ids.iter().enumerate().rev() {
            let key = id.to_ascii_uppercase();
            let Some(product) = model.catalog.get(&key) else {
                continue;
            };
            if product.artwork.portrait.is_none() && product.image.is_none() {
                continue;
            }
            if model.icons.contains_key(&key) {
                continue;
            }
            queue.add(product.clone(), visible, visible && index < 3);
        }
    }
    kick(ui);
}

fn kick(ui: &Ui) {
    let Some(cache) = ui.model.borrow().catalog_cache() else {
        return;
    };
    loop {
        let (request, generation) = {
            let mut queue = ui.artwork.borrow_mut();
            if queue.active >= WORKERS || *ui.closing.borrow() {
                return;
            }
            let Some(request) = queue.pending.pop_front() else {
                return;
            };
            queue.active += 1;
            (request, queue.generation)
        };
        let ui = ui.clone();
        let cache = cache.clone();
        glib::spawn_future_local(async move {
            // Publish each cover as soon as it arrives; a slow image never
            // holds the rest of the page behind an entire serial batch.
            let key = request.product.product_id.to_ascii_uppercase();
            let hero = request
                .hero
                .then(|| (cache.clone(), request.product.clone()));
            let result =
                gio::spawn_blocking(move || catalog::cover(&cache, &request.product)).await;
            let current = ui.artwork.borrow().generation == generation;
            if current {
                if let Ok(Ok(path)) = result {
                    let visible = ui.artwork.borrow().visible.contains(&key);
                    ui.model.borrow_mut().icons.insert(key, path);
                    if visible {
                        schedule_redraw(&ui);
                    }
                }
                if let Some((cache, product)) = hero {
                    let fetched =
                        gio::spawn_blocking(move || catalog::hero(&cache, &product)).await;
                    if matches!(fetched, Ok(Ok(_))) && ui.artwork.borrow().generation == generation
                    {
                        schedule_redraw(&ui);
                    }
                }
            }
            ui.artwork.borrow_mut().active -= 1;
            kick(&ui);
        });
    }
}

fn schedule_redraw(ui: &Ui) {
    if ui.artwork.borrow_mut().redraw_pending {
        return;
    }
    ui.artwork.borrow_mut().redraw_pending = true;
    let ui = ui.clone();
    glib::timeout_add_local_once(std::time::Duration::from_millis(150), move || {
        ui.artwork.borrow_mut().redraw_pending = false;
        if ui.window.is_visible() && !*ui.closing.borrow() {
            render(&ui);
        }
    });
}

#[cfg(test)]
mod tests {
    use super::*;
    fn product(id: &str) -> catalog::Product {
        serde_json::from_value(serde_json::json!({
            "product_id": id, "name": id, "image": "https://example.test/cover"
        }))
        .unwrap()
    }
    #[test]
    fn paging_promotes_pending_covers_without_duplicate_requests() {
        let mut queue = Queue::default();
        for id in ["9ZZART000001", "9ZZART000002", "9ZZART000003"] {
            queue.add(product(id), false, false);
        }
        queue.add(product("9ZZART000003"), true, true);
        assert_eq!(queue.pending.len(), 3);
        let first = queue.pending.pop_front().unwrap();
        assert_eq!(first.product.product_id, "9ZZART000003");
        assert!(first.hero);
        queue.add(product("9ZZART000003"), true, true);
        assert_eq!(
            queue.pending.len(),
            2,
            "in-flight/attempted work is deduplicated"
        );
    }
}
