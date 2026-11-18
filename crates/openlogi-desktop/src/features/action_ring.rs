//! Eight-slot Actions Ring editor for the active device.

mod action_icons;
mod editor;

use crate::ui::theme::FocusRingStyle as _;
use gpui::{
    App, AppContext as _, Context, Entity, FocusHandle, Focusable, InteractiveElement, IntoElement,
    ParentElement, Render, ScrollHandle, SharedString, StatefulInteractiveElement as _, Styled,
    Subscription, Window, div, prelude::FluentBuilder as _, px, rgb, svg,
};
use gpui_base::Button as BaseButton;
use gpui_component::{
    Icon, IconName, Selectable as _,
    button::{Button, ButtonGroup},
    h_flex,
    input::{Input, InputEvent, InputState},
    tooltip::Tooltip,
    v_flex,
};
use openlogi_core::binding::{
    ActionRingConfig, ActionRingEntry, ActionRingIcon, ActionRingLayout, ActionRingSize,
    ActionRingSlot, HapticStrength,
};
use openlogi_ui::action_icons::RING_CANCEL_ICON;

use self::action_icons::action_icon_path;
use self::editor::{PowerUserInputs, action_library};
use crate::features::keyboard::editors::{PowerUserKind, text_editor_placeholder};
use crate::state::{AppState, StateEvent};
use crate::ui::action::localized_action_label;
use crate::ui::components::{Toggle, toggle_row};
use crate::ui::theme::{self, Palette, SelectableStyle as _, Typography as _};

/// Stateful Actions Ring editor. Ring configuration itself lives in
/// [`AppState`]; this entity owns selection and editor input state.
pub struct ActionRingPanel {
    focus_handle: FocusHandle,
    selected_slot: ActionRingSlot,
    application_input: Option<Entity<InputState>>,
    shortcut_input: Option<Entity<InputState>>,
    type_text_input: Option<Entity<InputState>>,
    applescript_input: Option<Entity<InputState>>,
    shell_command_input: Option<Entity<InputState>>,
    library_scroll: ScrollHandle,
    folder_name: Option<Entity<InputState>>,
    action_search: Option<Entity<InputState>>,
    #[expect(dead_code, reason = "held to keep the AppState subscription alive")]
    state_obs: Subscription,
}

impl ActionRingPanel {
    /// Create the editor and repaint it after any config/device change.
    pub fn new(cx: &mut Context<Self>) -> Self {
        let state_obs =
            AppState::repaint_on(cx, |event| matches!(event, StateEvent::BindingsChanged(_)));
        Self {
            focus_handle: cx.focus_handle(),
            selected_slot: ActionRingSlot::Top,
            application_input: None,
            shortcut_input: None,
            type_text_input: None,
            applescript_input: None,
            shell_command_input: None,
            library_scroll: ScrollHandle::new(),
            folder_name: None,
            action_search: None,
            state_obs,
        }
    }

    fn search_input(&mut self, window: &mut Window, cx: &mut Context<Self>) -> Entity<InputState> {
        let first_render = self.action_search.is_none();
        let input = editor_input(
            &mut self.action_search,
            tr!("actions.search_actions"),
            window,
            cx,
        );
        if first_render {
            cx.subscribe(&input, |_, _, event: &InputEvent, cx| {
                if matches!(event, InputEvent::Change) {
                    cx.notify();
                }
            })
            .detach();
        }
        input
    }

    /// Get-or-create the three Power User text inputs, split out of `render`
    /// to keep it under clippy's line budget.
    fn power_user_inputs(
        &mut self,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> PowerUserInputs {
        PowerUserInputs {
            type_text: editor_input(
                &mut self.type_text_input,
                text_editor_placeholder(PowerUserKind::TypeText),
                window,
                cx,
            ),
            applescript: editor_input(
                &mut self.applescript_input,
                text_editor_placeholder(PowerUserKind::RunAppleScript),
                window,
                cx,
            ),
            shell_command: editor_input(
                &mut self.shell_command_input,
                text_editor_placeholder(PowerUserKind::RunShellCommand),
                window,
                cx,
            ),
        }
    }
}

impl Focusable for ActionRingPanel {
    fn focus_handle(&self, _cx: &App) -> FocusHandle {
        self.focus_handle.clone()
    }
}

impl Render for ActionRingPanel {
    fn render(&mut self, window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let pal = theme::palette(cx);
        let (ring, layout) = action_ring_editor_state(cx);
        let haptics_supported = current_device_supports_haptics(cx);
        let application_input = editor_input(
            &mut self.application_input,
            tr!("action_ring.application_folder_path_or_url"),
            window,
            cx,
        );
        let shortcut_input = editor_input(
            &mut self.shortcut_input,
            tr!("action_ring.shortcut_e_g_cmd_plus_shift_plus_p"),
            window,
            cx,
        );
        let power_user = self.power_user_inputs(window, cx);
        let view = cx.entity();
        let in_folder =
            AppState::try_read(cx).is_some_and(|state| !state.action_ring_folder_path().is_empty());
        let selected = self.selected_slot;
        let entry = layout.slots.get(&selected);
        let folder_editor = entry.filter(|entry| entry.folder().is_some()).map(|entry| {
            let input = editor_input(&mut self.folder_name, tr!("action_ring.folder"), window, cx);
            folder_controls(selected, entry, input, &view, pal)
        });

        v_flex()
            .w_full()
            .gap_4()
            .tab_group()
            .track_focus(&self.focus_handle)
            .child(
                v_flex()
                    .gap_1()
                    .child(
                        div()
                            .text_subheading()
                            .child(tr!("action_ring.actions_ring")),
                    )
                    .child(
                        div()
                            .text_caption()
                            .text_color(pal.text_muted)
                            .child(tr!("action_ring.action_ring_description")),
                    ),
            )
            .when(in_folder, |panel| {
                let view = view.clone();
                panel.child(
                    Button::new("ring-parent-folder")
                        .label(tr!("action_ring.parent_ring"))
                        .on_click(move |_, _, cx| navigate_folder(&view, None, cx)),
                )
            })
            .when(entry.is_none() && !in_folder, |panel| {
                panel.child(empty_slot_folder_actions(selected))
            })
            .child(
                h_flex()
                    .w_full()
                    .items_start()
                    .justify_center()
                    .gap_4()
                    .child(ring_preview(&layout, self.selected_slot, &view, pal))
                    .when_some(folder_editor, ParentElement::child)
                    .when(entry.and_then(ActionRingEntry::folder).is_none(), |row| {
                        let search = self.search_input(window, cx);
                        let query = search.read(cx).value();
                        row.child(action_library(
                            (selected, entry),
                            Some((&search, query.as_ref())),
                            &application_input,
                            &shortcut_input,
                            &power_user,
                            &self.library_scroll,
                            pal,
                        ))
                    }),
            )
            .child(ring_settings(&ring, haptics_supported, pal, cx))
    }
}

fn empty_slot_folder_actions(slot: ActionRingSlot) -> impl IntoElement {
    h_flex()
        .gap_2()
        .child(
            Button::new("ring-create-folder")
                .label(tr!("action_ring.add_folder"))
                .on_click(move |_, _, cx| {
                    AppState::apply(cx, |state| state.create_action_ring_folder(slot));
                }),
        )
        .child(
            Button::new("ring-create-volume-flyout")
                .label(tr!("action_ring.add_volume_controls"))
                .on_click(move |_, _, cx| {
                    AppState::apply(cx, |state| {
                        state.create_ring_volume_flyout(
                            slot,
                            tr!("action_ring.volume_controls").to_string(),
                        )
                    });
                }),
        )
}

fn ring_settings(
    ring: &ActionRingConfig,
    haptics_supported: bool,
    pal: Palette,
    cx: &App,
) -> impl IntoElement + use<> {
    v_flex()
        .gap_4()
        .child(toggle_row(
            "ring-enabled-row",
            pal,
            tr!("action_ring.enable_ring"),
            tr!("action_ring.open_at_the_current_cursor_position"),
            Toggle::new("ring-enabled-toggle")
                .selected(ring.enabled)
                .on_change(|enabled, _, cx| {
                    AppState::apply(cx, |state| state.commit_action_ring_enabled(*enabled));
                }),
        ))
        .child(toggle_row(
            "ring-size-row",
            pal,
            tr!("action_ring.ring_size"),
            tr!("action_ring.ring_size_description"),
            size_segment(ring.size, cx),
        ))
        .when(haptics_supported, |panel| {
            panel
                .child(toggle_row(
                    "ring-haptics-row",
                    pal,
                    tr!("action_ring.haptic_feedback"),
                    tr!("action_ring.action_ring_haptic_description"),
                    Toggle::new("ring-haptics-toggle")
                        .selected(ring.haptics)
                        .on_change(|haptics, _, cx| {
                            AppState::apply(cx, |state| state.commit_action_ring_haptics(*haptics));
                        }),
                ))
                .child(toggle_row(
                    "ring-haptic-strength-row",
                    pal,
                    tr!("action_ring.haptic_strength"),
                    if ring.haptic_strength.is_none() {
                        tr!("action_ring.device_strength")
                    } else {
                        tr!("action_ring.haptic_strength_description")
                    },
                    strength_segment(ring.haptic_strength.map(HapticStrength::nearest_level), cx),
                ))
        })
}

fn strength_segment(current: Option<HapticStrength>, cx: &App) -> ButtonGroup {
    let labels = [
        tr!("action_ring.haptic_off"),
        tr!("action_ring.haptic_subtle"),
        tr!("action_ring.haptic_low"),
        tr!("action_ring.haptic_medium"),
        tr!("action_ring.haptic_high"),
    ];
    ButtonGroup::new("ring-strength").outline().children(
        HapticStrength::LEVELS
            .into_iter()
            .zip(labels)
            .enumerate()
            .map(|(index, (strength, label))| {
                let selected = current.map(u8::from) == Some(strength);
                Button::new(("ring-strength", index))
                    .label(label)
                    .selected(selected)
                    .selected_segment(selected, cx)
                    .on_click(move |_, _, cx| {
                        AppState::apply(cx, |state| state.commit_haptic_strength(strength));
                    })
            }),
    )
}

fn navigate_folder(view: &Entity<ActionRingPanel>, slot: Option<ActionRingSlot>, cx: &mut App) {
    AppState::apply(cx, |state| state.navigate_action_ring_folder(slot));
    view.update(cx, |panel, cx| {
        panel.selected_slot = ActionRingSlot::Top;
        panel.folder_name = None;
        cx.notify();
    });
}

fn folder_controls(
    slot: ActionRingSlot,
    entry: &ActionRingEntry,
    input: Entity<InputState>,
    view: &Entity<ActionRingPanel>,
    pal: Palette,
) -> impl IntoElement + use<> {
    let view = view.clone();
    let flyout = entry.shows_flyout();
    let fits_flyout = entry.folder().is_some_and(|layout| layout.slots.len() <= 4);
    v_flex()
        .flex_1()
        .min_w(px(280.0))
        .max_w(px(320.0))
        .gap_2()
        .p_3()
        .rounded(pal.card_radius)
        .border_1()
        .border_color(pal.border)
        .bg(pal.panel)
        .child(div().text_subheading().child(entry_caption(entry)))
        .child(
            Button::new("ring-open-folder")
                .label(tr!("action_ring.open_folder"))
                .on_click(move |_, _, cx| navigate_folder(&view, Some(slot), cx)),
        )
        .when(fits_flyout, |panel| {
            panel.child(
                Button::new("ring-folder-display")
                    .label(if flyout {
                        tr!("action_ring.use_full_ring")
                    } else {
                        tr!("action_ring.use_compact_flyout")
                    })
                    .on_click(move |_, _, cx| {
                        AppState::apply(cx, |state| state.set_ring_folder_flyout(slot, !flyout));
                    }),
            )
        })
        .child(
            h_flex().gap_2().child(Input::new(&input)).child(
                Button::new("ring-rename-folder")
                    .label(tr!("action_ring.rename_folder"))
                    .on_click(move |_, _, cx| {
                        let value = input.read(cx).value();
                        let name = value.trim();
                        AppState::apply(cx, |state| {
                            state.rename_action_ring_folder(
                                slot,
                                (!name.is_empty()).then(|| name.to_owned()),
                            )
                        });
                    }),
            ),
        )
        .child(
            Button::new("ring-clear-folder")
                .label(tr!("action_ring.clear_slot"))
                .on_click(move |_, _, cx| {
                    AppState::apply(cx, |state| state.commit_action_ring_slot(slot, None));
                }),
        )
}

#[derive(Clone)]
struct DraggedRingSlot {
    slot: ActionRingSlot,
    label: SharedString,
}

impl Render for DraggedRingSlot {
    fn render(&mut self, _: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let pal = theme::palette(cx);
        div()
            .px_2()
            .py_1()
            .rounded_md()
            .border_1()
            .border_color(pal.border)
            .bg(pal.panel)
            .text_color(pal.text_primary)
            .child(self.label.clone())
    }
}

fn action_ring_editor_state(cx: &Context<ActionRingPanel>) -> (ActionRingConfig, ActionRingLayout) {
    AppState::try_read(cx).map_or_else(
        || {
            let ring = ActionRingConfig::default();
            let layout = ring.default.clone();
            (ring, layout)
        },
        |state| {
            let ring = state.current_action_ring();
            let layout = state.current_action_ring_layout();
            (ring, layout)
        },
    )
}

fn editor_input(
    state: &mut Option<Entity<InputState>>,
    placeholder: impl Into<SharedString>,
    window: &mut Window,
    cx: &mut Context<ActionRingPanel>,
) -> Entity<InputState> {
    let placeholder = placeholder.into();
    let state = state
        .get_or_insert_with(|| {
            cx.new(|cx| InputState::new(window, cx).placeholder(placeholder.clone()))
        })
        .clone();
    // Callers pass a per-render `tr!` string, so a cached input follows a live
    // language switch instead of keeping the placeholder it was built with.
    crate::ui::components::localize_placeholder(&state, placeholder, window, cx);
    state
}

/// Small / Medium / Large. Each button commits its own size: the group's own
/// `on_click` hears only pointer clicks, so Enter and Space on a focused
/// segment would not reach it.
fn size_segment(current: ActionRingSize, cx: &App) -> ButtonGroup {
    ButtonGroup::new("ring-size")
        .outline()
        .children(ActionRingSize::ALL.map(|size| {
            let (id, label) = match size {
                ActionRingSize::Small => ("ring-size-small", tr!("action_ring.size_small")),
                ActionRingSize::Medium => ("ring-size-medium", tr!("action_ring.size_medium")),
                ActionRingSize::Large => ("ring-size-large", tr!("action_ring.size_large")),
            };
            Button::new(id)
                .label(label)
                .selected(current == size)
                .selected_segment(current == size, cx)
                .on_click(move |_, _, cx| {
                    AppState::apply(cx, |state| state.commit_action_ring_size(size));
                })
        }))
}

fn current_device_supports_haptics(cx: &Context<ActionRingPanel>) -> bool {
    AppState::try_read(cx).is_some_and(|state| {
        state
            .current_record()
            .is_some_and(|record| record.capabilities_or_presumed().haptic_feedback)
    })
}

const PREVIEW_SIZE: f32 = 320.0;
const PREVIEW_RADIUS: f32 = 106.0;
const PREVIEW_SLOT_SIZE: f32 = 50.0;

fn ring_preview(
    layout: &ActionRingLayout,
    selected_slot: ActionRingSlot,
    view: &Entity<ActionRingPanel>,
    pal: Palette,
) -> impl IntoElement {
    div()
        .relative()
        .flex_none()
        .size(px(PREVIEW_SIZE))
        .child(
            div()
                .absolute()
                .left(px(24.0))
                .top(px(24.0))
                .size(px(PREVIEW_SIZE - 48.0))
                .rounded_full()
                .border_1()
                .border_color(pal.border)
                .bg(pal.panel),
        )
        .child(
            div()
                .absolute()
                .left(px(PREVIEW_SIZE / 2.0 - 24.0))
                .top(px(PREVIEW_SIZE / 2.0 - 24.0))
                .size(px(48.0))
                .flex()
                .items_center()
                .justify_center()
                .rounded_full()
                .bg(pal.muted)
                // An `svg` paints only with its own text colour.
                .child(
                    svg()
                        .path(RING_CANCEL_ICON)
                        .size(px(20.0))
                        .flex_none()
                        .text_color(pal.text_muted),
                ),
        )
        .children(ActionRingSlot::ALL.into_iter().map(|slot| {
            slot_button(
                slot,
                layout.slots.get(&slot),
                selected_slot == slot,
                view,
                pal,
            )
        }))
}

fn slot_button(
    slot: ActionRingSlot,
    entry: Option<&ActionRingEntry>,
    selected: bool,
    view: &Entity<ActionRingPanel>,
    pal: Palette,
) -> impl IntoElement {
    let index = slot.index();
    let (left, top) = slot.placement(PREVIEW_SIZE, PREVIEW_RADIUS, PREVIEW_SLOT_SIZE);
    let label = entry.map_or_else(
        || tr!("action_ring.empty_slot").to_string(),
        |entry| entry_caption(entry).to_string(),
    );
    let icon_path = entry.map(|entry| {
        entry.custom_icon().map_or_else(
            || {
                entry
                    .action()
                    .map_or(ActionRingIcon::Folder.asset_path(), action_icon_path)
            },
            ActionRingIcon::asset_path,
        )
    });
    let accessible_label = label.clone();
    let selected_view = view.clone();
    let drop_view = view.clone();

    BaseButton::new(("action-ring-slot", index))
        .debug_selector(move || format!("action-ring-slot-{slot:?}"))
        .selected(selected)
        .absolute()
        .left(px(left))
        .top(px(top))
        .size(px(PREVIEW_SLOT_SIZE))
        .flex()
        .items_center()
        .justify_center()
        .rounded_full()
        .border_2()
        .border_color(if selected {
            rgb(theme::ACCENT_BLUE).into()
        } else {
            pal.border
        })
        .bg(if selected {
            theme::accent_tint()
        } else {
            pal.control
        })
        .text_color(if selected {
            pal.text_primary
        } else {
            pal.text_muted
        })
        .cursor_pointer()
        .accessibility_label(accessible_label)
        .when(entry.is_some(), |button| {
            button.on_drag(
                DraggedRingSlot {
                    slot,
                    label: label.clone().into(),
                },
                |dragged, _, _, cx| cx.new(|_| dragged.clone()),
            )
        })
        .drag_over::<DraggedRingSlot>(|style, _, _, _| {
            style.bg(theme::accent_tint()).border_color(theme::accent())
        })
        .on_drop(move |dragged: &DraggedRingSlot, _, cx| {
            AppState::apply(cx, |state| state.move_action_ring_slot(dragged.slot, slot));
            drop_view.update(cx, |panel, cx| {
                panel.selected_slot = slot;
                panel.folder_name = None;
                cx.notify();
            });
        })
        .tooltip(move |window, cx| Tooltip::new(label.clone()).build(window, cx))
        .when_some(icon_path, |button, path| {
            button.child(svg().path(path).size(px(20.0)).text_color(if selected {
                pal.text_primary
            } else {
                pal.text_muted
            }))
        })
        .when(icon_path.is_none(), |button| {
            button.child(Icon::new(IconName::Plus).size_4())
        })
        .hover(move |button| {
            button.bg(if selected {
                theme::accent_tint_hover()
            } else {
                pal.control_hover
            })
        })
        .focus_visible(move |style| style.focus_ring(pal))
        .on_click(move |_, _, cx| {
            selected_view.update(cx, |panel, cx| {
                panel.selected_slot = slot;
                panel.folder_name = None;
                cx.notify();
            });
        })
}

/// The name a populated slot shows in the preview: the label the user gave it,
/// verbatim, or its action's localized name. The agent makes the same choice
/// for the ring itself, so the preview and the ring name a slot alike.
fn entry_caption(entry: &ActionRingEntry) -> SharedString {
    entry.custom_label().map_or_else(
        || {
            entry
                .action()
                .map_or_else(|| tr!("action_ring.folder"), localized_action_label)
        },
        |label| SharedString::from(label.to_owned()),
    )
}

#[cfg(test)]
mod tests;
