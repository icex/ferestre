//! What plays over the window when a game starts: its art lifts off the tile
//! it was started from, spins twice like a card on its way to the middle of the
//! dimmed window, is crossed by a band of light, and rushes forward as it fades. It only draws: input
//! goes through it, and the launch does not wait for it.
use super::*;

const LENGTH_MS: u32 = 2800;
/// When the art has arrived, and when it starts to leave, of the whole.
const ARRIVED: f64 = 0.48;
const LEAVES: f64 = 0.84;
/// How often the art turns on its way.
const TURNS: f64 = 2.0;
/// The window's class while the content behind is blurred.
const LAUNCHING: &str = "launching";

mod imp {
    use super::*;
    use gtk::subclass::prelude::*;

    #[derive(Default)]
    pub struct Show {
        pub art: RefCell<Option<gdk::Paintable>>,
        pub title: RefCell<String>,
        /// Where the art starts, in this widget.
        pub from: Cell<Option<(f32, f32, f32, f32)>>,
        pub at: Cell<f64>,
    }

    #[glib::object_subclass]
    impl ObjectSubclass for Show {
        const NAME: &'static str = "FerestreLaunchShow";
        type Type = super::Show;
        type ParentType = gtk::Widget;
    }

    impl ObjectImpl for Show {}

    impl WidgetImpl for Show {
        fn snapshot(&self, snapshot: &gtk::Snapshot) {
            self.obj().draw(snapshot);
        }
    }
}

glib::wrapper! {
    pub struct Show(ObjectSubclass<imp::Show>)
        @extends gtk::Widget, @implements gtk::Accessible, gtk::Buildable, gtk::ConstraintTarget;
}

impl Show {
    pub(crate) fn new() -> Show {
        let show: Show = glib::Object::new();
        show.set_can_target(false);
        show.set_can_focus(false);
        show.set_visible(false);
        show.set_accessible_role(gtk::AccessibleRole::Presentation);
        show.set_overflow(gtk::Overflow::Hidden);
        show
    }

    fn draw(&self, snapshot: &gtk::Snapshot) {
        use gtk::graphene::{Point, Rect};
        use gtk::subclass::prelude::ObjectSubclassIsExt;
        let imp = self.imp();
        let Some(art) = imp.art.borrow().clone() else {
            return;
        };
        let (w, h) = (self.width() as f32, self.height() as f32);
        let at = imp.at.get();
        let (arriving, leaving) = phases(at);
        let shown = (arriving * (1.0 - leaving)) as f32;
        let all = Rect::new(0.0, 0.0, w, h);
        snapshot.append_color(&gdk::RGBA::new(0.02, 0.03, 0.04, 0.80 * shown), &all);

        // The art itself, dimmed, drifting closer behind.
        let aspect = art.intrinsic_aspect_ratio();
        let aspect = if aspect > 0.0 { aspect } else { 2.0 / 3.0 };
        let middle = middle(w, h, aspect);
        let from = imp
            .from
            .get()
            .unwrap_or((middle.0, middle.1 + h * 0.08, middle.2, middle.3));
        let mix = |a: f32, b: f32| a + (b - a) * arriving as f32;
        let (mut x, mut y, mut cw, mut ch) = (
            mix(from.0, middle.0),
            mix(from.1, middle.1),
            mix(from.2, middle.2),
            mix(from.3, middle.3),
        );
        // A slow push in while it stands, then forward and gone.
        let scale = 1.0 + 0.04 * at as f32 + 0.55 * (leaving * leaving) as f32;
        x -= cw * (scale - 1.0) / 2.0;
        y -= ch * (scale - 1.0) / 2.0;
        cw *= scale;
        ch *= scale;
        let rect = Rect::new(x, y, cw, ch);
        let rounded = gtk::gsk::RoundedRect::from_rect(rect, 18.0);
        // It turns about its upright middle, in perspective.
        let turn = arriving * TURNS * std::f64::consts::TAU;
        let facing = turn.cos() as f32;
        let centre = Point::new(x + cw / 2.0, y + ch / 2.0);
        snapshot.save();
        snapshot.translate(&centre);
        snapshot.perspective(cw.max(ch) * 3.0);
        snapshot.rotate_3d(
            turn.to_degrees() as f32,
            &gtk::graphene::Vec3::new(0.0, 1.0, 0.0),
        );
        // The back shows the art too, the right way round.
        if facing < 0.0 {
            snapshot.scale(-1.0, 1.0);
        }
        snapshot.translate(&Point::new(-centre.x(), -centre.y()));
        let pulse = (0.65 + 0.35 * (at * std::f64::consts::TAU * 2.0).sin()) as f32;
        snapshot.push_opacity(f64::from(1.0 - leaving as f32));
        snapshot.append_outset_shadow(
            &rounded,
            &gdk::RGBA::new(0.0, 0.0, 0.0, 0.7 * shown),
            0.0,
            18.0,
            2.0,
            48.0,
        );
        snapshot.append_outset_shadow(
            &rounded,
            &gdk::RGBA::new(0.545, 0.843, 0.482, 0.55 * shown * pulse),
            0.0,
            0.0,
            2.0,
            34.0,
        );
        snapshot.push_rounded_clip(&rounded);
        // Covers the rectangle, whatever its shape on the way.
        let aspect = aspect as f32;
        let (aw, ah) = if cw / ch > aspect {
            (cw, cw / aspect)
        } else {
            (ch * aspect, ch)
        };
        snapshot.save();
        snapshot.translate(&Point::new(x + (cw - aw) / 2.0, y + (ch - ah) / 2.0));
        art.snapshot(snapshot, f64::from(aw), f64::from(ah));
        snapshot.restore();
        // Light sweeps over it twice.
        for start in [0.46, 0.64] {
            let sweep = ((at - start) / 0.2) as f32;
            if (0.0..=1.0).contains(&sweep) {
                let centre = x - cw * 0.5 + sweep * cw * 2.0;
                let half = cw * 0.35;
                let clear = gdk::RGBA::new(1.0, 1.0, 1.0, 0.0);
                snapshot.append_linear_gradient(
                    &rect,
                    &Point::new(centre - half, y),
                    &Point::new(centre + half, y + ch * 0.25),
                    &[
                        gtk::gsk::ColorStop::new(0.0, clear),
                        gtk::gsk::ColorStop::new(0.5, gdk::RGBA::new(1.0, 1.0, 1.0, 0.42)),
                        gtk::gsk::ColorStop::new(1.0, clear),
                    ],
                );
            }
        }
        // Darker the more it is turned away.
        snapshot.append_color(
            &gdk::RGBA::new(0.0, 0.0, 0.0, 0.5 * (1.0 - facing.abs())),
            &rect,
        );
        snapshot.pop();
        snapshot.pop();
        snapshot.restore();

        // The name rises into place under it.
        let words = (((at - 0.40) / 0.12).clamp(0.0, 1.0) * (1.0 - leaving)) as f32;
        if words > 0.0 {
            let line = |text: &str, size: i32, bold: bool, top: f32, alpha: f32| {
                let layout = self.create_pango_layout(Some(text));
                let mut font = layout.context().font_description().unwrap_or_default();
                font.set_absolute_size(f64::from(size * gtk::pango::SCALE));
                font.set_weight(if bold {
                    gtk::pango::Weight::Heavy
                } else {
                    gtk::pango::Weight::Medium
                });
                layout.set_font_description(Some(&font));
                layout.set_width((w * 0.8) as i32 * gtk::pango::SCALE);
                layout.set_alignment(gtk::pango::Alignment::Center);
                layout.set_ellipsize(gtk::pango::EllipsizeMode::End);
                snapshot.save();
                snapshot.translate(&Point::new(w * 0.1, top + 14.0 * (1.0 - words)));
                snapshot.append_layout(&layout, &gdk::RGBA::new(1.0, 1.0, 1.0, alpha * words));
                snapshot.restore();
            };
            let under = middle.1 + middle.3 * 1.04 + 30.0;
            line(&imp.title.borrow(), 34, true, under, 1.0);
            line("Starting…", 17, false, under + 50.0, 0.75);
        }
    }
}

/// How far the art has arrived, and how far it has left.
fn phases(at: f64) -> (f64, f64) {
    let part = (at / ARRIVED).clamp(0.0, 1.0);
    let arriving = part * part * (3.0 - 2.0 * part);
    let leaving = ((at - LEAVES) / (1.0 - LEAVES)).clamp(0.0, 1.0);
    (arriving, leaving * leaving)
}

/// Where the art stands in a window this large, for its shape.
fn middle(w: f32, h: f32, aspect: f64) -> (f32, f32, f32, f32) {
    let aspect = aspect.clamp(0.5, 2.4) as f32;
    let height = (h * 0.52).min(480.0);
    let width = (height * aspect).min(w * 0.7);
    let height = width / aspect;
    ((w - width) / 2.0, (h - height) / 2.0 - 46.0, width, height)
}

/// The picture a title's tile shows, the one that has the focus first.
fn tile_picture(ui: &Ui, product_id: &str) -> Option<gtk::Picture> {
    fn picture(root: &gtk::Widget) -> Option<gtk::Picture> {
        if let Some(picture) = root.downcast_ref::<gtk::Picture>() {
            return picture.paintable().map(|_| picture.clone());
        }
        let mut child = root.first_child();
        while let Some(widget) = child {
            if let Some(found) = picture(&widget) {
                return Some(found);
            }
            child = widget.next_sibling();
        }
        None
    }
    let names = [
        format!("feature-{product_id}-primary"),
        format!("game-{product_id}-primary"),
    ];
    let focus = GtkWindowExt::focus(&ui.window).filter(|f| {
        names
            .iter()
            .any(|n| f.widget_name().eq_ignore_ascii_case(n))
    });
    focus
        .into_iter()
        .chain(
            names
                .iter()
                .filter_map(|n| named_widget(ui.content.upcast_ref(), n)),
        )
        .filter(|w| w.is_mapped())
        .find_map(|w| picture(&w))
}

/// Play it for a title that is being started. Nothing without animations or
/// without art to show.
pub(crate) fn play(ui: &Ui, product_id: &str, name: &str) {
    use gtk::subclass::prelude::ObjectSubclassIsExt;
    let show = &ui.launch_show;
    if !gtk::Settings::default().is_none_or(|s| s.is_gtk_enable_animations())
        || !ui.window.is_mapped()
    {
        return;
    }
    let picture = tile_picture(ui, product_id);
    let art = picture.as_ref().and_then(|p| p.paintable()).or_else(|| {
        let path = ui
            .model
            .borrow()
            .icons
            .get(&product_id.to_ascii_uppercase())
            .cloned()?;
        gdk::Texture::from_filename(path).ok().map(|t| t.upcast())
    });
    let Some(art) = art else { return };
    let from = picture
        .and_then(|p| p.compute_bounds(&ui.window.content()?))
        .map(|b| (b.x(), b.y(), b.width(), b.height()));
    let imp = show.imp();
    imp.art.replace(Some(art));
    imp.title.replace(name.to_string());
    imp.from.set(from);
    imp.at.set(0.0);
    show.set_visible(true);
    ui.window.add_css_class(LAUNCHING);
    let target = adw::CallbackAnimationTarget::new(glib::clone!(
        #[weak]
        show,
        #[weak(rename_to = window)]
        ui.window,
        move |at| {
            show.imp().at.set(at);
            if at >= LEAVES {
                window.remove_css_class(LAUNCHING);
            }
            show.queue_draw();
        }
    ));
    let animation = adw::TimedAnimation::builder()
        .widget(show)
        .value_from(0.0)
        .value_to(1.0)
        .duration(LENGTH_MS)
        .easing(adw::Easing::Linear)
        .target(&target)
        .build();
    animation.connect_done(glib::clone!(
        #[weak]
        show,
        #[weak(rename_to = window)]
        ui.window,
        move |_| {
            window.remove_css_class(LAUNCHING);
            show.set_visible(false);
            show.imp().art.replace(None);
        }
    ));
    // A second launch while one plays takes over from it.
    if let Some(old) = ui.launch_animation.replace(Some(animation.clone())) {
        old.pause();
    }
    animation.play();
}

#[cfg(test)]
mod tests {
    use super::{middle, phases};

    #[test]
    fn the_art_arrives_stands_and_leaves() {
        assert_eq!(phases(0.0), (0.0, 0.0));
        assert_eq!(phases(0.6), (1.0, 0.0));
        assert_eq!(phases(1.0), (1.0, 1.0));
    }

    #[test]
    fn the_art_stays_inside_any_window() {
        for (w, h) in [(360.0, 480.0), (1280.0, 720.0), (5120.0, 1440.0)] {
            for aspect in [0.66, 1.0, 1.78, 3.1] {
                let (x, y, cw, ch) = middle(w, h, aspect);
                assert!(x >= 0.0 && x + cw <= w && y + ch <= h, "{w}x{h} {aspect}");
            }
        }
    }
}
