//! Runs privileged helper steps, optionally inside a dialog that shows step
//! progress and the full command output.

use crate::app::Ctx;
use crate::helper::Action;
use crate::privileged::{self, Event};
use adw::prelude::*;
use gtk::glib;
use std::rc::Rc;

pub struct Outcome {
    pub result: Result<(), String>,
    pub lines: Vec<String>,
}

impl Outcome {
    pub fn output(&self) -> String {
        self.lines.join("\n")
    }
    /// The `##RESULT <code> <message>` line printed by some helper actions.
    pub fn result_message(&self) -> Option<String> {
        self.lines.iter().rev().find_map(|l| {
            let rest = l.strip_prefix("##RESULT ")?;
            Some(
                rest.split_once(' ')
                    .map(|(_, m)| m.to_string())
                    .unwrap_or_default(),
            )
        })
    }
}

/// Run `actions` as root. With `title` a progress dialog is shown; without it
/// the run is silent. `on_done` is called once when the helper exits.
/// Returns false (and calls nothing) when the run could not start.
pub fn run(
    ctx: &Rc<Ctx>,
    title: Option<&str>,
    actions: Vec<Action>,
    on_done: impl FnOnce(&Rc<Ctx>, Outcome) + 'static,
) -> bool {
    if actions.is_empty() {
        return false;
    }
    if !ctx
        .report
        .borrow()
        .as_ref()
        .is_some_and(|r| r.system.has_pkexec)
    {
        ctx.toast("pkexec is not installed, so administrator actions are unavailable");
        return false;
    }
    if ctx.busy.replace(true) {
        ctx.toast("Another administrator task is still running");
        return false;
    }

    let dialog = title.map(|t| Dialog::new(t, &actions));
    if let Some(d) = &dialog {
        d.dialog.present(&ctx.window);
    }

    let rx = privileged::run(actions);
    let ctx = ctx.clone();
    glib::spawn_future_local(async move {
        let mut lines = Vec::new();
        let mut result = Err("Helper exited unexpectedly".to_string());
        while let Ok(ev) = rx.recv().await {
            match ev {
                Event::Step { index, total, text } => {
                    if let Some(d) = &dialog {
                        d.step(index, total, &text);
                    }
                }
                Event::Line(line) => {
                    if let Some(d) = &dialog {
                        if !line.starts_with("##") {
                            d.append(&line);
                        }
                    }
                    lines.push(line);
                }
                Event::Finished(r) => {
                    result = r;
                    break;
                }
            }
        }
        ctx.busy.set(false);
        if let Some(d) = &dialog {
            d.finish(&result);
        }
        on_done(&ctx, Outcome { result, lines });
    });
    true
}

struct Dialog {
    dialog: adw::Dialog,
    status: gtk::Label,
    progress: gtk::ProgressBar,
    buffer: gtk::TextBuffer,
    view: gtk::TextView,
    close: gtk::Button,
}

impl Dialog {
    fn new(title: &str, actions: &[Action]) -> Self {
        let status = gtk::Label::builder()
            .label("Waiting for authorization…")
            .wrap(true)
            .xalign(0.0)
            .css_classes(["heading"])
            .build();
        let steps = gtk::Label::builder()
            .label(
                actions
                    .iter()
                    .enumerate()
                    .map(|(i, a)| format!("{}. {}", i + 1, a.describe()))
                    .collect::<Vec<_>>()
                    .join("\n"),
            )
            .wrap(true)
            .xalign(0.0)
            .css_classes(["dim-label", "caption"])
            .build();
        let progress = gtk::ProgressBar::new();
        let view = gtk::TextView::builder()
            .editable(false)
            .cursor_visible(false)
            .monospace(true)
            .wrap_mode(gtk::WrapMode::WordChar)
            .css_classes(["log-view"])
            .build();
        let buffer = view.buffer();
        let scroller = gtk::ScrolledWindow::builder()
            .child(&view)
            .min_content_height(240)
            .vexpand(true)
            .css_classes(["card"])
            .build();
        let close = crate::ui::pill_button("Close", None);
        close.set_sensitive(false);
        close.set_halign(gtk::Align::Center);

        let body = gtk::Box::new(gtk::Orientation::Vertical, 12);
        body.set_margin_top(12);
        body.set_margin_bottom(18);
        body.set_margin_start(18);
        body.set_margin_end(18);
        body.append(&status);
        body.append(&steps);
        body.append(&progress);
        body.append(&scroller);
        body.append(&close);

        let tv = adw::ToolbarView::new();
        tv.add_top_bar(&adw::HeaderBar::new());
        tv.set_content(Some(&body));
        let dialog = adw::Dialog::builder()
            .title(title)
            .content_width(620)
            .content_height(560)
            .can_close(false)
            .child(&tv)
            .build();
        let d = dialog.clone();
        close.connect_clicked(move |_| {
            d.close();
        });
        Dialog {
            dialog,
            status,
            progress,
            buffer,
            view,
            close,
        }
    }

    fn step(&self, index: usize, total: usize, text: &str) {
        self.status
            .set_label(&format!("Step {index} of {total}: {text}"));
        self.progress
            .set_fraction((index.saturating_sub(1)) as f64 / total.max(1) as f64);
        self.append(&format!("\n▶ {text}"));
    }

    fn append(&self, line: &str) {
        let mut end = self.buffer.end_iter();
        self.buffer.insert(&mut end, &format!("{line}\n"));
        let mark = self
            .buffer
            .create_mark(None, &self.buffer.end_iter(), false);
        self.view.scroll_mark_onscreen(&mark);
        self.buffer.delete_mark(&mark);
    }

    fn finish(&self, result: &Result<(), String>) {
        match result {
            Ok(()) => {
                self.status.set_label("Done");
                self.progress.set_fraction(1.0);
            }
            Err(e) => {
                self.status.set_label(&format!("Failed: {e}"));
                self.status.add_css_class("error");
            }
        }
        self.dialog.set_can_close(true);
        self.close.set_sensitive(true);
        self.close.add_css_class("suggested-action");
        self.close.grab_focus();
    }
}
