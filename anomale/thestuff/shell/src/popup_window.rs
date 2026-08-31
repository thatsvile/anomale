use crate::niri;
use gtk4::glib;
use gtk4::prelude::*;
use gtk4::{ApplicationWindow, EventControllerKey, EventControllerMotion, Orientation, Widget};
use std::cell::{Cell, RefCell};
use std::collections::HashMap;
use std::rc::Rc;
use std::time::{SystemTime, UNIX_EPOCH};

const POPUP_HORIZONTAL_PADDING: i32 = 20;
const POINTER_HIDDEN_CLASS: &str = "popup-pointer-hidden";
const POINTER_HIDE_GRACE_MS: u64 = 200;

struct PointerHideState {
    cursor_hidden: Cell<bool>,
    suppress_motion_until: Cell<u64>,
}

thread_local! {
    static POINTER_HIDE_STATES: RefCell<HashMap<usize, Rc<PointerHideState>>> = RefCell::new(HashMap::new());
}

fn pointer_hide_states_mut<R>(f: impl FnOnce(&mut HashMap<usize, Rc<PointerHideState>>) -> R) -> R {
    POINTER_HIDE_STATES.with(|states| f(&mut states.borrow_mut()))
}

fn window_key(window: &ApplicationWindow) -> usize {
    window.as_ptr() as usize
}

fn now_ms() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|duration| duration.as_millis() as u64)
        .unwrap_or(0)
}

fn extend_pointer_hide_grace(window: &ApplicationWindow, extra_ms: u64) {
    pointer_hide_states_mut(|states| {
        let Some(state) = states.get(&window_key(window)) else {
            return;
        };
        state
            .suppress_motion_until
            .set(now_ms().saturating_add(extra_ms));
        if state.cursor_hidden.get() {
            enter_hidden(window, state);
        }
    });
}

pub struct PopupOptions {
    pub width: i32,
    pub height: Option<i32>,
}

impl PopupOptions {
    pub fn from_search_width(search_width: i32) -> Self {
        Self {
            width: search_width + POPUP_HORIZONTAL_PADDING,
            height: None,
        }
    }

    pub fn sized(width: i32, height: i32) -> Self {
        Self {
            width,
            height: Some(height),
        }
    }
}

/// Configure a menu window as a floating toplevel popup (not layer-shell).
pub fn prepare_popup_window(window: &ApplicationWindow, options: PopupOptions) {
    window.set_decorated(false);
    window.set_resizable(false);
    let height = options.height.unwrap_or(1);
    window.set_default_size(options.width, height);
    attach_hide_cursor_until_motion(window);

    // Niri `close-window` (e.g. Mod+Q) sends an xdg close request — hide, don't destroy.
    window.connect_close_request(|window| {
        window.set_visible(false);
        glib::Propagation::Stop
    });
}

/// Hide the pointer over menu popups until the user moves the mouse.
///
/// Niri's `warp-mouse-to-focus` centers the cursor on newly focused floating windows;
/// hiding it avoids obstructing the menu until deliberate mouse input.
fn attach_hide_cursor_until_motion(window: &ApplicationWindow) {
    let state = Rc::new(PointerHideState {
        cursor_hidden: Cell::new(false),
        suppress_motion_until: Cell::new(0),
    });
    pointer_hide_states_mut(|states| {
        states.insert(window_key(window), state.clone());
    });

    let window_show = window.clone();
    let state_show = state.clone();
    window.connect_show(move |_| {
        enter_hidden(&window_show, &state_show);
    });

    let motion = EventControllerMotion::new();
    let window_motion = window.clone();
    let state_motion = state.clone();
    motion.connect_motion(move |_, _, _| {
        if now_ms() < state_motion.suppress_motion_until.get() {
            return;
        }
        if state_motion.cursor_hidden.get() {
            state_motion.cursor_hidden.set(false);
            window_motion.remove_css_class(POINTER_HIDDEN_CLASS);
            restore_cursor(&window_motion);
            set_pointer_interaction(&window_motion, true);
        }
    });
    window.add_controller(motion);

    // Focused Entry widgets set a text cursor on their own; re-hide after keyboard input.
    let key = EventControllerKey::new();
    key.set_propagation_phase(gtk4::PropagationPhase::Capture);
    let window_key_ctrl = window.clone();
    let state_key = state.clone();
    key.connect_key_pressed(move |_, _, _, _| {
        if state_key.cursor_hidden.get() {
            enter_hidden(&window_key_ctrl, &state_key);
        }
        glib::Propagation::Proceed
    });
    window.add_controller(key);
}

fn enter_hidden(window: &ApplicationWindow, state: &PointerHideState) {
    state.cursor_hidden.set(true);
    state
        .suppress_motion_until
        .set(now_ms().saturating_add(POINTER_HIDE_GRACE_MS));
    window.add_css_class(POINTER_HIDDEN_CLASS);
    apply_hidden_cursor(window);
    set_pointer_interaction(window, false);
}

fn apply_hidden_cursor(window: &ApplicationWindow) {
    let _ = window.set_cursor_from_name(Some("none"));
    if let Some(child) = window.child() {
        apply_hidden_cursor_recursive(&child);
    }
}

fn apply_hidden_cursor_recursive(widget: &Widget) {
    let _ = widget.set_cursor_from_name(Some("none"));
    let mut child = widget.first_child();
    while let Some(current) = child {
        apply_hidden_cursor_recursive(&current);
        child = current.next_sibling();
    }
}

fn restore_cursor(window: &ApplicationWindow) {
    let _ = window.set_cursor_from_name(None);
    if let Some(child) = window.child() {
        restore_cursor_recursive(&child);
    }
}

fn restore_cursor_recursive(widget: &Widget) {
    let _ = widget.set_cursor_from_name(None);
    let mut child = widget.first_child();
    while let Some(current) = child {
        restore_cursor_recursive(&current);
        child = current.next_sibling();
    }
}

fn set_pointer_interaction(window: &ApplicationWindow, enabled: bool) {
    window.set_can_target(enabled);
    if let Some(child) = window.child() {
        set_pointer_interaction_recursive(&child, enabled);
    }
}

fn set_pointer_interaction_recursive(widget: &Widget, enabled: bool) {
    widget.set_can_target(enabled);
    let mut child = widget.first_child();
    while let Some(current) = child {
        set_pointer_interaction_recursive(&current, enabled);
        child = current.next_sibling();
    }
}

fn maintain_hidden_pointer_state(window: &ApplicationWindow) {
    if window.has_css_class(POINTER_HIDDEN_CLASS) {
        apply_hidden_cursor(window);
        set_pointer_interaction(window, false);
    }
}

/// Show the popup and request compositor focus.
pub fn present_popup(window: &ApplicationWindow) {
    window.present();
    maintain_hidden_pointer_state(window);
}

/// Resize the window to fit its child after layout.
pub fn resize_popup_to_content(window: &ApplicationWindow) {
    resize_popup_to_content_at(window, None);
}

/// Resize the popup, then optionally anchor it below the top of the workspace.
pub fn resize_popup_to_content_at(window: &ApplicationWindow, top_y: Option<i32>) {
    let window = window.clone();
    glib::idle_add_local(move || {
        if let Some(child) = window.child() {
            let (min_w, nat_w, _, _) = child.measure(Orientation::Horizontal, -1);
            let (min_h, nat_h, _, _) = child.measure(Orientation::Vertical, -1);
            let width = nat_w.max(min_w);
            let height = nat_h.max(min_h);
            if width > 0 && height > 0 {
                window.set_default_size(width, height);
                window.set_size_request(width, height);
            }
        }
        maintain_hidden_pointer_state(&window);

        if let Some(y) = top_y {
            schedule_floating_position(&window, y);
        }

        glib::ControlFlow::Break
    });
}

fn schedule_floating_position(window: &ApplicationWindow, top_y: i32) {
    let title = window.title().unwrap_or_default().to_string();
    let window_for_idle = window.clone();
    glib::idle_add_local({
        let title = title.clone();
        move || {
            niri::position_floating_window_top(&title, top_y);
            extend_pointer_hide_grace(&window_for_idle, POINTER_HIDE_GRACE_MS);
            glib::ControlFlow::Break
        }
    });
    let window_for_timeout = window.clone();
    glib::timeout_add_local_once(std::time::Duration::from_millis(50), move || {
        niri::position_floating_window_top(&title, top_y);
        extend_pointer_hide_grace(&window_for_timeout, POINTER_HIDE_GRACE_MS);
    });
}
