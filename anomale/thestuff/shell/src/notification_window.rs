use gtk4::prelude::*;
use gtk4::{Application, ApplicationWindow, Box, Image, Label, Orientation, Align};
use gtk4::glib::ControlFlow;
use gtk4_layer_shell::{Edge, Layer, LayerShell};
use crate::config::NotifyConfig;
use std::cell::{Cell, RefCell};
use std::rc::Rc;
use std::time::Instant;

const ANIM_TICK_MS: u64 = 8;
const SLIDE_OFF_SCREEN_PAD: i32 = 40;
const SLIDE_IN_VISIBLE_SLIVER: i32 = 2;

fn off_screen_h_margin(config: &NotifyConfig) -> i32 {
    -(config.width + SLIDE_OFF_SCREEN_PAD)
}

fn slide_in_start_h_margin(config: &NotifyConfig) -> i32 {
    -(config.width - SLIDE_IN_VISIBLE_SLIVER)
}

fn ease_out_quad(t: f64) -> f64 {
    1.0 - (1.0 - t) * (1.0 - t)
}

fn ease_in_quad(t: f64) -> f64 {
    t * t
}

fn animate_layer_margin(
    window: ApplicationWindow,
    edge: Edge,
    from: i32,
    to: i32,
    duration_ms: u64,
    ease: impl Fn(f64) -> f64 + 'static,
    anim_gen: Rc<Cell<u32>>,
    expected_gen: u32,
    on_complete: impl FnOnce() + 'static,
) {
    window.set_margin(edge, from);

    let start = Instant::now();
    let ease = Rc::new(ease);
    let on_complete = Rc::new(RefCell::new(Some(on_complete)));
    let last_margin = Rc::new(Cell::new(from));

    gtk4::glib::timeout_add_local(std::time::Duration::from_millis(ANIM_TICK_MS), move || {
        if anim_gen.get() != expected_gen {
            return ControlFlow::Break;
        }

        let elapsed_ms = start.elapsed().as_secs_f64() * 1000.0;
        let t = (elapsed_ms / duration_ms as f64).min(1.0);
        let value = from + ((to - from) as f64 * ease(t)).round() as i32;

        if value != last_margin.get() {
            last_margin.set(value);
            window.set_margin(edge, value);
        }

        if t >= 1.0 {
            if last_margin.get() != to {
                window.set_margin(edge, to);
            }
            if let Some(cb) = on_complete.borrow_mut().take() {
                cb();
            }
            ControlFlow::Break
        } else {
            ControlFlow::Continue
        }
    });
}

pub struct NotificationWindow {
    pub id: u32,
    pub window: ApplicationWindow,
    pub config: NotifyConfig,
    h_edge: Edge,
    v_edge: Edge,
    hiding: RefCell<bool>,
    anim_gen: Rc<Cell<u32>>,
}

impl NotificationWindow {
    pub fn new(
        app: &Application,
        id: u32,
        _app_name: &str,
        summary: &str,
        body: &str,
        icon_name: &str,
        config: NotifyConfig,
        monitor: Option<&gtk4::gdk::Monitor>,
    ) -> Rc<Self> {
        let window = ApplicationWindow::builder()
            .application(app)
            .title(format!("Notification {}", id))
            .decorated(false)
            .build();

        window.init_layer_shell();
        window.set_namespace("anomale-notification");
        window.add_css_class("anomale-notification-window");
        window.set_layer(Layer::Overlay);
        if let Some(m) = monitor {
            window.set_monitor(m);
        }

        let (v_edge, h_edge) = match config.corner.as_str() {
            "top-left" => (Edge::Top, Edge::Left),
            "top-right" => (Edge::Top, Edge::Right),
            "bottom-left" => (Edge::Bottom, Edge::Left),
            "bottom-right" => (Edge::Bottom, Edge::Right),
            _ => (Edge::Bottom, Edge::Right),
        };

        window.set_anchor(v_edge, true);
        window.set_anchor(h_edge, true);
        window.set_margin(v_edge, config.margin);
        window.set_margin(h_edge, slide_in_start_h_margin(&config));
        window.set_default_size(config.width, config.height);
        window.set_size_request(config.width, config.height);

        let content_box = Box::builder()
            .orientation(Orientation::Horizontal)
            .spacing(15)
            .halign(Align::Fill)
            .valign(Align::Center)
            .build();
        content_box.add_css_class("notification-window");
        content_box.set_width_request(config.width);
        content_box.set_height_request(config.height);

        let icon = if !icon_name.is_empty() {
            let img = Image::from_icon_name(icon_name);
            img.set_pixel_size(48);
            img.set_valign(Align::Center);
            Some(img)
        } else {
            None
        };

        if let Some(i) = icon {
            content_box.append(&i);
        }

        let text_box = Box::builder()
            .orientation(Orientation::Vertical)
            .spacing(2)
            .hexpand(true)
            .valign(Align::Center)
            .build();

        let summary_label = Label::builder()
            .label(summary)
            .halign(Align::Start)
            .ellipsize(gtk4::pango::EllipsizeMode::End)
            .build();
        summary_label.add_css_class("notification-summary");

        let body_label = Label::builder()
            .label(body)
            .halign(Align::Start)
            .use_markup(true)
            .wrap(true)
            .max_width_chars(35)
            .ellipsize(gtk4::pango::EllipsizeMode::End)
            .build();
        body_label.add_css_class("notification-body");

        text_box.append(&summary_label);
        text_box.append(&body_label);
        content_box.append(&text_box);

        window.set_child(Some(&content_box));

        Rc::new(Self {
            id,
            window,
            config,
            h_edge,
            v_edge,
            hiding: RefCell::new(false),
            anim_gen: Rc::new(Cell::new(0)),
        })
    }

    fn bump_anim_gen(&self) -> u32 {
        let gen = self.anim_gen.get() + 1;
        self.anim_gen.set(gen);
        gen
    }

    pub fn show(&self) {
        let from = slide_in_start_h_margin(&self.config);
        let to = self.config.margin;

        self.window.set_margin(self.h_edge, from);
        self.window.present();

        let started = Rc::new(Cell::new(false));
        let h_edge = self.h_edge;
        let window = self.window.clone();
        let anim_gen = self.anim_gen.clone();
        let gen = self.bump_anim_gen();
        let duration_ms = self.config.animation_ms;
        gtk4::glib::idle_add_local_once(move || {
            if started.replace(true) {
                return;
            }
            animate_layer_margin(
                window,
                h_edge,
                from,
                to,
                duration_ms,
                ease_out_quad,
                anim_gen,
                gen,
                || {},
            );
        });
    }

    pub fn hide(&self, callback: impl FnOnce() + 'static) {
        if *self.hiding.borrow() {
            return;
        }
        *self.hiding.borrow_mut() = true;

        let window = self.window.clone();
        let window_for_close = window.clone();
        let h_edge = self.h_edge;
        let from = self.config.margin;
        let to = off_screen_h_margin(&self.config);
        let anim_gen = self.anim_gen.clone();
        let gen = self.bump_anim_gen();

        animate_layer_margin(
            window,
            h_edge,
            from,
            to,
            self.config.animation_ms,
            ease_in_quad,
            anim_gen,
            gen,
            move || {
                window_for_close.close();
                callback();
            },
        );
    }

    pub fn set_y_offset(&self, offset: i32) {
        let margin = self.config.margin + offset;
        if self.window.margin(self.v_edge) != margin {
            self.window.set_margin(self.v_edge, margin);
        }
    }
}
