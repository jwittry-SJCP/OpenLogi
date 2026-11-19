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

#[gpui::test]
fn compact_folder_edits_children_in_the_parent_preview(cx: &mut gpui::TestAppContext) {
    use gpui::{Modifiers, MouseButton};
    use openlogi_core::config::Config;

    use crate::services::assets::AssetResolver;
    use crate::services::i18n::LOCALE_LOCK;
    use crate::state::{Sources, tests::direct_inventory};
    use ActionRingSlot::{Top, TopLeft};

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
}
