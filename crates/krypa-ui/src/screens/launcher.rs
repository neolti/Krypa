//! Launcher: branding, address/search field, installed apps, recents, settings entry.

use egui::{pos2, vec2, CentralPanel, Frame, Margin, Rect, RichText, ScrollArea, Ui, UiBuilder};
use krypa_navigation::{classify_input, Input, NavigationError};

use super::Insets;
use crate::app_icons::choose_icon;
use crate::components::{address_field, app_card, icon_button, list_row, CardIcon};
use crate::icons::Icon;
use crate::state::{display_origin_of, AppState, FrameOutput, Intent, Screen};
use crate::strings;
use crate::theme::Tokens;

const COLUMN_WIDTH: f32 = 640.0;
const CARD_MIN_WIDTH: f32 = 104.0;
const CARD_HEIGHT: f32 = 124.0;
/// Diameter of the round icon badge on a card, in points (see `components::app_card`).
const ICON_BADGE_POINTS: f32 = 44.0;

pub(super) fn show(
    ui: &mut Ui,
    state: &mut AppState,
    t: &Tokens,
    insets: Insets,
    out: &mut FrameOutput,
) {
    let panel = CentralPanel::default()
        .frame(Frame::new().fill(t.bg).inner_margin(Margin::same(16)))
        .show(ui, |ui| {
            ui.add_space(insets.top);
            // Keep the content in a readable column on wide screens.
            let avail = ui.max_rect();
            let w = avail.width().min(COLUMN_WIDTH);
            let column = Rect::from_min_max(
                pos2(avail.center().x - w / 2.0, avail.min.y + insets.top),
                pos2(avail.center().x + w / 2.0, avail.max.y),
            );
            ui.scope_builder(UiBuilder::new().max_rect(column), |ui| {
                ScrollArea::vertical()
                    .auto_shrink([false, false])
                    .show(ui, |ui| {
                        content(ui, state, t, out);
                    });
            });
        });
    out.covered_rects.push(panel.response.rect);
}

fn content(ui: &mut Ui, state: &mut AppState, t: &Tokens, out: &mut FrameOutput) {
    ui.horizontal(|ui| {
        ui.vertical(|ui| {
            ui.label(
                RichText::new(strings::APP_NAME)
                    .size(30.0)
                    .strong()
                    .color(t.text),
            );
            ui.label(RichText::new(strings::TAGLINE).color(t.text_muted));
        });
        ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
            if icon_button(ui, t, Icon::Settings, strings::SETTINGS, true).clicked() {
                state.settings_open = true;
            }
            if !state.sessions.is_empty()
                && icon_button(ui, t, Icon::Pages, strings::SWITCHER, true).clicked()
            {
                state.screen = Screen::Switcher;
            }
        });
    });
    ui.add_space(12.0);

    let before = state.address_input.clone();
    let field = address_field(ui, t, &mut state.address_input);
    if state.address_input != before {
        state.address_error = None;
    }
    if field.submitted {
        submit(state, out);
    }
    if let Some(error) = state.address_error {
        ui.label(RichText::new(error).color(t.danger));
    }

    ui.add_space(16.0);
    ui.label(
        RichText::new(strings::SECTION_APPS)
            .size(18.0)
            .strong()
            .color(t.text),
    );
    if state.installed_apps.is_empty() {
        ui.label(RichText::new(strings::APPS_EMPTY).color(t.text_muted));
    } else {
        app_grid(ui, state, t, out);
    }

    ui.add_space(16.0);
    ui.label(
        RichText::new(strings::SECTION_RECENTS)
            .size(18.0)
            .strong()
            .color(t.text),
    );
    if state.recents.is_empty() {
        ui.label(RichText::new(strings::RECENTS_EMPTY).color(t.text_muted));
    }
    for recent in &state.recents {
        let host = display_origin_of(recent.url.as_url())
            .map_or_else(|| strings::ORIGIN_UNKNOWN.to_owned(), |o| o.to_string());
        if list_row(ui, t, &recent.title, &host).clicked() {
            out.intents.push(Intent::Navigate(recent.url.clone()));
            state.screen = Screen::Browser;
        }
    }
}

fn app_grid(ui: &mut Ui, state: &mut AppState, t: &Tokens, out: &mut FrameOutput) {
    let gap = ui.spacing().item_spacing.x;
    let width = ui.available_width();
    let columns = (((width + gap) / (CARD_MIN_WIDTH + gap)).floor() as usize).max(2);
    let card_w = (width - gap * (columns as f32 - 1.0)) / columns as f32;
    // Ask the host for icons nobody asked for yet, and put the decoded ones on the GPU.
    let target_px = (ICON_BADGE_POINTS * ui.ctx().pixels_per_point()).ceil() as u32;
    for url in state.icons.wanted(&state.installed_apps, target_px) {
        out.intents.push(Intent::FetchIcon(url));
    }
    state.icons.upload(ui.ctx());
    let mut opened = None;
    for row in state.installed_apps.chunks(columns) {
        ui.horizontal(|ui| {
            for app in row {
                let origin = display_origin_of(app.start_url())
                    .map_or_else(|| strings::ORIGIN_UNKNOWN.to_owned(), |o| o.to_string());
                let icon = choose_icon(app, target_px).and_then(|c| {
                    state.icons.texture(&c.url).map(|texture| CardIcon {
                        texture,
                        purpose: c.purpose,
                    })
                });
                if app_card(ui, t, vec2(card_w, CARD_HEIGHT), app.name(), &origin, icon).clicked() {
                    opened = Some(app.id().clone());
                }
            }
        });
    }
    if let Some(id) = opened {
        out.intents.push(Intent::OpenApp(id));
        state.screen = Screen::Browser;
    }
}

/// Turns the field's text into an intent using `krypa-navigation`. The UI never builds or
/// edits a URL itself.
fn submit(state: &mut AppState, out: &mut FrameOutput) {
    match classify_input(&state.address_input) {
        Ok(Input::Url(url)) => {
            out.intents.push(Intent::Navigate(url));
            state.screen = Screen::Browser;
        }
        Ok(Input::Search(query)) => {
            out.intents.push(Intent::Search(query));
            state.screen = Screen::Browser;
        }
        Ok(Input::External(link)) => out.intents.push(Intent::OpenExternal(link)),
        Err(NavigationError::Empty) => return,
        Err(NavigationError::EmbeddedCredentials) => {
            state.address_error = Some(strings::ADDRESS_ERROR_CREDENTIALS);
            return;
        }
        Err(NavigationError::BlockedScheme(_)) => {
            state.address_error = Some(strings::ADDRESS_ERROR_BLOCKED);
            return;
        }
        Err(NavigationError::TooLong) => {
            state.address_error = Some(strings::ADDRESS_ERROR_TOO_LONG);
            return;
        }
        Err(_) => {
            state.address_error = Some(strings::ADDRESS_ERROR_INVALID);
            return;
        }
    }
    state.address_input.clear();
    state.address_error = None;
}
