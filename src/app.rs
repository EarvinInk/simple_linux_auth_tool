//! Application shell: window, navigation, shared state and refresh.

use crate::config::Settings;
use crate::detect::{self, system, Report};
use crate::ui;
use adw::prelude::*;
use gtk::glib;
use std::cell::{Cell, RefCell};
use std::process::ExitCode;
use std::rc::Rc;

pub const APP_ID: &str = "io.github.SimpleAuthTool";

const CSS: &str = r#"
.status-pill { border-radius: 999px; padding: 2px 10px; font-size: 0.85em; font-weight: 600; }
.status-pill.ready { background: alpha(@success_color, 0.18); color: @success_color; }
.status-pill.setup { background: alpha(@warning_color, 0.22); color: @warning_color; }
.status-pill.conflict, .status-pill.unsupported { background: alpha(@error_color, 0.18); color: @error_color; }
.finger-button { min-width: 76px; }
.finger-button.enrolled { color: @success_color; font-weight: bold; }
.log-view { font-family: monospace; font-size: 0.9em; padding: 8px; }
.enroll-card { padding: 18px; }
"#;

pub struct Ctx {
    pub window: adw::ApplicationWindow,
    pub nav: adw::NavigationView,
    pub toasts: adw::ToastOverlay,
    pub report: RefCell<Option<Rc<Report>>>,
    pub settings: RefCell<Settings>,
    pub overview: RefCell<Option<Rc<ui::overview::Overview>>>,
    pub device_page: RefCell<Option<Rc<ui::device_page::DevicePage>>>,
    /// The account whose fingerprints / faces are being managed.
    pub user: RefCell<String>,
    pub users: Vec<system::User>,
    pub busy: Cell<bool>,
    pub loading: Cell<bool>,
    opened_initial: Cell<bool>,
}

impl Ctx {
    pub fn toast(&self, msg: &str) {
        let t = adw::Toast::new(msg);
        t.set_timeout(4);
        self.toasts.add_toast(t);
    }

    pub fn refresh(self: &Rc<Self>) {
        if self.loading.replace(true) {
            return;
        }
        if let Some(o) = self.overview.borrow().as_ref() {
            o.set_loading(true);
        }
        let (tx, rx) = async_channel::bounded(1);
        let settings = self.settings.borrow().clone();
        std::thread::spawn(move || {
            let _ = tx.send_blocking(detect::run(&settings));
        });
        let ctx = self.clone();
        glib::spawn_future_local(async move {
            if let Ok(report) = rx.recv().await {
                ctx.apply(Rc::new(report));
            }
            ctx.loading.set(false);
        });
    }

    fn apply(self: &Rc<Self>, report: Rc<Report>) {
        self.report.replace(Some(report.clone()));
        let overview = self.overview.borrow().clone();
        if let Some(o) = overview {
            o.set_loading(false);
            o.render(self, &report);
        }
        let page = self.device_page.borrow().clone();
        if let Some(p) = page {
            p.render(self, &report);
        } else if let Some(key) = std::env::var("SIMPLE_AUTH_TOOL_OPEN")
            .ok()
            .filter(|_| !self.opened_initial.replace(true))
        {
            // Debug/testing aid: open a device page directly, e.g. SIMPLE_AUTH_TOOL_OPEN=face
            self.open_device(&key);
        }
    }

    pub fn open_device(self: &Rc<Self>, key: &str) {
        let Some(report) = self.report.borrow().clone() else {
            return;
        };
        let page = ui::device_page::DevicePage::new(key);
        page.render(self, &report);
        self.nav.push(&page.page);
        self.device_page.replace(Some(page));
    }
}

pub fn run() -> ExitCode {
    let app = adw::Application::builder().application_id(APP_ID).build();
    app.connect_startup(|_| {
        let provider = gtk::CssProvider::new();
        provider.load_from_string(CSS);
        if let Some(display) = gtk::gdk::Display::default() {
            gtk::style_context_add_provider_for_display(
                &display,
                &provider,
                gtk::STYLE_PROVIDER_PRIORITY_APPLICATION,
            );
        }
    });
    app.connect_activate(build_window);
    // Our own flags were handled in main(); don't let GTK parse them.
    let code = app.run_with_args::<&str>(&[]);
    ExitCode::from(code.value().clamp(0, 255) as u8)
}

fn build_window(app: &adw::Application) {
    if let Some(w) = app.active_window() {
        w.present();
        return;
    }
    let nav = adw::NavigationView::new();
    let toasts = adw::ToastOverlay::new();
    toasts.set_child(Some(&nav));
    let window = adw::ApplicationWindow::builder()
        .application(app)
        .title("Authentication")
        .default_width(760)
        .default_height(820)
        .width_request(360)
        .height_request(400)
        .content(&toasts)
        .build();

    let current = system::current_user();
    let ctx = Rc::new(Ctx {
        window: window.clone(),
        nav: nav.clone(),
        toasts,
        report: RefCell::new(None),
        settings: RefCell::new(Settings::load()),
        overview: RefCell::new(None),
        device_page: RefCell::new(None),
        user: RefCell::new(current),
        users: system::human_users(),
        busy: Cell::new(false),
        loading: Cell::new(false),
        opened_initial: Cell::new(false),
    });

    let overview = ui::overview::Overview::new(&ctx);
    nav.add(&overview.page);
    ctx.overview.replace(Some(overview));

    let weak = Rc::downgrade(&ctx);
    nav.connect_popped(move |_, page| {
        if let Some(ctx) = weak.upgrade() {
            let is_current = ctx
                .device_page
                .borrow()
                .as_ref()
                .is_some_and(|p| &p.page == page);
            if is_current {
                ctx.device_page.replace(None);
            }
        }
    });

    // Every UI callback holds only a Weak<Ctx>; the window owns the one strong
    // reference, so the state lives exactly as long as the window.
    let keep_alive = RefCell::new(Some(ctx.clone()));
    window.connect_close_request(move |_| {
        keep_alive.take();
        glib::Propagation::Proceed
    });

    window.present();
    ctx.refresh();
}
