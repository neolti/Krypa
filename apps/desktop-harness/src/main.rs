//! Development-only desktop harness for `krypa-ui`.
//!
//! Runs the real `SessionManager` against a fake engine so the interface can be built and
//! checked on a desktop. The area where the web view would be is a placeholder. This is
//! not a product and ships nowhere.

use std::time::{Duration, Instant};

use eframe::egui::{self, Align2, Color32, FontId, Sense, Shape};
use krypa_core::MemoryPressure;
use krypa_engine_api::PermissionKind;
use krypa_storage::Storage;
use krypa_ui::KrypaUi;

use desktop_harness::wiring::Core;

/// Sample pages that exercise the interesting UI states.
const SAMPLES: [(&str, &str); 9] = [
    ("Secure page", "https://example.com/"),
    ("Plain HTTP page", "http://insecure.test/"),
    ("Certificate error", "https://badcert.test/"),
    ("Load failure", "https://fail.test/"),
    ("Page with a manifest", "https://shop.example/manifest/"),
    (
        "Page with a hostile manifest",
        "https://shop.example/manifest/hostile",
    ),
    (
        "Page with a broken manifest",
        "https://shop.example/badmanifest",
    ),
    ("Lookalike host (punycode)", "https://xn--pypal-4ve.com/"),
    (
        "Hostile title + camera prompt",
        "https://hostile.test/hostile/camera",
    ),
];

struct Harness {
    core: Core,
    page_touches: u32,
}

impl Harness {
    fn controls(&mut self, ui: &mut egui::Ui, now: Instant) {
        ui.heading("Harness (dev only)");
        ui.label("Fake engine, real SessionManager.");
        ui.separator();
        ui.label("Open a sample page:");
        for (label, url) in SAMPLES {
            if ui.button(label).clicked() {
                if let Ok(parsed) = url::Url::parse(url) {
                    if let Err(e) = self.core.open(parsed, None, now) {
                        self.core.status = format!("core refused: {e}");
                    }
                }
            }
        }
        ui.separator();
        if ui.button("Crash content process (foreground)").clicked() {
            self.core.crash_foreground(now);
        }
        if ui.button("Memory pressure: warning").clicked() {
            self.core.memory_pressure(MemoryPressure::Warning, now);
        }
        if ui.button("Memory pressure: critical").clicked() {
            self.core.memory_pressure(MemoryPressure::Critical, now);
        }
        if ui.button("Pause background views").clicked() {
            self.core.suspend_background(now);
        }
        if ui.button("Page asks for microphone").clicked() {
            self.core
                .request_permission(PermissionKind::Microphone, now);
        }
        if ui.button("Page taps a tel: link").clicked() {
            self.core.page_requests_external("tel:+4312345", true, now);
        }
        if ui.button("Page opens tel: with no tap").clicked() {
            self.core.page_requests_external("tel:+4312345", false, now);
        }
        if ui.button("Page tries javascript: (blocked)").clicked() {
            self.core
                .page_requests_external("javascript:alert(1)", true, now);
        }
        ui.separator();
        ui.label(format!(
            "live views: {} (engine: {})",
            self.core.manager.live_view_count(),
            self.core.engine.view_count()
        ));
        ui.label(format!(
            "touches that reached the page: {}",
            self.page_touches
        ));
        if !self.core.status.is_empty() {
            ui.label(format!("status: {}", self.core.status));
        }
    }
}

impl eframe::App for Harness {
    fn ui(&mut self, ui: &mut egui::Ui, _frame: &mut eframe::Frame) {
        let now = Instant::now();
        self.core.pump(now);

        egui::Panel::right("harness-controls")
            .exact_size(280.0)
            .show(ui, |ui| self.controls(ui, now));

        // Reserve a paint slot below everything KrypaUi draws, fill it once the web view
        // rectangle is known.
        let slot = ui.painter().add(Shape::Noop);
        let out = KrypaUi::show(ui, &mut self.core.state);
        self.core.handle_intents(out.intents.clone(), now);

        if let Some(rect) = out.web_view_rect {
            let overlay = out.is_covered(rect.center());
            if !overlay {
                let shape = Shape::rect_filled(rect, 0.0, Color32::from_rgb(0x2B, 0x2F, 0x3A));
                ui.painter().set(slot, shape);
                let text = self.core.state.foreground_info().map_or_else(
                    || "web view".to_owned(),
                    |i| {
                        let host = i
                            .current_url
                            .as_ref()
                            .and_then(|u| u.host_str())
                            .unwrap_or("-");
                        format!("web view placeholder\nsession {}\n{host}", i.id.get())
                    },
                );
                ui.painter().text(
                    rect.center(),
                    Align2::CENTER_CENTER,
                    text,
                    FontId::proportional(16.0),
                    Color32::from_rgb(0xD0, 0xD4, 0xDC),
                );
                // A touch here is routed to the page, exactly as the iOS shell will do.
                if ui
                    .interact(
                        rect,
                        egui::Id::unique("web-view-placeholder"),
                        Sense::click(),
                    )
                    .clicked()
                {
                    self.page_touches += 1;
                }
            }
        }

        if self.core.engine.busy() || self.core.icons_pending() || out.animating {
            ui.ctx().request_repaint_after(Duration::from_millis(33));
        }
    }
}

fn main() -> eframe::Result<()> {
    let options = eframe::NativeOptions {
        viewport: egui::ViewportBuilder::default()
            .with_title("Krypa desktop harness")
            .with_inner_size([390.0 + 280.0, 844.0]),
        ..eframe::NativeOptions::default()
    };
    let mut core = Core::new().map_err(|e| eframe::Error::AppCreation(Box::new(e)))?;
    // Installed apps live in a database so they survive a restart. Dev-only location,
    // overridable; a harness that cannot open it still runs, and says so.
    let db_path = std::env::var_os("KRYPA_HARNESS_DB").map_or_else(
        || std::env::temp_dir().join("krypa-desktop-harness.sqlite3"),
        std::path::PathBuf::from,
    );
    match Storage::open(&db_path) {
        Ok(storage) => core.attach_storage(storage),
        Err(e) => {
            core.status = format!("no storage, installs will not persist: {e}");
            eprintln!("{}", core.status);
        }
    }
    for problem in &core.startup_problems {
        eprintln!("startup: {problem}");
    }
    eframe::run_native(
        "Krypa desktop harness",
        options,
        Box::new(|_cc| {
            Ok(Box::new(Harness {
                core,
                page_touches: 0,
            }))
        }),
    )
}
