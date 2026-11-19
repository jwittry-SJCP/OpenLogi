//! Eight-slot Actions Ring editor for the active device.

mod action_icons;
mod editor;

use crate::ui::theme::FocusRingStyle as _;
use gpui::{
    App, AppContext as _, Context, Entity, FocusHandle, Focusable, InteractiveElement, IntoElement,
    ParentElement, Render, ScrollHandle, SharedString, StatefulInteractiveElement as _, Styled,
    Subscription, Window, div, prelude::FluentBuilder as _, px, rgb, svg,
};
use gpui_kit::base::Button as BaseButton;
use gpui_kit::component::{
    Icon, IconName, Selectable as _,
    button::{Button, ButtonGroup},
    h_flex,
    input::{Input, InputState},
    tooltip::Tooltip,
    v_flex,
};
use openlogi_core::action_ring::{COMPACT_SLOTS, compact_slot_centre};
use openlogi_core::binding::{
    ActionRingConfig, ActionRingEntry, ActionRingLayout, ActionRingSize, ActionRingSlot,
};
use openlogi_ui::action_icons::RING_CANCEL_ICON;

use self::editor::{PowerUserInputs, action_library, icon_editor};
use crate::features::keyboard::editors::{PowerUserKind, text_editor_placeholder};
use crate::state::{AppState, StateEvent};
use crate::ui::action::localized_action_label;
use crate::ui::components::{Toggle, stacked_row, toggle_row};
use crate::ui::theme::{self, Palette, PressableStyle as _, SelectableStyle as _, Typography as _};

/// Stateful Actions Ring editor. Ring configuration itself lives in
/// [`AppState`]; this entity owns selection and editor input state.
pub struct ActionRingPanel {
    focus_handle: FocusHandle,
    selected_slot: ActionRingSlot,
    selected_child: Option<SlotTarget>,
    application_input: Option<Entity<InputState>>,
    shortcut_input: Option<Entity<InputState>>,
    type_text_input: Option<Entity<InputState>>,
    applescript_input: Option<Entity<InputState>>,
    shell_command_input: Option<Entity<InputState>>,
    library_scroll: ScrollHandle,
    folder_name: Option<Entity<InputState>>,
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
            selected_child: None,
            application_input: None,
            shortcut_input: None,
            type_text_input: None,
            applescript_input: None,
            shell_command_input: None,
            library_scroll: ScrollHandle::new(),
            folder_name: None,
            state_obs,
        }
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
        let compact = entry
            .filter(|entry| entry.shows_flyout())
            .and_then(ActionRingEntry::folder)
            .map(|layout| {
                let mut children = layout.clone();
                children.normalize_compact_slots();
                children
            });
        let selected_child = compact.as_ref().and_then(|layout| {
            let target = self.selected_child?;
            let count = layout.slots.len();
            match target.insertion {
                Some((_, old_count)) if count == old_count => Some(target),
                _ if layout.slots.contains_key(&target.slot) => {
                    Some(SlotTarget::child(selected, target.slot))
                }
                _ => None,
            }
        });
        let folder_editor = entry
            .filter(|entry| entry.folder().is_some() && selected_child.is_none())
            .map(|entry| {
                let input =
                    editor_input(&mut self.folder_name, tr!("action_ring.folder"), window, cx);
                folder_controls(selected, entry, input, &view, pal)
            });
        let library_target = selected_child.map_or_else(
            || {
                entry
                    .and_then(ActionRingEntry::folder)
                    .is_none()
                    .then_some((SlotTarget::parent(selected), entry))
            },
            |child| {
                Some((
                    child,
                    compact.as_ref().and_then(|layout| {
                        child
                            .insertion
                            .is_none()
                            .then(|| layout.slots.get(&child.slot))
                            .flatten()
                    }),
                ))
            },
        );

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
            .child(
                // The editor wraps below the ring when the window is too narrow
                // for both, rather than clipping them. The ring's own switches
                // sit under it, beside the taller editor, so the page fits one
                // screen at the larger interface scales.
                h_flex()
                    .w_full()
                    .flex_wrap()
                    .items_start()
                    .justify_center()
                    .gap_4()
                    .child(
                        v_flex()
                            .flex_none()
                            .w(px(PREVIEW_SIZE))
                            .gap_4()
                            .child(ring_preview(
                                &layout,
                                selected,
                                compact.as_ref(),
                                selected_child,
                                &view,
                                pal,
                            ))
                            .child(ring_settings(&ring, haptics_supported, pal, cx)),
                    )
                    .when_some(folder_editor, ParentElement::child)
                    .when_some(library_target, |row, target| {
                        row.child(action_library(
                            target,
                            !in_folder,
                            &application_input,
                            &shortcut_input,
                            &power_user,
                            &self.library_scroll,
                            pal,
                        ))
                    }),
            )
    }
}

/// The settings that apply to the whole ring.
fn ring_settings(
    ring: &ActionRingConfig,
    haptics_supported: bool,
    pal: Palette,
    cx: &App,
) -> impl IntoElement + use<> {
    v_flex()
        .gap_3()
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
        .child(stacked_row(
            "ring-size-row",
            pal,
            tr!("action_ring.ring_size"),
            tr!("action_ring.ring_size_description"),
            size_segment(ring.size, cx),
        ))
        .when(haptics_supported, |panel| {
            panel.child(toggle_row(
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
        })
}

fn navigate_folder(view: &Entity<ActionRingPanel>, slot: Option<ActionRingSlot>, cx: &mut App) {
    AppState::apply(cx, |state| state.navigate_action_ring_folder(slot));
    view.update(cx, |panel, cx| {
        panel.selected_slot = ActionRingSlot::Top;
        panel.selected_child = None;
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
    let can_switch_layout = entry
        .folder()
        .is_some_and(|layout| !flyout || layout.slots.len() <= ActionRingSlot::ALL.len());
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
        .when(!flyout, |panel| {
            panel.child(
                Button::new("ring-open-folder")
                    .label(tr!("action_ring.open_folder"))
                    .on_click(move |_, _, cx| navigate_folder(&view, Some(slot), cx)),
            )
        })
        .when(can_switch_layout, |panel| {
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
        .child(icon_editor(
            SlotTarget::parent(slot),
            (
                entry.default_icon().asset_path(),
                if flyout {
                    tr!("action_ring.use_flyout_icon")
                } else {
                    tr!("action_ring.use_folder_icon")
                },
            ),
            entry.custom_icon(),
            pal,
        ))
        .child(
            Button::new("ring-clear-folder")
                .label(tr!("action_ring.clear_slot"))
                .on_click(move |_, _, cx| {
                    AppState::apply(cx, |state| state.commit_action_ring_slot(slot, None));
                }),
        )
}

#[derive(Clone, Copy, PartialEq, Eq)]
struct SlotTarget {
    slot: ActionRingSlot,
    folder: Option<ActionRingSlot>,
    /// Which end was clicked and how many entries were present on that click.
    insertion: Option<(bool, usize)>,
}

impl SlotTarget {
    const fn parent(slot: ActionRingSlot) -> Self {
        Self {
            slot,
            folder: None,
            insertion: None,
        }
    }

    const fn child(folder: ActionRingSlot, slot: ActionRingSlot) -> Self {
        Self {
            slot,
            folder: Some(folder),
            insertion: None,
        }
    }

    fn add(folder: ActionRingSlot, before: bool, count: usize) -> Option<Self> {
        let slot = *COMPACT_SLOTS.get(if before { 0 } else { count })?;
        Some(Self {
            slot,
            folder: Some(folder),
            insertion: Some((before, count)),
        })
    }

    fn select(self, panel: &mut ActionRingPanel) {
        panel.selected_slot = self.folder.unwrap_or(self.slot);
        panel.selected_child = self.folder.map(|_| self);
        panel.folder_name = None;
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

const PREVIEW_SIZE: f32 = 352.0;
const PREVIEW_RADIUS: f32 = 82.0;
const PREVIEW_SLOT_SIZE: f32 = 42.0;

fn ring_preview(
    layout: &ActionRingLayout,
    selected_slot: ActionRingSlot,
    compact: Option<&ActionRingLayout>,
    selected_child: Option<SlotTarget>,
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
                .left(px(PREVIEW_SIZE / 2.0
                    - PREVIEW_RADIUS
                    - PREVIEW_SLOT_SIZE / 2.0
                    - 8.0))
                .top(px(PREVIEW_SIZE / 2.0
                    - PREVIEW_RADIUS
                    - PREVIEW_SLOT_SIZE / 2.0
                    - 8.0))
                .size(px(2.0 * PREVIEW_RADIUS + PREVIEW_SLOT_SIZE + 16.0))
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
                SlotTarget::parent(slot),
                slot.placement(PREVIEW_SIZE, PREVIEW_RADIUS, PREVIEW_SLOT_SIZE),
                layout.slots.get(&slot),
                selected_slot == slot,
                view,
                pal,
            )
        }))
        .when_some(compact, |preview, layout| {
            let count = layout.slots.len();
            #[expect(
                clippy::cast_precision_loss,
                reason = "a flyout has at most twelve actions"
            )]
            let count_f32 = count as f32;
            let position = |index| {
                let (x, y) = compact_slot_centre(
                    selected_slot,
                    index,
                    count_f32.max(1.0),
                    PREVIEW_RADIUS,
                    PREVIEW_SLOT_SIZE,
                );
                let origin = (PREVIEW_SIZE - PREVIEW_SLOT_SIZE) / 2.0;
                (origin + x, origin + y)
            };
            let actions = layout
                .slots
                .iter()
                .enumerate()
                .map(|(index, (&slot, entry))| {
                    #[expect(
                        clippy::cast_precision_loss,
                        reason = "a flyout has at most twelve actions"
                    )]
                    let index = index as f32;
                    let target = SlotTarget::child(selected_slot, slot);
                    slot_button(
                        target,
                        position(index),
                        Some(entry),
                        selected_child == Some(target),
                        view,
                        pal,
                    )
                });
            let add = [true, false].into_iter().filter_map(|before| {
                if count == COMPACT_SLOTS.len()
                    || (before && (count == 0 || count + 1 == COMPACT_SLOTS.len()))
                {
                    return None;
                }
                let target = SlotTarget::add(selected_slot, before, count)?;
                let offset = if before { -1.0 } else { count_f32 };
                let placement = position(offset);
                Some(slot_button(
                    target,
                    placement,
                    None,
                    selected_child == Some(target),
                    view,
                    pal,
                ))
            });
            preview.children(actions.chain(add))
        })
}

fn slot_button(
    target: SlotTarget,
    (left, top): (f32, f32),
    entry: Option<&ActionRingEntry>,
    selected: bool,
    view: &Entity<ActionRingPanel>,
    pal: Palette,
) -> impl IntoElement {
    let slot = target.slot;
    let index = slot.index();
    let id = match (target.folder, target.insertion) {
        (_, Some((true, _))) => "action-ring-add-before",
        (_, Some((false, _))) => "action-ring-add-after",
        (Some(_), None) => "action-ring-child",
        (None, None) => "action-ring-slot",
    };
    let label = entry.map_or_else(
        || tr!("action_ring.empty_slot").to_string(),
        |entry| entry_caption(entry).to_string(),
    );
    let icon_path = entry.map(|entry| entry.icon().asset_path());
    let accessible_label = label.clone();
    let selected_view = view.clone();

    BaseButton::new((id, index))
        .debug_selector(move || {
            if target.insertion.is_some() {
                id.to_owned()
            } else {
                format!("{id}-{slot:?}")
            }
        })
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
        .pressable_fill(selected, pal)
        .focus_visible(move |style| style.focus_ring(pal))
        .on_click(move |_, _, cx| {
            selected_view.update(cx, |panel, cx| {
                target.select(panel);
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
