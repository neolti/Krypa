//! Browser chrome: toolbar with origin and security indicator, navigation buttons, and the
//! area where the web view sits.

use egui::{vec2, Align, CentralPanel, Frame, Label, Layout, Margin, Panel, RichText, Sense, Ui};
use krypa_core::{FailureReason, SessionInfo, SessionState};
use krypa_engine_api::LoadErrorKind;
use krypa_navigation::SecurityState;

use super::Insets;
use crate::components::{error_view, icon_button, loading_bar, loading_indicator};
use crate::icons::{paint_icon, Icon};
use crate::state::{
    display_origin_of, security_for, session_url, AppState, FrameOutput, Intent, Screen,
};
use crate::strings;
use crate::text::display_text;
use crate::theme::Tokens;

pub(super) fn show(
    ui: &mut Ui,
    state: &mut AppState,
    t: &Tokens,
    insets: Insets,
    out: &mut FrameOutput,
) {
    let Some(info) = state.foreground_info().cloned() else {
        return;
    };
    let id = info.id;
    let security = security_for(&info);
    let loading = is_loading(&info);
    let can_navigate = matches!(
        info.state,
        SessionState::Loading | SessionState::Foreground | SessionState::Failed
    );

    let safe = insets;
    let top = Panel::top("krypa-chrome-top")
        .frame(Frame::new().fill(t.surface).inner_margin(Margin {
            left: 12,
            right: 4,
            top: (6.0 + safe.top) as i8,
            bottom: 4,
        }))
        .show(ui, |ui| {
            ui.horizontal(|ui| {
                origin_block(ui, &info, security, t);
                ui.with_layout(Layout::right_to_left(Align::Center), |ui| {
                    if loading {
                        if icon_button(ui, t, Icon::Cross, strings::STOP, can_navigate).clicked() {
                            out.intents.push(Intent::Stop(id));
                        }
                    } else if icon_button(ui, t, Icon::Reload, strings::RELOAD, can_navigate)
                        .clicked()
                    {
                        out.intents.push(Intent::Reload(id));
                    }
                });
            });
            if loading {
                loading_bar(ui, t, info.progress_permille);
            }
        });
    out.covered_rects.push(top.response.rect);

    let already_added =
        session_url(&info).is_some_and(|u| state.installed_apps.iter().any(|a| a.is_in_scope(u)));
    // An engine that cannot fetch the manifest never answers "Add to Krypa": not offered.
    let installable = state.capabilities.manifest_fetch
        && session_url(&info).is_some()
        && info.state.has_live_view();
    let bottom = Panel::bottom("krypa-chrome-bottom")
        .frame(Frame::new().fill(t.surface).inner_margin(Margin {
            left: 8,
            right: 8,
            top: 4,
            bottom: (4.0 + safe.bottom) as i8,
        }))
        .show(ui, |ui| {
            ui.columns(5, |cols| {
                if icon_button(
                    &mut cols[0],
                    t,
                    Icon::Back,
                    strings::BACK,
                    can_navigate && info.can_go_back,
                )
                .clicked()
                {
                    out.intents.push(Intent::GoBack(id));
                }
                if icon_button(
                    &mut cols[1],
                    t,
                    Icon::Forward,
                    strings::FORWARD,
                    can_navigate && info.can_go_forward,
                )
                .clicked()
                {
                    out.intents.push(Intent::GoForward(id));
                }
                let (icon, label, enabled) = if already_added {
                    (Icon::Plus, strings::ALREADY_ADDED, false)
                } else {
                    (Icon::Plus, strings::ADD_TO_KRYPA, installable)
                };
                if icon_button(&mut cols[2], t, icon, label, enabled).clicked() {
                    out.intents.push(Intent::AddToKrypa(id));
                }
                if icon_button(&mut cols[3], t, Icon::Home, strings::HOME, true).clicked() {
                    state.screen = Screen::Launcher;
                }
                if icon_button(&mut cols[4], t, Icon::Pages, strings::SWITCHER, true).clicked() {
                    state.screen = Screen::Switcher;
                }
            });
        });
    out.covered_rects.push(bottom.response.rect);

    // The web view area. Normally empty: the web view sits here. Error and loading views
    // are drawn over it, so they count as covered.
    let mut covered = true;
    let reduced = state.reduced_motion;
    let mut animating = false;
    let central = CentralPanel::default()
        .frame(Frame::NONE)
        .show(ui, |ui| match &info.failure {
            Some(reason) if info.state == SessionState::Failed => {
                ui.painter().rect_filled(ui.max_rect(), 0.0, t.bg);
                let (title, body) = failure_text(*reason);
                if error_view(ui, t, title, body, strings::RETRY) {
                    out.intents.push(Intent::Reload(id));
                }
            }
            _ if matches!(
                info.state,
                SessionState::NotRunning | SessionState::Terminated
            ) =>
            {
                ui.painter().rect_filled(ui.max_rect(), 0.0, t.bg);
                loading_indicator(ui, t, reduced, strings::RESTORING);
                animating = !reduced;
            }
            _ => covered = false,
        });
    let web_rect = central.response.rect;
    out.web_view_rect = Some(web_rect);
    out.animating = animating;
    if covered {
        out.covered_rects.push(web_rect);
    }
}

fn is_loading(info: &SessionInfo) -> bool {
    info.state == SessionState::Loading
        || (info.state.has_live_view()
            && info.state != SessionState::Failed
            && info.progress_permille > 0
            && info.progress_permille < 1000)
}

fn failure_text(reason: FailureReason) -> (&'static str, &'static str) {
    match reason {
        FailureReason::ContentProcessTerminated => {
            (strings::ERROR_CRASH_TITLE, strings::ERROR_CRASH)
        }
        FailureReason::LoadFailed(kind) => (
            strings::ERROR_LOAD_TITLE,
            match kind {
                LoadErrorKind::Network => strings::ERROR_NETWORK,
                LoadErrorKind::Tls => strings::ERROR_TLS,
                LoadErrorKind::Blocked => strings::ERROR_BLOCKED,
                LoadErrorKind::Cancelled | LoadErrorKind::Other => strings::ERROR_OTHER,
            },
        ),
    }
}

/// Security indicator and origin. The origin is the `DisplayOrigin` from
/// `krypa-navigation` in full: nothing is cut, so the real site name cannot be hidden
/// behind an ellipsis, and a long host wraps instead.
fn origin_block(ui: &mut Ui, info: &SessionInfo, security: Option<SecurityState>, t: &Tokens) {
    let origin = session_url(info).and_then(display_origin_of);
    let (icon, text, color) = match security {
        None => (Icon::Unverified, strings::SECURITY_UNVERIFIED, t.text_muted),
        Some(SecurityState::Secure) => (Icon::Lock, strings::SECURITY_SECURE, t.success),
        Some(SecurityState::Insecure) => (Icon::Warning, strings::SECURITY_INSECURE, t.warning),
        Some(SecurityState::CertificateError) => {
            (Icon::Warning, strings::SECURITY_CERTIFICATE, t.danger)
        }
    };
    ui.vertical(|ui| {
        ui.set_max_width((ui.available_width() - 52.0).max(80.0));
        ui.horizontal(|ui| {
            let (rect, _) = ui.allocate_exact_size(vec2(16.0, 16.0), Sense::hover());
            paint_icon(ui.painter(), rect, icon, color);
            ui.label(RichText::new(text).size(13.0).color(color));
        });
        match &origin {
            Some(o) => {
                ui.add(
                    Label::new(
                        RichText::new(display_text(&o.to_string(), 300))
                            .size(17.0)
                            .strong()
                            .color(t.text),
                    )
                    .wrap(),
                );
                if o.shown_as_punycode {
                    ui.label(
                        RichText::new(strings::PUNYCODE_NOTE)
                            .size(12.0)
                            .color(t.text_muted),
                    );
                }
            }
            None => {
                ui.label(
                    RichText::new(strings::ORIGIN_UNKNOWN)
                        .size(17.0)
                        .color(t.text),
                );
            }
        }
    });
}
