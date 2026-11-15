//! The three screens and the entry point that draws them.

mod chrome;
mod launcher;
mod switcher;

/// Space the system keeps for itself (notch, home indicator), taken from egui's own
/// safe-area insets: the shell passes them in `RawInput::safe_area_insets`.
#[derive(Clone, Copy)]
pub(crate) struct Insets {
    pub top: f32,
    pub bottom: f32,
}

use egui::{Context, Ui};
use krypa_core::{InstallRefusal, InstallSource, ManualReason};

use crate::components::{
    external_open_dialog, install_dialog, modal_panel, permission_dialog, settings_choice_row,
    settings_toggle_row,
};
use crate::state::{display_origin_of, AppState, FrameOutput, Intent, Screen};
use crate::strings;
use crate::text::{display_text, MAX_DISPLAY_CHARS};
use crate::theme::{self, ThemeChoice, Tokens};

/// Longest page-controlled link shown in the "open another app" dialog, in characters.
const MAX_TARGET_CHARS: usize = 80;

/// Entry point of the interface.
pub struct KrypaUi;

impl KrypaUi {
    /// Draws one frame. Call it inside an egui pass (`Context::run_pass` /
    /// `Context::run_ui`), once per pass.
    pub fn frame(ctx: &Context, state: &mut AppState) -> FrameOutput {
        ctx.root_ui(|ui| Self::show(ui, state))
    }

    /// Same as [`KrypaUi::frame`] for callers that already hold the root `Ui` (eframe).
    pub fn show(ui: &mut Ui, state: &mut AppState) -> FrameOutput {
        let ctx = ui.ctx().clone();
        theme::apply(&ctx, state.theme, state.reduced_motion);
        let t = Tokens::for_theme(ctx.theme());
        let (view, content) = (ctx.viewport_rect(), ctx.content_rect());
        let insets = Insets {
            top: (content.min.y - view.min.y).max(0.0),
            bottom: (view.max.y - content.max.y).max(0.0),
        };
        let mut out = FrameOutput::default();

        let screen = match state.screen {
            Screen::Browser if state.foreground_info().is_none() => Screen::Launcher,
            s => s,
        };
        match screen {
            Screen::Launcher => launcher::show(ui, state, &t, insets, &mut out),
            Screen::Browser => chrome::show(ui, state, &t, insets, &mut out),
            Screen::Switcher => switcher::show(ui, state, &t, insets, &mut out),
        }

        if show_dialogs(&ctx, state, &t, &mut out) {
            // A modal blocks everything behind it, including the web view.
            out.covered_rects = vec![ctx.viewport_rect()];
        }
        out
    }
}

/// Draws the permission dialog or the settings panel. Returns true if one is open.
fn show_dialogs(ctx: &Context, state: &mut AppState, t: &Tokens, out: &mut FrameOutput) -> bool {
    if let Some(prompt) = state.permission_prompts.first().cloned() {
        let origin = display_origin_of(&prompt.origin).map_or_else(
            || strings::ORIGIN_UNKNOWN.to_owned(),
            |o| display_text(&o.to_string(), MAX_DISPLAY_CHARS),
        );
        let question = strings::permission_title(&origin, prompt.kind);
        let (decision, _rect) = permission_dialog(ctx, t, &question);
        if let Some(decision) = decision {
            state.permission_prompts.remove(0);
            out.intents.push(Intent::RespondPermission {
                session: prompt.session,
                request: prompt.request,
                decision,
            });
        }
        return true;
    }
    if let Some(prompt) = state.external_prompt.clone() {
        let origin = prompt
            .origin
            .as_ref()
            .and_then(display_origin_of)
            .map_or_else(
                || strings::ORIGIN_UNKNOWN.to_owned(),
                |o| display_text(&o.to_string(), MAX_DISPLAY_CHARS),
            );
        // The link is page-controlled: sanitized and cut, never shown whole.
        let target = strings::external_open_target(
            &display_text(prompt.url.scheme(), MAX_DISPLAY_CHARS),
            &display_text(prompt.url.as_str(), MAX_TARGET_CHARS),
        );
        let (answer, _rect) =
            external_open_dialog(ctx, t, &strings::external_open_title(&origin), &target);
        if let Some(allow) = answer {
            state.external_prompt = None;
            out.intents.push(Intent::RespondExternalOpen {
                session: prompt.session,
                allow,
            });
        }
        return true;
    }
    if let Some(prompt) = state.install_prompt.clone() {
        let origin = display_origin_of(prompt.app.start_url()).map_or_else(
            || strings::ORIGIN_UNKNOWN.to_owned(),
            |o| display_text(&o.to_string(), MAX_DISPLAY_CHARS),
        );
        let note = match prompt.source {
            InstallSource::Manifest => None,
            InstallSource::Manual(ManualReason::NoManifest) => {
                Some(strings::INSTALL_NOTE_NO_MANIFEST)
            }
            InstallSource::Manual(_) => Some(strings::INSTALL_NOTE_BAD_MANIFEST),
        };
        let title = strings::install_title(&display_text(prompt.app.name(), MAX_DISPLAY_CHARS));
        let (answer, _rect) =
            install_dialog(ctx, t, &title, &strings::install_origin(&origin), note);
        if let Some(confirmed) = answer {
            state.install_prompt = None;
            if confirmed {
                out.intents
                    .push(Intent::ConfirmInstall(Box::new(prompt.app)));
            }
        }
        return true;
    }
    if let Some(reason) = state.install_refusal {
        let body = match reason {
            InstallRefusal::NotHttps => strings::INSTALL_REFUSED_NOT_HTTPS,
            InstallRefusal::ConnectionNotSecure => strings::INSTALL_REFUSED_NOT_SECURE,
        };
        let panel = modal_panel(
            ctx,
            t,
            "krypa-install-refused",
            strings::INSTALL_REFUSED_TITLE,
            |ui| {
                ui.add_space(8.0);
                ui.add(egui::Label::new(egui::RichText::new(body).color(t.text_muted)).wrap());
            },
        );
        if panel.dismissed {
            state.install_refusal = None;
        }
        return true;
    }
    if state.settings_open {
        let mut changed = false;
        let panel = modal_panel(ctx, t, "krypa-settings", strings::SETTINGS_TITLE, |ui| {
            ui.add_space(8.0);
            changed |= settings_choice_row(
                ui,
                t,
                strings::SETTINGS_THEME,
                &[
                    (ThemeChoice::System, strings::THEME_SYSTEM),
                    (ThemeChoice::Light, strings::THEME_LIGHT),
                    (ThemeChoice::Dark, strings::THEME_DARK),
                ],
                &mut state.theme,
            );
            changed |= settings_toggle_row(
                ui,
                t,
                strings::SETTINGS_REDUCED_MOTION,
                &mut state.reduced_motion,
            );
        });
        if changed {
            out.intents.push(Intent::SettingsChanged);
        }
        if panel.dismissed {
            state.settings_open = false;
        }
        return true;
    }
    false
}
