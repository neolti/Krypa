//! Reusable building blocks. Every interactive component is at least [`MIN_TOUCH`] points
//! in both directions, carries an accessible name, and renders web-derived text only
//! through [`crate::text`].

use egui::accesskit::Role;
use egui::epaint::Vertex;
use egui::{
    vec2, Align, Align2, Color32, Context, CornerRadius, FontId, Frame, Id, Key, Layout, Margin,
    Mesh, Modal, Rect, RichText, Sense, Shape, Stroke, StrokeKind, TextEdit, TextureId, Ui,
    WidgetInfo,
};
use krypa_apps::IconPurpose;

use crate::app_icons::icon_layout;
use crate::icons::{paint_icon, Icon};
use crate::strings;
use crate::text::{display_text, one_line, MAX_DISPLAY_CHARS};
use crate::theme::{Tokens, MIN_TOUCH, RADIUS};
use krypa_core::SessionState;

fn radius() -> CornerRadius {
    CornerRadius::same(RADIUS)
}

/// A square icon-only button with an accessible name.
pub fn icon_button(
    ui: &mut Ui,
    t: &Tokens,
    icon: Icon,
    label: &str,
    enabled: bool,
) -> egui::Response {
    let sense = if enabled {
        Sense::click()
    } else {
        Sense::hover()
    };
    let (rect, resp) = ui.allocate_exact_size(vec2(MIN_TOUCH, MIN_TOUCH), sense);
    if ui.is_rect_visible(rect) {
        let p = ui.painter();
        if enabled && (resp.is_pointer_button_down_on() || resp.hovered()) {
            p.rect_filled(rect.shrink(2.0), radius(), t.surface_alt);
        }
        let color = if enabled { t.text } else { t.text_muted };
        paint_icon(p, rect.shrink(11.0), icon, color);
        if resp.has_focus() {
            p.rect_stroke(
                rect,
                radius(),
                Stroke::new(2.0, t.accent),
                StrokeKind::Inside,
            );
        }
    }
    resp.widget_info(|| WidgetInfo::labeled(Role::Button, enabled, label));
    resp
}

/// Look of a [`text_button`].
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ButtonKind {
    /// Filled with the accent color.
    Primary,
    /// Outlined.
    Secondary,
}

/// A text button, at least 44 points high.
pub fn text_button(ui: &mut Ui, t: &Tokens, label: &str, kind: ButtonKind) -> egui::Response {
    let (fill, fg) = match kind {
        ButtonKind::Primary => (t.accent, t.on_accent),
        ButtonKind::Secondary => (t.surface, t.text),
    };
    ui.add(
        egui::Button::new(RichText::new(label).color(fg).strong())
            .fill(fill)
            .stroke(Stroke::new(1.0, t.border))
            .corner_radius(radius())
            .min_size(vec2(MIN_TOUCH, MIN_TOUCH)),
    )
}

/// Outcome of an [`address_field`] frame.
pub struct AddressFieldResult {
    /// The user asked to submit the text (Enter or the Go button).
    pub submitted: bool,
}

/// The search/address field with its Go button. The caller owns the text and decides what
/// it means; the field never interprets it.
pub fn address_field(ui: &mut Ui, t: &Tokens, text: &mut String) -> AddressFieldResult {
    let mut submitted = false;
    ui.horizontal(|ui| {
        let go_width = 64.0;
        let field_width = (ui.available_width() - go_width - ui.spacing().item_spacing.x).max(80.0);
        let resp = ui.add_sized(
            [field_width, 48.0],
            TextEdit::singleline(text)
                .id(Id::unique("krypa-address-field"))
                .hint_text(strings::ADDRESS_FIELD_LABEL)
                .margin(Margin::symmetric(14, 12))
                .font(FontId::proportional(17.0))
                .align(Align2::LEFT_CENTER),
        );
        // The hint is only a placeholder; give assistive technology a real name.
        ui.ctx().accesskit_node_builder(resp.id, |node| {
            node.set_label(strings::ADDRESS_FIELD_LABEL);
        });
        if resp.lost_focus() && ui.input(|i| i.key_pressed(Key::Enter)) {
            submitted = true;
        }
        let go = ui.add_sized(
            [go_width, 48.0],
            egui::Button::new(
                RichText::new(strings::ADDRESS_GO)
                    .color(t.on_accent)
                    .strong(),
            )
            .fill(t.accent)
            .corner_radius(radius()),
        );
        submitted |= go.clicked();
    });
    AddressFieldResult { submitted }
}

/// The icon of an app card: an uploaded texture and how the manifest says it may be used.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct CardIcon {
    /// The uploaded image.
    pub texture: TextureId,
    /// `Any` or `Maskable` (see `app_icons::icon_layout`).
    pub purpose: IconPurpose,
}

const BADGE_SEGMENTS: u32 = 40;

/// Paints an app icon into a round badge at `center`. Decoration only: the card's
/// accessible name never depends on it.
fn paint_card_icon(p: &egui::Painter, t: &Tokens, center: egui::Pos2, radius: f32, icon: CardIcon) {
    let layout = icon_layout(icon.purpose);
    if layout.masked {
        // The mask is the badge circle: a triangle fan whose texture coordinates are the
        // safe-zone rectangle.
        let mut mesh = Mesh::with_texture(icon.texture);
        let uv_center = layout.uv.center();
        mesh.vertices.push(Vertex {
            pos: center,
            uv: uv_center,
            color: Color32::WHITE,
        });
        for i in 0..BADGE_SEGMENTS {
            let angle = std::f32::consts::TAU * i as f32 / BADGE_SEGMENTS as f32;
            let dir = vec2(angle.cos(), angle.sin());
            mesh.vertices.push(Vertex {
                pos: center + dir * radius,
                uv: uv_center + dir * (layout.uv.size() * 0.5),
                color: Color32::WHITE,
            });
        }
        for i in 0..BADGE_SEGMENTS {
            mesh.add_triangle(0, 1 + i, 1 + (i + 1) % BADGE_SEGMENTS);
        }
        p.add(Shape::mesh(mesh));
    } else {
        p.circle_filled(center, radius, t.surface_alt);
        let side = radius * 2.0 * layout.extent;
        let mut mesh = Mesh::with_texture(icon.texture);
        mesh.add_rect_with_uv(
            Rect::from_center_size(center, vec2(side, side)),
            layout.uv,
            Color32::WHITE,
        );
        p.add(Shape::mesh(mesh));
    }
}

/// Installed-app tile: the app icon (or, while it loads and if it fails, a monogram), the
/// app name and its origin. Returns the click response.
pub fn app_card(
    ui: &mut Ui,
    t: &Tokens,
    size: egui::Vec2,
    name: &str,
    origin: &str,
    icon: Option<CardIcon>,
) -> egui::Response {
    let name = display_text(name, MAX_DISPLAY_CHARS);
    let origin = display_text(origin, MAX_DISPLAY_CHARS);
    let (rect, resp) = ui.allocate_exact_size(size, Sense::click());
    let p = ui.painter();
    let fill = if resp.hovered() {
        t.surface_alt
    } else {
        t.surface
    };
    p.rect_filled(rect, radius(), fill);
    p.rect_stroke(
        rect,
        radius(),
        Stroke::new(1.0, t.border),
        StrokeKind::Inside,
    );
    let badge =
        Rect::from_center_size(rect.center_top() + vec2(0.0, 12.0 + 22.0), vec2(44.0, 44.0));
    if let Some(icon) = icon {
        paint_card_icon(p, t, badge.center(), 22.0, icon);
    } else {
        p.circle_filled(badge.center(), 22.0, t.accent);
        let initial = name
            .chars()
            .next()
            .map_or_else(String::new, |c| c.to_uppercase().collect());
        p.text(
            badge.center(),
            Align2::CENTER_CENTER,
            initial,
            FontId::proportional(22.0),
            t.on_accent,
        );
    }
    let w = rect.width() - 16.0;
    let g = one_line(ui, &name, FontId::proportional(15.0), t.text, w);
    let y = badge.bottom() + 8.0;
    ui.painter().galley(
        egui::pos2(rect.center().x - g.size().x / 2.0, y),
        g.clone(),
        t.text,
    );
    let g2 = one_line(ui, &origin, FontId::proportional(12.0), t.text_muted, w);
    ui.painter().galley(
        egui::pos2(rect.center().x - g2.size().x / 2.0, y + g.size().y + 2.0),
        g2,
        t.text_muted,
    );
    let label = strings::app_card_label(&name, &origin);
    resp.widget_info(|| WidgetInfo::labeled(Role::Button, true, &label));
    resp
}

/// What a [`switcher_card`] shows.
pub struct CardInfo<'a> {
    /// Page title (hostile; sanitized here).
    pub title: &'a str,
    /// Origin text, already produced by `DisplayOrigin`.
    pub origin: &'a str,
    /// Lifecycle state, which decides the look.
    pub state: SessionState,
    /// A private session.
    pub private: bool,
    /// The session on screen.
    pub is_foreground: bool,
}

/// What the user did with a switcher card.
#[derive(Debug, Default, Clone, Copy, PartialEq, Eq)]
pub struct CardResponse {
    /// Open this session.
    pub open: bool,
    /// Close this session.
    pub close: bool,
}

/// A card for the app switcher. Live, paused and terminated views look different on
/// purpose: a live view has a solid accent border and a filled dot, a paused one a pause
/// glyph, a terminated one a dashed border and a hollow dot. The state is also spelled
/// out in words and in the accessible name, never carried by color alone.
pub fn switcher_card(
    ui: &mut Ui,
    t: &Tokens,
    size: egui::Vec2,
    info: &CardInfo<'_>,
) -> CardResponse {
    let title = if info.title.trim().is_empty() {
        strings::UNTITLED.to_owned()
    } else {
        display_text(info.title, MAX_DISPLAY_CHARS)
    };
    let origin = display_text(info.origin, MAX_DISPLAY_CHARS);
    let (rect, resp) = ui.allocate_exact_size(size, Sense::click());
    let live = info.state.has_live_view();
    let terminated = !live;
    let fill = if terminated { t.bg } else { t.surface };
    let p = ui.painter();
    p.rect_filled(rect, radius(), fill);
    if terminated {
        let r = rect.shrink(1.0);
        let pts = [
            r.left_top(),
            r.right_top(),
            r.right_bottom(),
            r.left_bottom(),
            r.left_top(),
        ];
        p.extend(Shape::dashed_line(
            &pts,
            Stroke::new(1.5, t.border),
            6.0,
            4.0,
        ));
    } else {
        let width = if info.is_foreground { 3.0 } else { 1.5 };
        let color = if info.is_foreground {
            t.accent
        } else {
            t.border
        };
        p.rect_stroke(
            rect,
            radius(),
            Stroke::new(width, color),
            StrokeKind::Inside,
        );
    }
    let pad = 14.0;
    let text_w = rect.width() - 2.0 * pad - MIN_TOUCH;
    let g = one_line(ui, &title, FontId::proportional(16.0), t.text, text_w);
    let title_h = g.size().y;
    ui.painter().galley(rect.min + vec2(pad, pad), g, t.text);
    let g = one_line(
        ui,
        &origin,
        FontId::proportional(13.0),
        t.text_muted,
        rect.width() - 2.0 * pad,
    );
    let origin_y = rect.min.y + pad + title_h + 4.0;
    ui.painter().galley(
        egui::pos2(rect.min.x + pad, origin_y),
        g.clone(),
        t.text_muted,
    );

    // State badge at the bottom: glyph plus words.
    let badge_y = rect.max.y - pad - 18.0;
    let glyph = Rect::from_min_size(egui::pos2(rect.min.x + pad, badge_y), vec2(18.0, 18.0));
    let p = ui.painter();
    match info.state {
        SessionState::Foreground | SessionState::Loading | SessionState::Background => {
            p.circle_filled(glyph.center(), 6.0, t.success);
        }
        SessionState::Suspended => paint_icon(p, glyph, Icon::Pause, t.warning),
        SessionState::Failed => paint_icon(p, glyph, Icon::Warning, t.danger),
        SessionState::Terminated | SessionState::NotRunning => {
            p.circle_stroke(glyph.center(), 6.0, Stroke::new(2.0, t.border));
        }
    }
    let mut badge = strings::state_badge(info.state).to_owned();
    if info.private {
        badge.push_str(" \u{00B7} ");
        badge.push_str(strings::PRIVATE_TAG);
    }
    let g = one_line(
        ui,
        &badge,
        FontId::proportional(13.0),
        t.text,
        rect.width() - 2.0 * pad - 26.0,
    );
    ui.painter()
        .galley(egui::pos2(glyph.max.x + 8.0, badge_y), g, t.text);

    let label = strings::switcher_card_label(&title, &origin, info.state, info.private);
    resp.widget_info(|| WidgetInfo::labeled(Role::Button, true, &label));
    let open = resp.clicked();

    let close_rect = Rect::from_min_size(
        egui::pos2(rect.max.x - MIN_TOUCH - 4.0, rect.min.y + 4.0),
        vec2(MIN_TOUCH, MIN_TOUCH),
    );
    let close = ui
        .put(close_rect, |ui: &mut Ui| {
            icon_button(ui, t, Icon::Cross, &strings::close_page_label(&title), true)
        })
        .clicked();
    CardResponse {
        open: open && !close,
        close,
    }
}

/// A thin determinate progress bar for page loads.
pub fn loading_bar(ui: &mut Ui, t: &Tokens, permille: u16) {
    let (rect, resp) = ui.allocate_exact_size(vec2(ui.available_width(), 4.0), Sense::hover());
    let p = ui.painter();
    p.rect_filled(rect, 0.0, t.surface_alt);
    let frac = f32::from(permille.min(1000)) / 1000.0;
    p.rect_filled(
        Rect::from_min_size(rect.min, vec2(rect.width() * frac, rect.height())),
        0.0,
        t.accent,
    );
    resp.widget_info(|| WidgetInfo {
        value: Some(f64::from(permille.min(1000)) / 10.0),
        ..WidgetInfo::labeled(Role::ProgressIndicator, true, strings::LOADING)
    });
}

/// Centered "loading" state. With `reduced_motion` it is static text; otherwise a spinner.
pub fn loading_indicator(ui: &mut Ui, t: &Tokens, reduced_motion: bool, label: &str) {
    ui.vertical_centered(|ui| {
        ui.add_space(ui.available_height() * 0.3);
        if !reduced_motion {
            ui.add(egui::Spinner::new().color(t.accent).size(32.0));
        }
        ui.add_space(8.0);
        ui.label(RichText::new(label).color(t.text_muted));
    });
}

/// Full-area error message with one retry button. Returns true when retry was pressed.
pub fn error_view(ui: &mut Ui, t: &Tokens, title: &str, body: &str, retry: &str) -> bool {
    let mut pressed = false;
    ui.vertical_centered(|ui| {
        ui.add_space(ui.available_height() * 0.25);
        let (rect, _) = ui.allocate_exact_size(vec2(40.0, 40.0), Sense::hover());
        paint_icon(ui.painter(), rect, Icon::Warning, t.danger);
        ui.add_space(8.0);
        ui.label(RichText::new(title).size(20.0).strong().color(t.text));
        ui.label(RichText::new(body).color(t.text_muted));
        ui.add_space(12.0);
        pressed = text_button(ui, t, retry, ButtonKind::Primary).clicked();
    });
    pressed
}

/// A settings row: label on the left, a control on the right, at least 44 points high.
pub fn settings_row<R>(
    ui: &mut Ui,
    t: &Tokens,
    label: &str,
    control: impl FnOnce(&mut Ui) -> R,
) -> R {
    Frame::new()
        .fill(t.surface)
        .stroke(Stroke::new(1.0, t.border))
        .corner_radius(radius())
        .inner_margin(Margin::symmetric(14, 4))
        .show(ui, |ui| {
            ui.set_min_height(MIN_TOUCH);
            ui.set_width(ui.available_width());
            ui.with_layout(Layout::left_to_right(Align::Center), |ui| {
                ui.label(RichText::new(label).color(t.text));
                ui.with_layout(Layout::right_to_left(Align::Center), control)
                    .inner
            })
            .inner
        })
        .inner
}

/// A settings row with an on/off switch. Returns true when the value changed.
pub fn settings_toggle_row(ui: &mut Ui, t: &Tokens, label: &str, value: &mut bool) -> bool {
    settings_row(ui, t, label, |ui| {
        let (rect, mut resp) = ui.allocate_exact_size(vec2(56.0, MIN_TOUCH), Sense::click());
        if resp.clicked() {
            *value = !*value;
            resp.mark_changed();
        }
        let track = Rect::from_center_size(rect.center(), vec2(52.0, 30.0));
        let p = ui.painter();
        p.rect_filled(track, 15.0, if *value { t.accent } else { t.surface_alt });
        p.rect_stroke(track, 15.0, Stroke::new(1.5, t.border), StrokeKind::Inside);
        let knob_x = if *value {
            track.max.x - 15.0
        } else {
            track.min.x + 15.0
        };
        p.circle_filled(
            egui::pos2(knob_x, track.center().y),
            10.0,
            if *value { t.on_accent } else { t.text },
        );
        let on = *value;
        resp.widget_info(|| WidgetInfo::selected(Role::Switch, true, on, label));
        resp.changed()
    })
}

/// A settings row with a segmented choice. Returns true when the selection changed.
pub fn settings_choice_row<T: Copy + PartialEq>(
    ui: &mut Ui,
    t: &Tokens,
    label: &str,
    options: &[(T, &str)],
    current: &mut T,
) -> bool {
    let mut changed = false;
    settings_row(ui, t, label, |ui| {
        for (value, name) in options.iter().rev() {
            let selected = *current == *value;
            let (fill, fg) = if selected {
                (t.accent, t.on_accent)
            } else {
                (t.surface, t.text)
            };
            let r = ui.add(
                egui::Button::new(RichText::new(*name).color(fg))
                    .fill(fill)
                    .stroke(Stroke::new(1.0, t.border))
                    .corner_radius(radius())
                    .selected(selected)
                    .min_size(vec2(MIN_TOUCH, MIN_TOUCH)),
            );
            if r.clicked() && !selected {
                *current = *value;
                changed = true;
            }
        }
    });
    changed
}

/// Result of a [`modal_panel`] frame.
pub struct ModalOutput<R> {
    /// What the content closure returned.
    pub inner: R,
    /// The panel's own rectangle.
    pub rect: Rect,
    /// The user dismissed it (close button, Escape, or a tap outside).
    pub dismissed: bool,
}

fn modal_frame(t: &Tokens) -> Frame {
    Frame::new()
        .fill(t.surface)
        .stroke(Stroke::new(1.0, t.border))
        .corner_radius(radius())
        .inner_margin(Margin::same(18))
}

/// A centered modal panel with a title and a close button. Input outside it is blocked.
pub fn modal_panel<R>(
    ctx: &Context,
    t: &Tokens,
    id: &str,
    title: &str,
    content: impl FnOnce(&mut Ui) -> R,
) -> ModalOutput<R> {
    let mut close_clicked = false;
    let response = Modal::new(Id::unique(id))
        .frame(modal_frame(t))
        .show(ctx, |ui| {
            ui.set_max_width(360.0);
            ui.horizontal(|ui| {
                ui.label(RichText::new(title).size(20.0).strong().color(t.text));
                ui.with_layout(Layout::right_to_left(Align::Center), |ui| {
                    close_clicked = icon_button(ui, t, Icon::Cross, strings::CLOSE, true).clicked();
                });
            });
            content(ui)
        });
    let dismissed = close_clicked || response.should_close();
    ModalOutput {
        rect: response.response.rect,
        inner: response.inner,
        dismissed,
    }
}

/// A permission dialog. `origin` and `question` are shown as plain text. Returns the
/// user's decision, if any, and the dialog rectangle. Escape or a tap outside denies,
/// because "no" is the safe default.
pub fn permission_dialog(
    ctx: &Context,
    t: &Tokens,
    question: &str,
) -> (Option<krypa_engine_api::PermissionDecision>, Rect) {
    use krypa_engine_api::PermissionDecision;
    let mut decision = None;
    let response = Modal::new(Id::unique("krypa-permission-dialog"))
        .frame(modal_frame(t))
        .show(ctx, |ui| {
            ui.set_max_width(340.0);
            ui.add(
                egui::Label::new(RichText::new(question).size(18.0).strong().color(t.text)).wrap(),
            );
            ui.add_space(12.0);
            if text_button(ui, t, strings::PERMISSION_ALLOW, ButtonKind::Primary).clicked() {
                decision = Some(PermissionDecision::Grant);
            }
            if text_button(ui, t, strings::PERMISSION_DENY, ButtonKind::Secondary).clicked() {
                decision = Some(PermissionDecision::Deny);
            }
        });
    if decision.is_none() && response.should_close() {
        decision = Some(PermissionDecision::Deny);
    }
    (decision, response.response.rect)
}

/// A yes/no dialog whose safe answer is "no": Cancel is listed first, drawn as the primary
/// button and holds keyboard focus when the dialog opens (so Enter cancels), and Escape or a
/// tap outside also cancel. `details` are plain-text lines under the title. Returns the
/// answer, if any (`true` = confirm), and the dialog rectangle.
fn confirm_dialog(
    ctx: &Context,
    t: &Tokens,
    id: &'static str,
    title: &str,
    details: &[&str],
    confirm: &str,
    cancel: &str,
) -> (Option<bool>, Rect) {
    let mut answer = None;
    let response = Modal::new(Id::unique(id))
        .frame(modal_frame(t))
        .show(ctx, |ui| {
            ui.set_max_width(340.0);
            ui.add(egui::Label::new(RichText::new(title).size(18.0).strong().color(t.text)).wrap());
            for line in details {
                ui.add_space(6.0);
                ui.add(egui::Label::new(RichText::new(*line).color(t.text_muted)).wrap());
            }
            ui.add_space(12.0);
            let cancel_button = text_button(ui, t, cancel, ButtonKind::Primary);
            if ui.memory(|m| m.focused().is_none()) {
                cancel_button.request_focus();
            }
            if cancel_button.clicked() {
                answer = Some(false);
            }
            if text_button(ui, t, confirm, ButtonKind::Secondary).clicked() {
                answer = Some(true);
            }
        });
    if answer.is_none() && response.should_close() {
        answer = Some(false);
    }
    (answer, response.response.rect)
}

/// The "this page wants to open another app" dialog. `title` names the asking page,
/// `target` is the sanitized, shortened link. Returns `Some(true)` only for an explicit tap
/// on Open.
pub fn external_open_dialog(
    ctx: &Context,
    t: &Tokens,
    title: &str,
    target: &str,
) -> (Option<bool>, Rect) {
    confirm_dialog(
        ctx,
        t,
        "krypa-external-open-dialog",
        title,
        &[target],
        strings::EXTERNAL_OPEN_CONFIRM,
        strings::EXTERNAL_OPEN_CANCEL,
    )
}

/// The install confirmation: what will be added, from where, and a note when it is only a
/// shortcut. Returns `Some(true)` only for an explicit tap on Add.
pub fn install_dialog(
    ctx: &Context,
    t: &Tokens,
    title: &str,
    origin: &str,
    note: Option<&str>,
) -> (Option<bool>, Rect) {
    let mut lines = vec![origin];
    lines.extend(note);
    confirm_dialog(
        ctx,
        t,
        "krypa-install-dialog",
        title,
        &lines,
        strings::INSTALL_CONFIRM,
        strings::INSTALL_CANCEL,
    )
}

/// A full-width tappable list row with a title and a subtitle (recents). Both texts are
/// hostile and rendered as plain, truncated text.
pub fn list_row(ui: &mut Ui, t: &Tokens, title: &str, subtitle: &str) -> egui::Response {
    let title = display_text(title, MAX_DISPLAY_CHARS);
    let subtitle = display_text(subtitle, MAX_DISPLAY_CHARS);
    let (rect, resp) = ui.allocate_exact_size(vec2(ui.available_width(), 56.0), Sense::click());
    let fill = if resp.hovered() {
        t.surface_alt
    } else {
        t.surface
    };
    let p = ui.painter();
    p.rect_filled(rect, radius(), fill);
    p.rect_stroke(
        rect,
        radius(),
        Stroke::new(1.0, t.border),
        StrokeKind::Inside,
    );
    let w = rect.width() - 28.0;
    let g = one_line(ui, &title, FontId::proportional(16.0), t.text, w);
    let h = g.size().y;
    ui.painter().galley(rect.min + vec2(14.0, 8.0), g, t.text);
    let g = one_line(ui, &subtitle, FontId::proportional(13.0), t.text_muted, w);
    ui.painter()
        .galley(rect.min + vec2(14.0, 10.0 + h), g, t.text_muted);
    let label = format!("{title}, {subtitle}");
    resp.widget_info(|| WidgetInfo::labeled(Role::Button, true, &label));
    resp
}
