//! Fingerprint enrollment over fprintd's D-Bus API (no root needed; polkit
//! decides, and asks for a password if it has to).

use crate::app::Ctx;
use crate::backends::fprintd::{self, Device, FINGERS};
use crate::ui;
use adw::prelude::*;
use gtk::glib;
use std::cell::RefCell;
use std::rc::Rc;

struct State {
    ctx: Rc<Ctx>,
    user: String,
    device: RefCell<Option<Rc<Device>>>,
    enrolled: RefCell<Vec<String>>,
    cancel: RefCell<Option<async_channel::Sender<()>>>,
    fingers: Vec<(String, gtk::ToggleButton)>,
    icon: gtk::Image,
    status: gtk::Label,
    progress: gtk::ProgressBar,
    enroll: gtk::Button,
    test: gtk::Button,
    delete: gtk::Button,
    delete_all: gtk::Button,
    cancel_btn: gtk::Button,
}

pub fn build(ctx: &Rc<Ctx>) -> gtk::Box {
    let outer = gtk::Box::new(gtk::Orientation::Vertical, 0);
    let card = gtk::Box::new(gtk::Orientation::Vertical, 14);
    card.add_css_class("card");
    card.add_css_class("enroll-card");
    outer.append(&card);

    let icon = gtk::Image::builder()
        .icon_name("auth-fingerprint-symbolic")
        .pixel_size(72)
        .build();
    icon.add_css_class("dim-label");
    let status = gtk::Label::builder()
        .label("Connecting to the reader…")
        .wrap(true)
        .justify(gtk::Justification::Center)
        .css_classes(["title-4"])
        .build();
    let progress = gtk::ProgressBar::new();
    progress.set_visible(false);
    card.append(&icon);
    card.append(&status);
    card.append(&progress);

    // Two rows of finger toggles: left hand, right hand.
    let mut fingers = Vec::new();
    let mut first: Option<gtk::ToggleButton> = None;
    for (hand, range) in [("Left hand", 0..5), ("Right hand", 5..10)] {
        let row = gtk::Box::new(gtk::Orientation::Horizontal, 6);
        row.set_halign(gtk::Align::Center);
        let lbl = gtk::Label::builder()
            .label(hand)
            .width_chars(10)
            .xalign(0.0)
            .css_classes(["dim-label"])
            .build();
        row.append(&lbl);
        let linked = gtk::Box::new(gtk::Orientation::Horizontal, 0);
        linked.add_css_class("linked");
        for (id, name) in &FINGERS[range] {
            let b = gtk::ToggleButton::with_label(name);
            b.add_css_class("finger-button");
            if let Some(f) = &first {
                b.set_group(Some(f));
            } else {
                first = Some(b.clone());
            }
            linked.append(&b);
            fingers.push((id.to_string(), b));
        }
        row.append(&linked);
        card.append(&row);
    }

    let buttons = gtk::Box::new(gtk::Orientation::Horizontal, 8);
    buttons.set_halign(gtk::Align::Center);
    buttons.set_margin_top(6);
    let enroll = ui::pill_button("Enroll finger", Some("suggested-action"));
    let test = ui::pill_button("Test", None);
    let delete = ui::pill_button("Delete", None);
    let delete_all = ui::pill_button("Delete all", Some("destructive-action"));
    let cancel_btn = ui::pill_button("Cancel", None);
    cancel_btn.set_visible(false);
    for b in [&enroll, &test, &delete, &delete_all, &cancel_btn] {
        buttons.append(b);
    }
    card.append(&buttons);

    let st = Rc::new(State {
        ctx: ctx.clone(),
        user: ctx.user.borrow().clone(),
        device: RefCell::new(None),
        enrolled: RefCell::new(Vec::new()),
        cancel: RefCell::new(None),
        fingers,
        icon,
        status,
        progress,
        enroll,
        test,
        delete,
        delete_all,
        cancel_btn,
    });
    st.set_controls(false);

    for (_, b) in &st.fingers {
        let s = Rc::downgrade(&st);
        b.connect_toggled(move |_| {
            if let Some(s) = s.upgrade() {
                s.update_buttons();
            }
        });
    }
    let s = st.clone();
    st.enroll.connect_clicked(move |_| s.clone().start_enroll());
    let s = st.clone();
    st.test.connect_clicked(move |_| s.clone().start_verify());
    let s = st.clone();
    st.delete
        .connect_clicked(move |_| s.clone().delete_selected());
    let s = st.clone();
    st.delete_all
        .connect_clicked(move |_| s.clone().delete_everything());
    let s = st.clone();
    st.cancel_btn.connect_clicked(move |_| {
        if let Some(tx) = s.cancel.borrow().as_ref() {
            let _ = tx.try_send(());
        }
    });

    let s = st.clone();
    glib::spawn_future_local(async move {
        match Device::default().await {
            Ok(dev) => {
                s.device.replace(Some(Rc::new(dev)));
                s.reload().await;
            }
            Err(e) => s.say(&format!("{e:#}"), true),
        }
    });
    outer
}

impl State {
    fn selected(&self) -> Option<String> {
        self.fingers
            .iter()
            .find(|(_, b)| b.is_active())
            .map(|(id, _)| id.clone())
    }

    fn say(&self, text: &str, error: bool) {
        self.status.set_label(text);
        if error {
            self.status.add_css_class("error");
        } else {
            self.status.remove_css_class("error");
        }
    }

    /// Enable/disable everything; `true` = idle and a device is connected.
    fn set_controls(&self, idle: bool) {
        for (_, b) in &self.fingers {
            b.set_sensitive(idle);
        }
        self.cancel_btn
            .set_visible(!idle && self.cancel.borrow().is_some());
        if idle {
            self.update_buttons();
        } else {
            for b in [&self.enroll, &self.test, &self.delete, &self.delete_all] {
                b.set_sensitive(false);
            }
        }
    }

    fn update_buttons(&self) {
        let connected = self.device.borrow().is_some();
        let enrolled = self.enrolled.borrow();
        let sel = self.selected();
        let sel_enrolled = sel.as_ref().is_some_and(|s| enrolled.contains(s));
        self.enroll.set_sensitive(connected && sel.is_some());
        self.enroll.set_label(if sel_enrolled {
            "Re-enroll finger"
        } else {
            "Enroll finger"
        });
        self.delete.set_sensitive(connected && sel_enrolled);
        self.test.set_sensitive(connected && !enrolled.is_empty());
        self.delete_all
            .set_sensitive(connected && !enrolled.is_empty());
    }

    async fn reload(self: &Rc<Self>) {
        let Some(dev) = self.device.borrow().clone() else {
            return;
        };
        match dev.enrolled(&self.user).await {
            Ok(list) => {
                for (id, b) in &self.fingers {
                    let on = list.contains(id);
                    let name = FINGERS
                        .iter()
                        .find(|(f, _)| f == id)
                        .map(|(_, n)| *n)
                        .unwrap_or("");
                    b.set_label(&if on {
                        format!("✓ {name}")
                    } else {
                        name.to_string()
                    });
                    if on {
                        b.add_css_class("enrolled");
                    } else {
                        b.remove_css_class("enrolled");
                    }
                }
                let msg = match list.len() {
                    0 => format!(
                        "{} has no fingerprints yet. Pick a finger to enroll",
                        self.user
                    ),
                    n => format!(
                        "{} has {n} enrolled finger{}",
                        self.user,
                        if n == 1 { "" } else { "s" }
                    ),
                };
                self.enrolled.replace(list);
                self.say(&msg, false);
            }
            Err(e) => self.say(&format!("Could not list fingerprints: {e:#}"), true),
        }
        self.set_controls(true);
    }

    fn begin(&self) -> async_channel::Receiver<()> {
        let (tx, rx) = async_channel::bounded(1);
        self.cancel.replace(Some(tx));
        self.set_controls(false);
        self.icon.remove_css_class("dim-label");
        self.icon.add_css_class("accent");
        self.progress.set_visible(true);
        self.progress.set_fraction(0.0);
        rx
    }

    fn end(&self, text: &str, ok: bool) {
        self.cancel.replace(None);
        self.icon.remove_css_class("accent");
        self.icon.add_css_class("dim-label");
        self.progress.set_visible(false);
        self.say(text, !ok);
        self.set_controls(true);
    }

    fn start_enroll(self: Rc<Self>) {
        let (Some(dev), Some(finger)) = (self.device.borrow().clone(), self.selected()) else {
            return;
        };
        let cancel = self.begin();
        let stages = dev.stages;
        let label = fprintd::finger_label(&finger);
        self.say(
            &format!("Place your {} on the sensor", label.to_lowercase()),
            false,
        );
        glib::spawn_future_local(async move {
            let mut passed = 0u32;
            let s = self.clone();
            let res = dev
                .enroll(&self.user, &finger, cancel, |status, done| {
                    if status == "enroll-stage-passed" {
                        passed += 1;
                        match stages {
                            Some(n) => s.progress.set_fraction((passed as f64 / n as f64).min(1.0)),
                            None => s.progress.pulse(),
                        }
                    }
                    if !done {
                        s.say(fprintd::status_message(status), false);
                    }
                })
                .await;
            match res {
                Ok(st) if st == "enroll-completed" => self.end(&format!("{label} enrolled"), true),
                Ok(st) => self.end(fprintd::status_message(&st), st == "cancelled"),
                Err(e) => self.end(&format!("{e:#}"), false),
            }
            self.reload_keep_message().await;
        });
    }

    /// Refresh the finger marks without replacing the last status message.
    async fn reload_keep_message(self: &Rc<Self>) {
        let msg = self.status.label().to_string();
        let err = self.status.has_css_class("error");
        self.reload().await;
        self.say(&msg, err);
    }

    fn start_verify(self: Rc<Self>) {
        let Some(dev) = self.device.borrow().clone() else {
            return;
        };
        let cancel = self.begin();
        self.progress.set_visible(false);
        self.say("Touch the sensor with any enrolled finger", false);
        glib::spawn_future_local(async move {
            let s = self.clone();
            let res = dev
                .verify(&self.user, cancel, |status, done| {
                    if !done {
                        s.say(fprintd::status_message(status), false);
                    }
                })
                .await;
            match res {
                Ok(st) => self.end(
                    fprintd::status_message(&st),
                    st == "verify-match" || st == "cancelled",
                ),
                Err(e) => self.end(&format!("{e:#}"), false),
            }
        });
    }

    fn delete_selected(self: Rc<Self>) {
        let (Some(dev), Some(finger)) = (self.device.borrow().clone(), self.selected()) else {
            return;
        };
        self.set_controls(false);
        glib::spawn_future_local(async move {
            match dev.delete_finger(&self.user, &finger).await {
                Ok(true) => {
                    self.ctx
                        .toast(&format!("{} deleted", fprintd::finger_label(&finger)));
                    self.reload().await;
                }
                Ok(false) => {
                    self.set_controls(true);
                    let yes = ui::confirm(
                        &self.ctx.window,
                        "Delete all fingerprints?",
                        &format!(
                            "This driver (open-fprintd) can only delete all of {}'s fingerprints at once. You can \
                             enroll the ones you want to keep again afterwards.",
                            self.user
                        ),
                        "Delete all",
                        true,
                    )
                    .await;
                    if yes {
                        self.clone().wipe(dev).await;
                    }
                }
                Err(e) => {
                    self.say(&format!("{e:#}"), true);
                    self.set_controls(true);
                }
            }
        });
    }

    fn delete_everything(self: Rc<Self>) {
        let Some(dev) = self.device.borrow().clone() else {
            return;
        };
        glib::spawn_future_local(async move {
            let yes = ui::confirm(
                &self.ctx.window,
                "Delete all fingerprints?",
                &format!(
                    "All of {}'s enrolled fingerprints will be removed from the reader.",
                    self.user
                ),
                "Delete all",
                true,
            )
            .await;
            if yes {
                self.clone().wipe(dev).await;
            }
        });
    }

    async fn wipe(self: Rc<Self>, dev: Rc<Device>) {
        self.set_controls(false);
        match dev.delete_all(&self.user).await {
            Ok(()) => {
                self.ctx.toast("All fingerprints deleted");
                self.reload().await;
            }
            Err(e) => {
                self.say(&format!("{e:#}"), true);
                self.set_controls(true);
            }
        }
    }
}
