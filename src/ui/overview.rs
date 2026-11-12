//! First screen: what was found and how ready each method is.

use crate::app::Ctx;
use crate::backends::{Method, Status};
use crate::detect::Report;
use crate::ui;
use adw::prelude::*;
use std::rc::Rc;

pub struct Overview {
    pub page: adw::NavigationPage,
    body: gtk::Box,
    stack: gtk::Stack,
    refresh: gtk::Button,
}

impl Overview {
    pub fn new(ctx: &Rc<Ctx>) -> Rc<Self> {
        let header = adw::HeaderBar::new();
        let refresh = gtk::Button::from_icon_name("view-refresh-symbolic");
        refresh.set_tooltip_text(Some("Detect again"));
        header.pack_start(&refresh);

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

        let spinner = gtk::Spinner::builder()
            .spinning(true)
            .width_request(32)
            .height_request(32)
            .build();
        let loading = adw::StatusPage::builder()
            .title("Looking for sensors…")
            .description("Checking USB devices, cameras, drivers and PAM")
            .child(&spinner)
            .build();

        let stack = gtk::Stack::new();
        stack.add_named(&loading, Some("loading"));
        stack.add_named(&scroller, Some("content"));

        let tv = adw::ToolbarView::new();
        tv.add_top_bar(&header);
        tv.set_content(Some(&stack));
        let page = adw::NavigationPage::builder()
            .title("Authentication")
            .tag("overview")
            .child(&tv)
            .build();

        let weak = Rc::downgrade(ctx);
        refresh.connect_clicked(move |_| {
            if let Some(ctx) = weak.upgrade() {
                ctx.refresh();
            }
        });

        Rc::new(Overview {
            page,
            body,
            stack,
            refresh,
        })
    }

    pub fn set_loading(&self, loading: bool) {
        self.refresh.set_sensitive(!loading);
        // Keep showing old content during a re-scan; only the first scan shows the spinner.
        if loading && self.body.first_child().is_none() {
            self.stack.set_visible_child_name("loading");
        }
    }

    pub fn render(&self, ctx: &Rc<Ctx>, report: &Report) {
        ui::clear_box(&self.body);
        self.stack.set_visible_child_name("content");

        let intro = gtk::Label::builder()
            .label(
                "This tool finds your fingerprint reader and IR camera, installs the right driver, enrolls you and \
                 switches them on for login and sudo. It doesn't do any authentication itself.",
            )
            .wrap(true)
            .xalign(0.0)
            .css_classes(["dim-label"])
            .build();
        self.body.append(&intro);

        if !report.system.has_pkexec {
            let b = adw::Banner::new("pkexec (polkit) is missing, so the tool can detect sensors but can't change anything");
            b.set_revealed(true);
            self.body.append(&b);
        }
        if !report.system.has_apt {
            let b = adw::Banner::new(
                "This system doesn't use apt. You'll need to install packages yourself",
            );
            b.set_revealed(true);
            self.body.append(&b);
        }

        for (method, title, empty) in [
            (
                Method::Fingerprint,
                "Fingerprint",
                "No fingerprint reader found",
            ),
            (Method::Face, "Face", "No camera found"),
        ] {
            let group = adw::PreferencesGroup::builder().title(title).build();
            let devices: Vec<_> = report
                .devices
                .iter()
                .filter(|d| d.method == method)
                .collect();
            if devices.is_empty() {
                let r = ui::row(empty, "");
                r.add_css_class("dim-label");
                group.add(&r);
            }
            for dev in devices {
                let pam = match method {
                    Method::Fingerprint => &report.system.pam.fprintd,
                    Method::Face => &report.system.pam.howdy,
                };
                let mut subtitle = dev.backend.label();
                if dev.status == Status::Ready {
                    subtitle.push_str(if pam.enabled() {
                        " · used for login"
                    } else {
                        " · not used for login yet"
                    });
                }
                let r = ui::row(&dev.title, &subtitle);
                r.set_activatable(true);
                r.add_prefix(&gtk::Image::from_icon_name(ui::method_icon(method)));
                r.add_suffix(&ui::status_pill(dev.status));
                r.add_suffix(&gtk::Image::from_icon_name("go-next-symbolic"));
                let key = dev.key.clone();
                let weak = Rc::downgrade(ctx);
                r.connect_activated(move |_| {
                    if let Some(ctx) = weak.upgrade() {
                        ctx.open_device(&key);
                    }
                });
                group.add(&r);
            }
            self.body.append(&group);
        }

        // Raw hardware, for reference.
        let hw = adw::PreferencesGroup::builder()
            .title("Detected hardware")
            .build();
        let cams = adw::ExpanderRow::builder()
            .title("Cameras")
            .subtitle(format!("{} found", report.cameras.len()))
            .build();
        for c in &report.cameras {
            let r = ui::row(
                &c.name,
                &format!("{}{}", c.node, if c.is_ir { " · infrared" } else { "" }),
            );
            cams.add_row(&r);
        }
        hw.add(&cams);
        let fp_svc = adw::ExpanderRow::builder()
            .title("Fingerprint services")
            .build();
        match &report.system.fprintd_devices {
            Ok(list) if !list.is_empty() => {
                for (path, name) in list {
                    fp_svc.add_row(&ui::row(name, path));
                }
            }
            Ok(_) => fp_svc.add_row(&ui::row("fprintd reports no readers", "")),
            Err(e) => fp_svc.add_row(&ui::row("fprintd is not reachable", e)),
        }
        for (unit, st) in report.system.units.iter().filter(|(_, s)| s.exists()) {
            fp_svc.add_row(&ui::row(unit, &format!("{} · {}", st.active, st.enabled)));
        }
        hw.add(&fp_svc);
        self.body.append(&hw);
    }
}
