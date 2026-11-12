//! Howdy face models. Adding, removing and testing need root, so they go
//! through the privileged helper.

use crate::app::Ctx;
use crate::backends::howdy::{self, FaceModel, ModelList};
use crate::helper::{self, Action};
use crate::ui::{self, log_view};
use adw::prelude::*;
use std::rc::Rc;

pub fn build(ctx: &Rc<Ctx>) -> gtk::Box {
    let user = ctx.user.borrow().clone();
    let outer = gtk::Box::new(gtk::Orientation::Vertical, 18);

    let models_group = adw::PreferencesGroup::builder()
        .title("Face models")
        .description(
            "Each model is a short capture of your face. Add more if recognition is unreliable, \
             e.g. with and without glasses.",
        )
        .build();
    outer.append(&models_group);
    match howdy::list_models(&user) {
        ModelList::Models(models) => fill_models(ctx, &models_group, &user, &models),
        ModelList::NeedsRoot => {
            let r = ui::row("Face models are only readable by root", "");
            let show = gtk::Button::with_label("Show");
            show.set_valign(gtk::Align::Center);
            let weak = Rc::downgrade(ctx);
            let group = models_group.clone();
            let u = user.clone();
            let r2 = r.clone();
            show.connect_clicked(move |_| {
                let Some(ctx) = weak.upgrade() else { return };
                let group = group.clone();
                let u = u.clone();
                let r2 = r2.clone();
                log_view::run(
                    &ctx,
                    None,
                    vec![Action::HowdyList { user: u.clone() }],
                    move |ctx, out| {
                        if out.result.is_ok() {
                            group.remove(&r2);
                            fill_models(ctx, &group, &u, &howdy::parse_plain_list(&out.output()));
                        } else {
                            ctx.toast("Could not read face models");
                        }
                    },
                );
            });
            r.add_suffix(&show);
            models_group.add(&r);
        }
    }

    // Add a model.
    let add_group = adw::PreferencesGroup::builder()
        .title("Add a face model")
        .description("Sit as you normally would and look straight at the camera for a few seconds.")
        .build();
    let label = adw::EntryRow::builder()
        .title("Label")
        .text("Initial model")
        .build();
    let add = gtk::Button::with_label("Add");
    add.set_valign(gtk::Align::Center);
    add.add_css_class("suggested-action");
    label.add_suffix(&add);
    add_group.add(&label);

    let test_row = ui::row(
        "Test recognition",
        "Checks whether Howdy recognises you right now",
    );
    let test = gtk::Button::with_label("Test");
    test.set_valign(gtk::Align::Center);
    test_row.add_suffix(&test);
    add_group.add(&test_row);
    outer.append(&add_group);

    let weak = Rc::downgrade(ctx);
    let u = user.clone();
    let label2 = label.clone();
    add.connect_clicked(move |_| {
        let Some(ctx) = weak.upgrade() else { return };
        let text = label2.text().trim().to_string();
        if let Err(e) = helper::check_label(&text) {
            ctx.toast(&format!("Label: {e}"));
            return;
        }
        log_view::run(
            &ctx,
            Some("Adding a face model"),
            vec![Action::HowdyAdd {
                user: u.clone(),
                label: text,
            }],
            |ctx, out| {
                ctx.toast(if out.result.is_ok() {
                    "Face model added"
                } else {
                    "Could not add a face model"
                });
                ctx.refresh();
            },
        );
    });

    let weak = Rc::downgrade(ctx);
    let u = user.clone();
    test.connect_clicked(move |_| {
        let Some(ctx) = weak.upgrade() else { return };
        ctx.toast("Look at the camera…");
        log_view::run(
            &ctx,
            None,
            vec![Action::HowdyTest { user: u.clone() }],
            |ctx, out| {
                let msg = out.result_message().unwrap_or_else(|| match &out.result {
                    Ok(()) => "Face recognised".into(),
                    Err(e) => e.clone(),
                });
                ctx.toast(&msg);
            },
        );
    });
    outer
}

fn fill_models(ctx: &Rc<Ctx>, group: &adw::PreferencesGroup, user: &str, models: &[FaceModel]) {
    if models.is_empty() {
        let r = ui::row(&format!("{user} has no face models yet"), "");
        r.add_css_class("dim-label");
        group.add(&r);
        return;
    }
    for m in models {
        let when = if m.time > 0.0 {
            glib_date(m.time)
        } else {
            String::new()
        };
        let r = ui::row(
            if m.label.is_empty() {
                "Unnamed model"
            } else {
                &m.label
            },
            &format!(
                "ID {}{}",
                m.id,
                if when.is_empty() {
                    String::new()
                } else {
                    format!(" · {when}")
                }
            ),
        );
        r.add_prefix(&gtk::Image::from_icon_name("avatar-default-symbolic"));
        let del = ui::flat_icon_button("user-trash-symbolic", "Remove this model");
        let weak = Rc::downgrade(ctx);
        let (u, id) = (user.to_string(), m.id);
        del.connect_clicked(move |_| {
            if let Some(ctx) = weak.upgrade() {
                log_view::run(
                    &ctx,
                    None,
                    vec![Action::HowdyRemove {
                        user: u.clone(),
                        id,
                    }],
                    |ctx, out| {
                        ctx.toast(if out.result.is_ok() {
                            "Face model removed"
                        } else {
                            "Could not remove the model"
                        });
                        ctx.refresh();
                    },
                );
            }
        });
        r.add_suffix(&del);
        group.add(&r);
    }
    if models.len() > 1 {
        let r = ui::row("Remove all face models", "");
        let b = gtk::Button::with_label("Remove all");
        b.set_valign(gtk::Align::Center);
        b.add_css_class("destructive-action");
        let weak = Rc::downgrade(ctx);
        let u = user.to_string();
        b.connect_clicked(move |_| {
            let Some(ctx) = weak.upgrade() else { return };
            let u = u.clone();
            gtk::glib::spawn_future_local(async move {
                if ui::confirm(
                    &ctx.window,
                    "Remove all face models?",
                    &format!(
                        "{u} won't be able to log in with their face until they enroll again."
                    ),
                    "Remove all",
                    true,
                )
                .await
                {
                    log_view::run(
                        &ctx,
                        None,
                        vec![Action::HowdyClear { user: u }],
                        |ctx, out| {
                            ctx.toast(if out.result.is_ok() {
                                "All face models removed"
                            } else {
                                "Could not remove models"
                            });
                            ctx.refresh();
                        },
                    );
                }
            });
        });
        r.add_suffix(&b);
        group.add(&r);
    }
}

fn glib_date(unix: f64) -> String {
    gtk::glib::DateTime::from_unix_local(unix as i64)
        .and_then(|d| d.format("%Y-%m-%d %H:%M"))
        .map(|s| s.to_string())
        .unwrap_or_default()
}
