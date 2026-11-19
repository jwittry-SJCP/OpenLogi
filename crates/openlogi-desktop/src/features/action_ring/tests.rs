use std::collections::BTreeMap;

use gpui::{TestAppContext, VisualTestContext, size};
use openlogi_core::binding::{Action, ActionRingIcon, KeyCombo, RingAction};
use openlogi_core::config::Config;

use super::*;
use crate::services::assets::AssetResolver;
use crate::state::Sources;

/// A ring whose top slot sends `Super+K`, optionally named by the user.
fn shortcut_layout(label: Option<&str>) -> ActionRingLayout {
    let combo: KeyCombo = "Super+K".parse().expect("a valid chord");
    let action = RingAction::new(Action::CustomShortcut(combo)).expect("a valid ring action");
    let mut layout = ActionRingLayout {
        slots: BTreeMap::new(),
    };
    layout.set_action(ActionRingSlot::Top, Some(action));
    layout.set_label(ActionRingSlot::Top, label.map(str::to_owned));
    layout
}

#[test]
fn preview_names_a_slot_by_its_custom_label() {
    let layout = shortcut_layout(Some("Kitty"));

    assert_eq!(
        entry_caption(&layout.slots[&ActionRingSlot::Top]).as_ref(),
        "Kitty"
    );
}

#[test]
fn preview_names_an_unlabelled_slot_by_its_action() {
    let layout = shortcut_layout(None);

    assert_eq!(
        entry_caption(&layout.slots[&ActionRingSlot::Top]).as_ref(),
        "Super+K"
    );
}

/// The panel in a column narrower than the ring and the slot editor side
/// by side, as the Actions Ring tab is at the main window's minimum width.
struct NarrowHost {
    panel: Entity<ActionRingPanel>,
}

impl Render for NarrowHost {
    fn render(&mut self, _window: &mut Window, _cx: &mut Context<Self>) -> impl IntoElement {
        div().w(px(NARROW_W)).child(self.panel.clone())
    }
}

const NARROW_W: f32 = 500.;

/// Draw the panel in the narrow column with an ephemeral config.
fn draw_narrow_panel(cx: &mut TestAppContext) -> &mut VisualTestContext {
    cx.update(gpui_kit::init);
    cx.update(|cx| {
        let (commands, _) = tokio::sync::mpsc::unbounded_channel();
        let resolver = AssetResolver::new();
        let state =
            cx.new(|_| AppState::new(Sources::in_memory(Config::ephemeral(), &resolver, commands)));
        AppState::set_global(state, cx);
    });
    let (_, cx) = cx.add_window_view(|_, cx| NarrowHost {
        panel: cx.new(ActionRingPanel::new),
    });
    cx.simulate_resize(size(px(800.), px(1200.)));
    cx.update(|window, cx| window.draw(cx).clear(cx));
    cx
}

#[gpui::test]
fn the_slot_editor_wraps_below_the_ring_in_a_narrow_column(cx: &mut TestAppContext) {
    let cx = draw_narrow_panel(cx);

    let editor = cx
        .debug_bounds("ring-slot-editor")
        .expect("the slot editor renders");
    assert!(
        editor.left() >= px(0.) && editor.right() <= px(NARROW_W),
        "the slot editor {editor:?} must stay inside the {NARROW_W}px column"
    );
    assert!(
        editor.top() >= px(PREVIEW_SIZE),
        "the slot editor {editor:?} wraps below the ring"
    );
}

/// The Small / Medium / Large segment sits under the Ring size caption, at
/// the row's leading edge, instead of beside the text where it would leave
/// the caption a few words per line.
#[gpui::test]
fn the_ring_size_segment_sits_under_its_caption(cx: &mut TestAppContext) {
    let cx = draw_narrow_panel(cx);

    let row = cx
        .debug_bounds("ring-size-row")
        .expect("the ring size row renders");
    let segment = cx
        .debug_bounds("ring-size-row-control")
        .expect("the ring size segment renders");
    assert_eq!(
        segment.left(),
        row.left(),
        "the segment {segment:?} starts at the row's leading edge {row:?}"
    );
    assert!(
        segment.top() > row.top() && segment.bottom() <= row.bottom(),
        "the segment {segment:?} is the row's last line {row:?}"
    );
    assert!(
        segment.right() <= row.right(),
        "the segment {segment:?} stays inside the row {row:?}"
    );
}

fn drag(visual: &mut gpui::VisualTestContext, source: &'static str, target: &'static str) {
    use gpui::{Modifiers, MouseButton, point};
    let from = visual.debug_bounds(source).unwrap().center();
    let to = visual.debug_bounds(target).unwrap().center();
    visual.simulate_mouse_down(from, MouseButton::Left, Modifiers::none());
    visual.simulate_mouse_move(
        from + point(px(12.), px(12.)),
        MouseButton::Left,
        Modifiers::none(),
    );
    visual.simulate_mouse_move(to, MouseButton::Left, Modifiers::none());
    visual.simulate_mouse_up(to, MouseButton::Left, Modifiers::none());
    visual.update(|window, cx| window.draw(cx).clear(cx));
}

#[gpui::test]
fn dragging_an_actions_ring_slot_moves_then_swaps(cx: &mut gpui::TestAppContext) {
    use openlogi_core::config::Config;

    use crate::services::assets::AssetResolver;
    use crate::services::i18n::LOCALE_LOCK;
    use crate::state::{Sources, tests::direct_inventory};

    let _locale = LOCALE_LOCK.lock().unwrap();
    cx.update(gpui_kit::init);
    cx.update(theme::register_builtin_themes);
    cx.update(|cx| {
        let resolver = AssetResolver::new();
        let (sender, _receiver) = tokio::sync::mpsc::unbounded_channel();
        let inventory = direct_inventory([0xa3, 0x93, 0xca, 0xe0]);
        let state = cx.new(|_| {
            AppState::new(Sources {
                inventories: &[inventory],
                ..Sources::in_memory(Config::ephemeral(), &resolver, sender)
            })
        });
        AppState::set_global(state, cx);
        AppState::apply(cx, |state| {
            state.commit_action_ring_slot(ActionRingSlot::Left, None)
        });
    });
    let (_, visual) = cx.add_window_view(|window, cx| {
        let panel = cx.new(ActionRingPanel::new);
        gpui_kit::component::Root::new(panel, window, cx)
    });
    visual.update(|window, cx| window.draw(cx).clear(cx));
    let empty = visual
        .debug_bounds("action-ring-slot-Left")
        .unwrap()
        .center();
    visual.simulate_mouse_down(empty, gpui::MouseButton::Left, gpui::Modifiers::none());
    visual.simulate_mouse_up(empty, gpui::MouseButton::Left, gpui::Modifiers::none());
    visual.update(|window, cx| window.draw(cx).clear(cx));
    let library = visual.debug_bounds("ring-slot-editor").unwrap();
    let add_folders = visual.debug_bounds("ring-folder-create-actions").unwrap();
    assert!(add_folders.bottom() <= library.bottom());

    drag(visual, "action-ring-slot-Top", "action-ring-slot-Left");
    let icons = visual.debug_bounds("ring-icon-picker").unwrap();
    let add_folders = visual.debug_bounds("ring-folder-create-actions").unwrap();
    assert!(add_folders.top() >= icons.bottom());
    let layout =
        visual.update(|_, cx| AppState::try_read(cx).unwrap().current_action_ring_layout());
    assert!(!layout.slots.contains_key(&ActionRingSlot::Top));
    assert_eq!(
        layout.slots[&ActionRingSlot::Left].action(),
        Some(&Action::Cut)
    );

    drag(visual, "action-ring-slot-Left", "action-ring-slot-Right");
    let layout =
        visual.update(|_, cx| AppState::try_read(cx).unwrap().current_action_ring_layout());
    assert_eq!(
        layout.slots[&ActionRingSlot::Left].action(),
        Some(&Action::Paste)
    );
    assert_eq!(
        layout.slots[&ActionRingSlot::Right].action(),
        Some(&Action::Cut)
    );

    let slot = ActionRingSlot::Right;
    visual.update(|_, cx| {
        AppState::apply(cx, |state| {
            state.commit_action_ring_icon(slot, Some(ActionRingIcon::Star))
        });
        let shortcut = AppState::try_read(cx)
            .unwrap()
            .current_action_ring_layout()
            .slots[&slot]
            .clone();
        AppState::apply(cx, |state| state.create_action_ring_folder(slot, true));
        let layout = AppState::try_read(cx).unwrap().current_action_ring_layout();
        let folder = &layout.slots[&slot];
        assert!(folder.shows_flyout());
        assert_eq!(
            folder.folder().unwrap().slots[&ActionRingSlot::Top],
            shortcut
        );
    });
}

#[gpui::test]
fn compact_folder_edits_and_drags_children_in_the_parent_preview(cx: &mut gpui::TestAppContext) {
    use gpui::{Modifiers, MouseButton};
    use openlogi_core::config::Config;

    use crate::services::assets::AssetResolver;
    use crate::services::i18n::LOCALE_LOCK;
    use crate::state::{Sources, tests::direct_inventory};
    use ActionRingSlot::{Top, TopLeft, TopRight};

    let _locale = LOCALE_LOCK.lock().unwrap();
    cx.update(gpui_kit::init);
    cx.update(theme::register_builtin_themes);
    cx.update(|cx| {
        let resolver = AssetResolver::new();
        let (sender, _receiver) = tokio::sync::mpsc::unbounded_channel();
        let inventory = direct_inventory([0xa3, 0x93, 0xca, 0xe0]);
        let state = cx.new(|_| {
            AppState::new(Sources {
                inventories: &[inventory],
                ..Sources::in_memory(Config::ephemeral(), &resolver, sender)
            })
        });
        AppState::set_global(state, cx);
        AppState::apply(cx, |state| state.commit_action_ring_slot(Top, None));
        AppState::apply(cx, |state| state.create_action_ring_folder(Top, true));
    });
    let (_, visual) = cx.add_window_view(|window, cx| {
        let panel = cx.new(ActionRingPanel::new);
        gpui_kit::component::Root::new(panel, window, cx)
    });
    visual.update(|window, cx| window.draw(cx).clear(cx));
    assert!(visual.debug_bounds("action-ring-add-after").is_some());
    assert!(visual.debug_bounds("action-ring-add-before").is_none());
    visual.update(|_, cx| {
        AppState::apply(cx, |state| {
            state.edit_compact_ring(Top, |layout| {
                layout.insert_compact_action(false, RingAction::new(Action::Copy).unwrap());
            })
        });
    });
    visual.update(|window, cx| window.draw(cx).clear(cx));
    let parent = visual
        .debug_bounds("action-ring-slot-Top")
        .unwrap()
        .center();
    let first = visual
        .debug_bounds("action-ring-child-Top")
        .unwrap()
        .center();
    assert!(first.y < parent.y);
    assert!((first.x - parent.x).abs() < px(1.0));
    assert!(visual.debug_bounds("action-ring-add-before").is_some());

    let empty = visual
        .debug_bounds("action-ring-add-after")
        .unwrap()
        .center();
    visual.simulate_mouse_down(empty, MouseButton::Left, Modifiers::none());
    visual.simulate_mouse_up(empty, MouseButton::Left, Modifiers::none());
    visual.update(|window, cx| window.draw(cx).clear(cx));
    assert!(visual.debug_bounds("ring-slot-editor").is_some());
    assert!(visual.debug_bounds("action-ring-slot-TopLeft").is_some());

    visual.update(|_, cx| {
        AppState::apply(cx, |state| {
            state.edit_compact_ring(Top, |layout| {
                layout.insert_compact_action(false, RingAction::new(Action::Cut).unwrap());
            })
        });
    });
    visual.update(|window, cx| window.draw(cx).clear(cx));
    drag(visual, "action-ring-child-Top", "action-ring-add-after");
    visual.update(|_, cx| {
        let state = AppState::try_read(cx).unwrap();
        assert!(state.action_ring_folder_path().is_empty());
        let ring = state.current_action_ring_layout();
        assert!(ring.slots.contains_key(&TopLeft));
        let children = ring.slots[&Top].folder().unwrap();
        assert_eq!(children.slots[&Top].action(), Some(&Action::Cut));
        assert_eq!(children.slots[&TopRight].action(), Some(&Action::Copy));
    });
}
