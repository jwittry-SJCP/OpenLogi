//! The Actions Ring's shared display-duration constant.
//!
//! The ring's runtime session state (`ActionRingManager`: `Mutex`, `Notify`,
//! `Instant`) is agent-only and stays in `openlogi-agent-core` — it is not a
//! shared contract. This constant is, though: the agent derives the
//! session's expiry window from it and the GUI's overlay helper times its own
//! window by it, so the two clocks cannot drift out of step. The ring's
//! persisted layout/config schema ([`crate::binding::ActionRingLayout`] and
//! friends) is a separate concern, unrelated to this runtime timing value.

use std::time::Duration;

use crate::binding::ActionRingSlot;

/// How long the overlay keeps the ring on screen, counted from the moment its
/// window opens. The overlay owns the display; the constant lives here so the
/// session that has to outlive it is derived from it rather than kept in step
/// by hand.
pub const DISPLAY_LIFETIME: Duration = Duration::from_secs(15);

/// Available positions at 30-degree intervals around the outer circle.
pub const COMPACT_SLOTS: [ActionRingSlot; 12] = [
    ActionRingSlot::Top,
    ActionRingSlot::TopRight,
    ActionRingSlot::Right,
    ActionRingSlot::BottomRight,
    ActionRingSlot::Bottom,
    ActionRingSlot::BottomLeft,
    ActionRingSlot::Left,
    ActionRingSlot::TopLeft,
    ActionRingSlot::Flyout9,
    ActionRingSlot::Flyout10,
    ActionRingSlot::Flyout11,
    ActionRingSlot::Flyout12,
];

/// Outer-circle tile centers. The action group is centered on its anchor.
/// Negative and trailing indices let the editor show the two insertion controls.
#[must_use]
pub fn compact_slot_centre(
    anchor: ActionRingSlot,
    index: f32,
    count: f32,
    ring_radius: f32,
    diameter: f32,
) -> (f32, f32) {
    let step = std::f32::consts::TAU / 12.0;
    let offset = index - (count - 1.0) / 2.0;
    let (dx, dy) = anchor.unit_offset();
    let (sin, cos) = (offset * step).sin_cos();
    let radius = (ring_radius + diameter + 10.0).max((diameter + 7.0) / (2.0 * (step / 2.0).sin()));
    (
        (dx * cos - dy * sin) * radius,
        (dx * sin + dy * cos) * radius,
    )
}
