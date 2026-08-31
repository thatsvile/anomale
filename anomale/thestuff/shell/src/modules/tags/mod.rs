use crate::niri::{self, TagState};
use async_channel;
use gtk4::prelude::*;
use gtk4::{Align, Box, Button, Label, Orientation};
use std::collections::HashMap;
use std::sync::{Arc, Mutex};

pub fn build(monitor: &gtk4::gdk::Monitor) -> Box {
    let container = Box::new(Orientation::Horizontal, 0);
    container.add_css_class("tags-container");

    let monitor_name = monitor.connector().unwrap_or_else(|| "Unknown".into());
    let buttons = Arc::new(Mutex::new(HashMap::new()));

    for i in 1..=9 {
        let button = Button::builder().has_frame(false).build();
        button.add_css_class("tag");
        button.set_visible(false);

        // Inner layout: solid CSS circle + Number.
        // A unicode bullet leaves Nvidia Wayland 1px AA crumbs on toggle;
        // a filled box does not go through the text rasterizer.
        let bbox = Box::new(Orientation::Horizontal, 2);
        let dot = Box::new(Orientation::Horizontal, 0);
        dot.add_css_class("dot");
        dot.set_valign(Align::Center);
        dot.set_halign(Align::Center);

        let num = Label::new(Some(&i.to_string()));
        num.add_css_class("num");
        num.set_valign(Align::Center);

        bbox.append(&dot);
        bbox.append(&num);
        button.set_child(Some(&bbox));

        let tag_id = i;
        button.connect_clicked(move |_| {
            niri::focus_workspace(tag_id);
        });

        container.append(&button);
        buttons.lock().unwrap().insert(i, button);
    }

    let (sender, receiver) = async_channel::unbounded();
    niri::spawn_workspace_watcher(monitor_name.to_string(), sender);

    let container_draw = container.clone();
    gtk4::glib::MainContext::default().spawn_local(async move {
        while let Ok((id, state)) = receiver.recv().await {
            apply_tag_state(&buttons, id, state);

            // Force a full tags redraw so Nvidia damage tracking clears old pixels.
            container_draw.queue_draw();
        }
    });

    container
}

fn apply_tag_state(buttons: &Arc<Mutex<HashMap<i32, Button>>>, id: i32, state: TagState) {
    let Ok(buttons) = buttons.lock() else {
        return;
    };

    let Some(button) = buttons.get(&id) else {
        return;
    };

    if state.selected {
        button.add_css_class("selected");
    } else {
        button.remove_css_class("selected");
    }

    if state.occupied {
        button.add_css_class("occupied");
    } else {
        button.remove_css_class("occupied");
    }

    if state.urgent {
        button.add_css_class("urgent");
    } else {
        button.remove_css_class("urgent");
    }

    if !state.occupied && !state.selected {
        button.set_visible(false);
    } else {
        button.set_visible(true);
    }
}
