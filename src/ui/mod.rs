pub mod device_page;
pub mod enroll_face;
pub mod enroll_finger;
pub mod log_view;
pub mod overview;

use crate::backends::{Method, Status};
use adw::prelude::*;

pub fn status_pill(status: Status) -> gtk::Label {
    let l = gtk::Label::new(Some(status.label()));
    l.add_css_class("status-pill");
    l.add_css_class(match status {
        Status::Ready => "ready",
        Status::NeedsSetup => "setup",
        Status::Conflict => "conflict",
        Status::Unsupported => "unsupported",
    });
    l.set_valign(gtk::Align::Center);
    l
}

pub fn method_icon(method: Method) -> &'static str {
    match method {
        Method::Fingerprint => "auth-fingerprint-symbolic",
        Method::Face => "camera-web-symbolic",
    }
}

pub fn clear_box(b: &gtk::Box) {
    while let Some(child) = b.first_child() {
        b.remove(&child);
    }
}

/// A plain row; titles are never parsed as markup.
pub fn row(title: &str, subtitle: &str) -> adw::ActionRow {
    let r = adw::ActionRow::builder()
        .title(title)
        .use_markup(false)
        .build();
    if !subtitle.is_empty() {
        r.set_subtitle(subtitle);
        r.set_subtitle_lines(0);
    }
    r
}

/// A row carrying an explanatory note with an icon.
pub fn note_row(text: &str, warning: bool) -> adw::ActionRow {
    let r = adw::ActionRow::builder()
        .title(text)
        .use_markup(false)
        .title_lines(0)
        .build();
    let icon = gtk::Image::from_icon_name(if warning {
        "dialog-warning-symbolic"
    } else {
        "dialog-information-symbolic"
    });
    if warning {
        icon.add_css_class("warning");
    } else {
        icon.add_css_class("dim-label");
    }
    r.add_prefix(&icon);
    r
}

pub fn pill_button(label: &str, style: Option<&str>) -> gtk::Button {
    let b = gtk::Button::with_label(label);
    b.add_css_class("pill");
    if let Some(s) = style {
        b.add_css_class(s);
    }
    b
}

pub fn flat_icon_button(icon: &str, tooltip: &str) -> gtk::Button {
    let b = gtk::Button::from_icon_name(icon);
    b.add_css_class("flat");
    b.set_valign(gtk::Align::Center);
    b.set_tooltip_text(Some(tooltip));
    b
}

/// Ask a yes/no question; resolves to true when `confirm_id` was chosen.
pub async fn confirm(
    parent: &impl IsA<gtk::Widget>,
    heading: &str,
    body: &str,
    confirm_label: &str,
    destructive: bool,
) -> bool {
    let d = adw::AlertDialog::new(Some(heading), Some(body));
    d.add_responses(&[("cancel", "Cancel"), ("ok", confirm_label)]);
    d.set_response_appearance(
        "ok",
        if destructive {
            adw::ResponseAppearance::Destructive
        } else {
            adw::ResponseAppearance::Suggested
        },
    );
    d.set_default_response(Some("cancel"));
    d.set_close_response("cancel");
    d.choose_future(parent).await == "ok"
}
