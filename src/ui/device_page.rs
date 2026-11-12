//! One device: a guided page that runs top to bottom.
//! 1. Set up the driver  2. Enroll  3. Use it for login and sudo.

use crate::app::Ctx;
use crate::backends::{pam, AuthDevice, Backend, Method, Status};
use crate::config::ValiditySource;
use crate::detect::{system, Report};
use crate::helper::Action;
use crate::ui::{self, log_view};
use adw::prelude::*;
use gtk::glib;
use std::rc::Rc;

pub struct DevicePage {
    pub page: adw::NavigationPage,
    key: String,
    body: gtk::Box,
}

impl DevicePage {
    pub fn new(key: &str) -> Rc<Self> {
        let body = gtk::Box::new(gtk::Orientation::Vertical, 24);
        body.set_margin_top(24);
        body.set_margin_bottom(24);
        body.set_margin_start(12);
        body.set_margin_end(12);
        let clamp = adw::Clamp::builder().maximum_size(680).child(&body).build();
        let scroller = gtk::ScrolledWindow::builder()
            .hscrollbar_policy(gtk::PolicyType::Never)
            .child(&clamp)
            .build();
        let tv = adw::ToolbarView::new();
        tv.add_top_bar(&adw::HeaderBar::new());
        tv.set_content(Some(&scroller));
        let page = adw::NavigationPage::builder()
            .title("Device")
            .child(&tv)
            .build();
        Rc::new(DevicePage {
            page,
            key: key.to_string(),
            body,
        })
    }

    pub fn render(&self, ctx: &Rc<Ctx>, report: &Report) {
        ui::clear_box(&self.body);
        let Some(dev) = report.devices.iter().find(|d| d.key == self.key) else {
            self.body.append(
                &adw::StatusPage::builder()
                    .icon_name("dialog-question-symbolic")
                    .title("Device not found")
                    .description("It may have been unplugged. Go back and refresh.")
                    .build(),
            );
            return;
        };
        self.page.set_title(&dev.title);

        self.body.append(&summary_group(dev));
        self.body.append(&setup_group(ctx, report, dev));
        if dev.method == Method::Face && report.system.howdy_installed {
            self.body.append(&camera_group(ctx, report, dev));
        }
        if dev.status == Status::Ready {
            self.body.append(&profile_group(ctx));
            match dev.method {
                Method::Fingerprint => self.body.append(&ui::enroll_finger::build(ctx)),
                Method::Face => self.body.append(&ui::enroll_face::build(ctx)),
            }
        }
        self.body.append(&login_group(ctx, report, dev));
    }
}

fn summary_group(dev: &AuthDevice) -> adw::PreferencesGroup {
    let g = adw::PreferencesGroup::new();
    let head = ui::row(&dev.title, &dev.hardware);
    head.add_prefix(
        &gtk::Image::builder()
            .icon_name(ui::method_icon(dev.method))
            .pixel_size(32)
            .build(),
    );
    head.add_suffix(&ui::status_pill(dev.status));
    g.add(&head);
    g.add(&ui::row("Driver", &dev.backend.label()));
    for n in &dev.notes {
        let warn = dev.status != Status::Ready || n.contains("not") || n.contains("safer");
        g.add(&ui::note_row(n, warn));
    }
    g
}

fn setup_group(ctx: &Rc<Ctx>, report: &Report, dev: &AuthDevice) -> adw::PreferencesGroup {
    let g = adw::PreferencesGroup::builder().title("1 · Driver").build();

    // python-validity forks run from a checkout the user picks.
    if let Backend::PythonValidity {
        source: ValiditySource::Local,
        checkout,
    } = &dev.backend
    {
        let r = ui::row(
            "python-validity source",
            &checkout
                .as_ref()
                .map(|c| c.display().to_string())
                .unwrap_or_else(|| "Not chosen".into()),
        );
        let choose = gtk::Button::with_label("Choose…");
        choose.set_valign(gtk::Align::Center);
        let weak = Rc::downgrade(ctx);
        choose.connect_clicked(move |_| {
            let Some(ctx) = weak.upgrade() else { return };
            glib::spawn_future_local(async move {
                let fd = gtk::FileDialog::builder().title("Choose the python-validity checkout").modal(true).build();
                let Ok(folder) = fd.select_folder_future(Some(&ctx.window)).await else { return };
                let Some(path) = folder.path() else { return };
                if !system::is_validity_checkout(&path) {
                    ctx.toast("That folder has no dbus_service/dbus-service — not a python-validity checkout");
                    return;
                }
                ctx.settings.borrow_mut().validity_checkout = Some(path);
                if let Err(e) = ctx.settings.borrow().save() {
                    ctx.toast(&format!("Could not save settings: {e}"));
                }
                ctx.refresh();
            });
        });
        r.add_suffix(&choose);
        g.add(&r);
    }

    match dev.status {
        Status::Ready => {
            g.set_description(Some("Installed and running."));
            if dev.method == Method::Fingerprint {
                let r = ui::row(
                    "Restart driver services",
                    "Helps if the reader stopped answering, e.g. after suspend",
                );
                let b = gtk::Button::with_label("Restart");
                b.set_valign(gtk::Align::Center);
                let units: Vec<&str> = match &dev.backend {
                    Backend::PythonValidity { .. } => {
                        vec!["python3-validity.service", "open-fprintd.service"]
                    }
                    _ => vec!["fprintd.service"],
                };
                let actions: Vec<Action> = units
                    .into_iter()
                    .filter(|u| report.system.unit(u).exists())
                    .map(|u| Action::Systemctl {
                        verb: "restart".into(),
                        unit: u.into(),
                    })
                    .collect();
                b.set_sensitive(!actions.is_empty());
                let weak = Rc::downgrade(ctx);
                b.connect_clicked(move |_| {
                    if let Some(ctx) = weak.upgrade() {
                        log_view::run(&ctx, None, actions.clone(), |ctx, out| {
                            ctx.toast(if out.result.is_ok() {
                                "Services restarted"
                            } else {
                                "Restart failed"
                            });
                            ctx.refresh();
                        });
                    }
                });
                r.add_suffix(&b);
                g.add(&r);
            }
        }
        Status::Unsupported => {
            g.set_description(Some(
                "There's nothing this tool can install for this sensor.",
            ));
        }
        Status::NeedsSetup | Status::Conflict => {
            g.set_description(Some(
                "These steps run as administrator. You'll be asked for your password once.",
            ));
            for (i, a) in dev.setup.iter().enumerate() {
                let r = ui::row(&a.describe(), "");
                r.add_prefix(
                    &gtk::Label::builder()
                        .label(format!("{}", i + 1))
                        .css_classes(["dim-label", "numeric"])
                        .build(),
                );
                g.add(&r);
            }
            if let Some(b) = &dev.setup_blocker {
                g.add(&ui::note_row(b, true));
            }
            let button = ui::pill_button(
                if dev.status == Status::Conflict {
                    "Fix and install"
                } else {
                    "Install and configure"
                },
                Some("suggested-action"),
            );
            button.set_halign(gtk::Align::Center);
            button.set_margin_top(12);
            button.set_sensitive(
                dev.setup_blocker.is_none() && !dev.setup.is_empty() && report.system.has_pkexec,
            );
            let actions = dev.setup.clone();
            let title = format!("Setting up {}", dev.title);
            let weak = Rc::downgrade(ctx);
            button.connect_clicked(move |_| {
                if let Some(ctx) = weak.upgrade() {
                    log_view::run(&ctx, Some(&title), actions.clone(), |ctx, out| {
                        ctx.toast(if out.result.is_ok() {
                            "Setup finished"
                        } else {
                            "Setup failed — see the log"
                        });
                        ctx.refresh();
                    });
                }
            });
            // Buttons can't go in a PreferencesGroup's list, so put it in the header suffix area below.
            let holder = gtk::Box::new(gtk::Orientation::Vertical, 0);
            holder.append(&button);
            g.add(&holder);
        }
    }
    g
}

fn camera_group(ctx: &Rc<Ctx>, report: &Report, dev: &AuthDevice) -> adw::PreferencesGroup {
    let g = adw::PreferencesGroup::builder()
        .title("Camera")
        .description(
            "Howdy should use the infrared camera. It works in the dark and a photo can't fool it.",
        )
        .build();
    let cams = &report.cameras;
    let names: Vec<String> = cams
        .iter()
        .map(|c| {
            format!(
                "{}{} — {}",
                c.name,
                if c.is_ir { " (IR)" } else { "" },
                c.node
            )
        })
        .collect();
    let refs: Vec<&str> = names.iter().map(String::as_str).collect();
    let model = gtk::StringList::new(&refs);
    let combo = adw::ComboRow::builder()
        .title("Camera used by Howdy")
        .model(&model)
        .build();
    let configured = report.system.howdy.device_path.clone();
    let current_idx = dev
        .camera
        .as_ref()
        .and_then(|c| cams.iter().position(|x| x.node == c.node))
        .unwrap_or(0);
    combo.set_selected(current_idx as u32);

    let apply = gtk::Button::with_label("Apply");
    apply.set_valign(gtk::Align::Center);
    apply.add_css_class("suggested-action");
    let is_configured =
        |i: u32| configured.as_deref() == cams.get(i as usize).map(|c| c.node.as_str());
    apply.set_visible(!is_configured(current_idx as u32));
    combo.add_suffix(&apply);

    let nodes: Vec<String> = cams.iter().map(|c| c.node.clone()).collect();
    let configured2 = configured.clone();
    let apply2 = apply.clone();
    combo.connect_selected_notify(move |c| {
        let node = nodes.get(c.selected() as usize);
        apply2.set_visible(node.map(String::as_str) != configured2.as_deref());
    });

    let nodes: Vec<String> = cams.iter().map(|c| c.node.clone()).collect();
    let weak = Rc::downgrade(ctx);
    let combo2 = combo.clone();
    apply.connect_clicked(move |_| {
        let Some(ctx) = weak.upgrade() else { return };
        let Some(node) = nodes.get(combo2.selected() as usize).cloned() else {
            return;
        };
        let user = ctx
            .report
            .borrow()
            .as_ref()
            .map(|r| r.system.current_user.clone())
            .unwrap_or_default();
        let action = Action::HowdySet {
            user,
            key: "device_path".into(),
            value: node.clone(),
        };
        log_view::run(&ctx, None, vec![action], move |ctx, out| {
            match out.result {
                Ok(()) => ctx.toast(&format!("Howdy now uses {node}")),
                Err(e) => ctx.toast(&format!("Could not change camera: {e}")),
            }
            ctx.refresh();
        });
    });
    g.add(&combo);
    g
}

fn profile_group(ctx: &Rc<Ctx>) -> adw::PreferencesGroup {
    let g = adw::PreferencesGroup::builder().title("2 · Enroll").build();
    let names: Vec<String> = ctx
        .users
        .iter()
        .map(|u| {
            if u.full_name.is_empty() {
                u.name.clone()
            } else {
                format!("{} ({})", u.full_name, u.name)
            }
        })
        .collect();
    let refs: Vec<&str> = names.iter().map(String::as_str).collect();
    let combo = adw::ComboRow::builder()
        .title("Profile")
        .subtitle("The user account to enroll")
        .model(&gtk::StringList::new(&refs))
        .build();
    let current = ctx.user.borrow().clone();
    if let Some(i) = ctx.users.iter().position(|u| u.name == current) {
        combo.set_selected(i as u32);
    }
    let weak = Rc::downgrade(ctx);
    combo.connect_selected_notify(move |c| {
        let Some(ctx) = weak.upgrade() else { return };
        if let Some(u) = ctx.users.get(c.selected() as usize) {
            if *ctx.user.borrow() != u.name {
                ctx.user.replace(u.name.clone());
                // Re-render so the enroll section loads the new user's data.
                let report = ctx.report.borrow().clone();
                let page = ctx.device_page.borrow().clone();
                if let (Some(r), Some(p)) = (report, page) {
                    p.render(&ctx, &r);
                }
            }
        }
    });
    g.add(&combo);
    g
}

fn login_group(ctx: &Rc<Ctx>, report: &Report, dev: &AuthDevice) -> adw::PreferencesGroup {
    let g = adw::PreferencesGroup::builder()
        .title("3 · Login and sudo")
        .description("Uses pam-auth-update, so your password keeps working as a fallback.")
        .build();
    let pam_state = match dev.method {
        Method::Fingerprint => report.system.pam.fprintd.clone(),
        Method::Face => report.system.pam.howdy.clone(),
    };
    let sw = adw::SwitchRow::builder()
        .title("Use for login, sudo and screen unlock")
        .active(pam_state.enabled())
        .build();
    let usable =
        dev.status == Status::Ready && pam_state.profile_installed && report.system.has_pkexec;
    sw.set_sensitive(usable);
    if !pam_state.profile_installed {
        sw.set_subtitle(&format!(
            "No “{}” PAM profile is installed yet. Finish the driver setup first",
            pam_state.profile
        ));
    } else if pam_state.common_auth == pam::LineState::Commented {
        sw.set_subtitle(
            "Off because someone commented out its line in /etc/pam.d/common-auth by hand",
        );
    } else if dev.status != Status::Ready {
        sw.set_subtitle("Set up the driver first");
    }
    g.add(&sw);

    let profile = pam_state.profile.to_string();
    let weak = Rc::downgrade(ctx);
    sw.connect_active_notify(move |sw| {
        let Some(ctx) = weak.upgrade() else { return };
        let enable = sw.is_active();
        sw.set_sensitive(false);
        set_pam(&ctx, profile.clone(), enable, false);
    });
    g
}

fn set_pam(ctx: &Rc<Ctx>, profile: String, enable: bool, force: bool) {
    let action = Action::PamAuthUpdate {
        profile: profile.clone(),
        enable,
        force,
    };
    let started = log_view::run(ctx, None, vec![action], move |ctx, out| {
        let local_mods = pam::output_reports_local_modifications(&out.output());
        if out.result.is_ok() && !local_mods {
            ctx.toast(if enable {
                "Enabled for login and sudo"
            } else {
                "Disabled for login and sudo"
            });
            ctx.refresh();
            return;
        }
        if local_mods && !force {
            let ctx2 = ctx.clone();
            glib::spawn_future_local(async move {
                let yes = ui::confirm(
                    &ctx2.window,
                    "PAM files were edited by hand",
                    "pam-auth-update won't change /etc/pam.d/common-* because they were edited by hand. Overwrite \
                     them with the standard configuration? Your other enabled PAM profiles are kept. Keep a root \
                     terminal open until you've checked that sudo still works.",
                    "Overwrite",
                    true,
                )
                .await;
                if yes {
                    set_pam(&ctx2, profile, enable, true);
                } else {
                    ctx2.refresh();
                }
            });
            return;
        }
        let msg = out
            .result
            .err()
            .unwrap_or_else(|| "pam-auth-update made no change".into());
        ctx.toast(&format!("PAM change failed: {msg}"));
        ctx.refresh();
    });
    if !started {
        // Re-render so the switch shows the real state again.
        ctx.refresh();
    }
}
