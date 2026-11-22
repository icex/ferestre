//! A one-child container that caps its child's width and keeps it at the
//! start edge, where `adw::Clamp` would centre it. The Library tabs use it so
//! a text list stays readable under a header that keeps its place.
use gtk::{glib, prelude::*, subclass::prelude::*};

mod imp {
    use super::*;
    use std::cell::Cell;

    #[derive(Default)]
    pub struct Layout {
        pub(super) maximum: Cell<i32>,
    }

    #[glib::object_subclass]
    impl ObjectSubclass for Layout {
        const NAME: &'static str = "FerestreMaxWidthLayout";
        type Type = super::Layout;
        type ParentType = gtk::LayoutManager;
    }

    impl ObjectImpl for Layout {}

    impl LayoutManagerImpl for Layout {
        fn request_mode(&self, _widget: &gtk::Widget) -> gtk::SizeRequestMode {
            gtk::SizeRequestMode::HeightForWidth
        }

        fn measure(
            &self,
            widget: &gtk::Widget,
            orientation: gtk::Orientation,
            for_size: i32,
        ) -> (i32, i32, i32, i32) {
            let Some(child) = widget.first_child().filter(|c| c.should_layout()) else {
                return (0, 0, -1, -1);
            };
            let maximum = self.maximum.get();
            match orientation {
                gtk::Orientation::Horizontal => {
                    let (min, natural, _, _) = child.measure(orientation, for_size);
                    (min, natural.min(maximum).max(min), -1, -1)
                }
                _ => {
                    let width = if for_size < 0 {
                        -1
                    } else {
                        for_size.min(maximum)
                    };
                    let width = if width < 0 {
                        -1
                    } else {
                        width.max(child.measure(gtk::Orientation::Horizontal, -1).0)
                    };
                    let (min, natural, _, _) = child.measure(orientation, width);
                    (min, natural, -1, -1)
                }
            }
        }

        fn allocate(&self, widget: &gtk::Widget, width: i32, height: i32, _baseline: i32) {
            if let Some(child) = widget.first_child().filter(|c| c.should_layout()) {
                let min = child.measure(gtk::Orientation::Horizontal, -1).0;
                let w = width.min(self.maximum.get()).max(min);
                child.size_allocate(&gtk::Allocation::new(0, 0, w, height), -1);
            }
        }
    }
}

glib::wrapper! {
    pub struct Layout(ObjectSubclass<imp::Layout>) @extends gtk::LayoutManager;
}

/// A box holding `child`, at most `maximum` px wide, at the start edge.
pub(crate) fn wrap(child: &impl IsA<gtk::Widget>, maximum: i32) -> gtk::Box {
    let layout: Layout = glib::Object::new();
    layout.imp().maximum.set(maximum);
    let holder = gtk::Box::builder().hexpand(true).build();
    holder.set_layout_manager(Some(layout));
    holder.append(child);
    holder
}
