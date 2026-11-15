//! App switcher: one card per session.

use egui::{vec2, Align, CentralPanel, Frame, Layout, Margin, Panel, RichText, ScrollArea, Ui};
use krypa_engine_api::ProfileKind;

use super::Insets;
use crate::components::{icon_button, switcher_card, text_button, ButtonKind, CardInfo};
use crate::icons::Icon;
use crate::state::{display_origin_of, session_url, AppState, FrameOutput, Intent, Screen};
use crate::strings;
use crate::theme::Tokens;

const CARD_HEIGHT: f32 = 132.0;

pub(super) fn show(
    ui: &mut Ui,
    state: &mut AppState,
    t: &Tokens,
    insets: Insets,
    out: &mut FrameOutput,
) {
    let top = Panel::top("krypa-switcher-top")
        .frame(
            Frame::new()
                .fill(t.surface)
                .inner_margin(Margin::symmetric(12, 6)),
        )
        .show(ui, |ui| {
            ui.add_space(insets.top);
            ui.horizontal(|ui| {
                ui.label(
                    RichText::new(strings::SWITCHER_TITLE)
                        .size(22.0)
                        .strong()
                        .color(t.text),
                );
                ui.with_layout(Layout::right_to_left(Align::Center), |ui| {
                    if state.foreground_info().is_some()
                        && text_button(ui, t, strings::SWITCHER_DONE, ButtonKind::Secondary)
                            .clicked()
                    {
                        state.screen = Screen::Browser;
                    }
                    if icon_button(ui, t, Icon::Plus, strings::SWITCHER_NEW, true).clicked() {
                        state.screen = Screen::Launcher;
                    }
                });
            });
        });
    out.covered_rects.push(top.response.rect);

    let central = CentralPanel::default()
        .frame(Frame::new().fill(t.bg).inner_margin(Margin::same(12)))
        .show(ui, |ui| {
            if state.sessions.is_empty() {
                ui.label(RichText::new(strings::SWITCHER_EMPTY).color(t.text_muted));
                return;
            }
            ScrollArea::vertical()
                .auto_shrink([false, false])
                .show(ui, |ui| {
                    grid(ui, state, t, out);
                });
        });
    out.covered_rects.push(central.response.rect);
}

fn grid(ui: &mut Ui, state: &mut AppState, t: &Tokens, out: &mut FrameOutput) {
    let gap = ui.spacing().item_spacing.x;
    let width = ui.available_width();
    let columns = (((width + gap) / (200.0 + gap)).floor() as usize).max(1);
    let card_w = (width - gap * (columns as f32 - 1.0)) / columns as f32;
    let mut open = None;
    let mut close = None;
    for row in state.sessions.chunks(columns) {
        ui.horizontal(|ui| {
            for info in row {
                let origin = session_url(info)
                    .and_then(display_origin_of)
                    .map_or_else(|| strings::ORIGIN_UNKNOWN.to_owned(), |o| o.to_string());
                let r = switcher_card(
                    ui,
                    t,
                    vec2(card_w, CARD_HEIGHT),
                    &CardInfo {
                        title: info.title.as_deref().unwrap_or(""),
                        origin: &origin,
                        state: info.state,
                        private: info.profile == ProfileKind::Private,
                        is_foreground: state.foreground == Some(info.id),
                    },
                );
                if r.open {
                    open = Some(info.id);
                }
                if r.close {
                    close = Some(info.id);
                }
            }
        });
    }
    if let Some(id) = open {
        out.intents.push(Intent::SwitchTo(id));
        state.screen = Screen::Browser;
    }
    if let Some(id) = close {
        out.intents.push(Intent::CloseSession(id));
    }
}
