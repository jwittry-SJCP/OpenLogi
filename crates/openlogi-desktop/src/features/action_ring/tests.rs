use std::collections::BTreeMap;

use openlogi_core::binding::{Action, KeyCombo, RingAction};

use super::*;

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

#[gpui::test]
fn dragging_an_actions_ring_slot_moves_then_swaps(cx: &mut gpui::TestAppContext) {
    use gpui::{Modifiers, MouseButton, VisualTestContext, point};
    use openlogi_core::config::Config;

    use crate::services::assets::AssetResolver;
    use crate::services::i18n::LOCALE_LOCK;
    use crate::state::{Sources, tests::direct_inventory};

    fn drag(visual: &mut VisualTestContext, source: &'static str, target: &'static str) {
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

    let _locale = LOCALE_LOCK.lock().unwrap();
    cx.update(gpui_component::init);
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
        gpui_component::Root::new(panel, window, cx)
    });
    visual.update(|window, cx| window.draw(cx).clear(cx));
    drag(visual, "action-ring-slot-Top", "action-ring-slot-Left");
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
}
