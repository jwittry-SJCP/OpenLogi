//! Agent-owned Actions Ring invocation and selection state.
//!
//! The overlay receives an opaque session and a read-only presentation
//! snapshot. Executable actions remain in the agent, and IPC commands can
//! select only a slot from that authoritative snapshot.

use std::collections::BTreeMap;
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::{Mutex, MutexGuard, PoisonError};
use std::time::Duration;

use openlogi_core::action_ring::{DISPLAY_LIFETIME, SliderKind};
use openlogi_core::binding::{Action, ActionRingLayout, ActionRingSize, ActionRingSlot};
use openlogi_core::scroll::ScrollDelta;
use openlogi_hid::DeviceRoute;
use openlogi_ipc::{
    ActionRingCommandError, ActionRingInvocation, ActionRingPresentation, ActionRingSlider,
    Generation, OBSERVE_HOLD, RingObservation,
};
use tokio::sync::watch;
use tokio::time::Instant;

/// Slack between the window closing and the session expiring.
///
/// The two clocks do not start together: a session is stamped in `begin`,
/// while the overlay's timer starts only once the long poll has delivered the
/// invocation and the window is up. Giving them equal durations expired the
/// session *before* the ring the user could still click disappeared, and a
/// click landing in that tail returned `SessionNotFound` — the window closed
/// and the action silently did not run. This covers that gap plus the click's
/// round-trip back.
const SESSION_GRACE: Duration = Duration::from_secs(3);

/// How long a session lives after [`ActionRingManager::begin`] opens it.
///
/// No session outlives this: hover, activation, and cancel all find an older
/// one expired. Work that must not outlast a session, but may never hear it
/// close (the overlay can exit without cancelling), ends this long after the
/// session opened.
pub const SESSION_LIFETIME: Duration = DISPLAY_LIFETIME.saturating_add(SESSION_GRACE);

/// How long the trigger must stay down after the ring opens for its release
/// to choose.
///
/// A shorter press is a click: the ring stays open for the pointer to pick a
/// slot, so a hand still moving the mouse as it clicks cannot run an action
/// it only passed over.
pub const HOLD_TO_CHOOSE: Duration = Duration::from_millis(250);

/// Immutable input used to open one ring session.
pub struct ActionRingSessionSpec {
    /// Config key of the device whose control opened the ring.
    pub device_key: String,
    /// HID++ route used for feedback when both config and capabilities allow it.
    pub haptic_route: Option<DeviceRoute>,
    /// Exact layout the agent will execute for this session.
    pub layout: ActionRingLayout,
    /// Configured UI locale, or `None` to follow the overlay host's system locale.
    pub language: Option<String>,
    /// How large the overlay draws the ring.
    pub size: ActionRingSize,
}

/// A validated slot activation returned to the action dispatcher.
pub struct ActionRingActivation {
    /// Config key of the device whose control opened the ring.
    pub device_key: String,
    /// Action snapshotted when the ring opened.
    pub action: Action,
    /// Route of the triggering device when activation feedback is available.
    pub haptic_route: Option<DeviceRoute>,
}

/// A validated hover transition that may play feedback.
#[derive(Debug, PartialEq, Eq)]
pub struct ActionRingHover {
    /// Route of the triggering device when hover feedback is available.
    pub haptic_route: Option<DeviceRoute>,
}

/// What releasing the trigger that opened the ring did to it.
pub enum ActionRingRelease {
    /// Nothing closed: the press was a click, which leaves the ring open for
    /// the pointer, or no ring was waiting on a held trigger.
    Kept,
    /// Held with no slot highlighted, so the ring closed without an action.
    Cancelled {
        /// The session that closed.
        session_id: u64,
    },
    /// Held over a slot, so the ring closed and that slot's action runs.
    Activated {
        /// The session that closed.
        session_id: u64,
        /// The action to run.
        activation: ActionRingActivation,
    },
}

struct Session {
    invocation: ActionRingInvocation,
    device_key: String,
    haptic_route: Option<DeviceRoute>,
    layout: ActionRingLayout,
    parent: Option<ActionRingLayout>,
    hovered: Option<ActionRingSlot>,
    /// The trigger that opened this ring has not been released yet.
    trigger_held: bool,
    opened_at: Instant,
}

impl Session {
    /// Navigate into a folder while preserving the session and ring position.
    fn enter_folder(&mut self, slot: ActionRingSlot) -> bool {
        let Some(entry) = self.layout.slots.get(&slot) else {
            return false;
        };
        let slider = entry
            .action()
            .and_then(SliderKind::for_action)
            .map(|kind| ActionRingSlider {
                slot,
                kind,
                scroll_origin: self.invocation.slider_scroll,
            });
        if self.invocation.flyout.is_some() && slider.is_some() {
            self.invocation.slider = slider.filter(|target| {
                !self
                    .invocation
                    .slider
                    .is_some_and(|current| current.slot == target.slot)
            });
            self.hovered = None;
            self.trigger_held = false;
            return true;
        }
        let mut layout = match (entry.folder(), slider) {
            (Some(layout), _) => layout.clone(),
            (None, Some(_)) => ActionRingLayout {
                slots: BTreeMap::new(),
            },
            (None, None) => return false,
        };
        let compact = entry.shows_flyout();
        if compact {
            layout.normalize_compact_slots();
            if self.invocation.flyout.is_some() {
                self.invocation.slots = presentation(&layout);
                self.invocation.slider = None;
                self.layout = layout;
                self.hovered = None;
                self.trigger_held = false;
                return true;
            }
        }
        self.invocation.flyout = compact.then_some(slot);
        self.invocation.slider = slider;
        self.invocation.parent_slots = if self.invocation.flyout.is_some() || slider.is_some() {
            std::mem::take(&mut self.invocation.slots)
        } else {
            BTreeMap::new()
        };
        self.invocation.slots = presentation(&layout);
        let previous = std::mem::replace(&mut self.layout, layout);
        self.parent = (self.invocation.flyout.is_some() || slider.is_some()).then_some(previous);
        self.hovered = None;
        self.trigger_held = false;
        true
    }

    fn activate(self, slot: ActionRingSlot) -> Option<ActionRingActivation> {
        Some(ActionRingActivation {
            action: self.layout.slots.get(&slot)?.action()?.clone(),
            device_key: self.device_key,
            haptic_route: self.haptic_route,
        })
    }
}

fn presentation(layout: &ActionRingLayout) -> BTreeMap<ActionRingSlot, ActionRingPresentation> {
    layout
        .slots
        .iter()
        .map(|(&slot, entry)| {
            let action = entry.action();
            let label = entry.custom_label();
            (
                slot,
                ActionRingPresentation {
                    label: label.map_or_else(
                        || action.map_or_else(|| "action_ring.folder".to_owned(), Action::label),
                        str::to_owned,
                    ),
                    literal: label.is_some(),
                    icon: entry.icon(),
                    opens_folder: entry.folder().is_some()
                        || action.and_then(SliderKind::for_action).is_some(),
                },
            )
        })
        .collect()
}

#[derive(Default)]
struct State {
    active: Option<Session>,
}

impl State {
    fn expire(&mut self) {
        if self
            .active
            .as_ref()
            .is_some_and(|session| session.opened_at.elapsed() > SESSION_LIFETIME)
        {
            self.active = None;
        }
    }

    fn showing(&self) -> bool {
        self.active.is_some()
    }

    fn active_session(&mut self, session_id: u64) -> Result<&mut Session, ActionRingCommandError> {
        match self.active.as_mut() {
            Some(session) if session.invocation.session_id == session_id => Ok(session),
            _ => Err(ActionRingCommandError::SessionNotFound),
        }
    }
}

/// Shared ring state used by input dispatch and IPC handlers.
pub struct ActionRingManager {
    next_session: AtomicU64,
    state: Mutex<State>,
    /// What the overlay observes. Derived from [`Self::state`] after every
    /// change, so "no ring" is simply the absence of one — there is no closed
    /// message to invent, and an overlay that restarts mid-ring reads the live
    /// one instead of having missed its invocation.
    published: watch::Sender<RingObservation>,
}

impl Default for ActionRingManager {
    fn default() -> Self {
        // Generation 1: 0 is the observer's "seen nothing" sentinel.
        let (published, _) = watch::channel(RingObservation {
            generation: 1,
            invocation: None,
        });
        Self {
            next_session: AtomicU64::new(1),
            state: Mutex::new(State::default()),
            published,
        }
    }
}

impl ActionRingManager {
    /// Open or replace the current session and wake the overlay long-poll.
    pub fn begin(&self, spec: ActionRingSessionSpec) -> ActionRingInvocation {
        let session_id = self.next_session.fetch_add(1, Ordering::Relaxed);
        let invocation = ActionRingInvocation {
            session_id,
            slots: presentation(&spec.layout),
            language: spec.language,
            size: spec.size,
            flyout: None,
            parent_slots: BTreeMap::new(),
            slider: None,
            slider_scroll: 0,
        };
        let mut state = self.state();
        state.active = Some(Session {
            invocation: invocation.clone(),
            device_key: spec.device_key,
            haptic_route: spec.haptic_route,
            layout: spec.layout,
            parent: None,
            hovered: None,
            trigger_held: true,
            opened_at: Instant::now(),
        });
        self.publish(&state);
        invocation
    }

    /// Dismiss the showing session, if any, and return its session id —
    /// which is what lets a second press of the ring trigger toggle the ring
    /// closed, and tells the caller which session that press ended.
    ///
    /// Dismissing simply removes the session. The old wire format could only
    /// carry invocations, so a dismissal had to be encoded as an *empty*
    /// invocation for the overlay to acknowledge; observing state needs no such
    /// placeholder, and a trigger press racing the close now finds no session
    /// and opens a fresh ring.
    pub fn dismiss_active(&self) -> Option<u64> {
        let mut state = self.state();
        self.expire_and_publish(&mut state);
        let dismissed = if state.showing() {
            state
                .active
                .take()
                .map(|session| session.invocation.session_id)
        } else {
            None
        };
        self.publish(&state);
        dismissed
    }

    /// Whether a ring is showing that a trigger press would dismiss — the
    /// same test [`Self::dismiss_active`] makes, without dismissing.
    pub fn is_showing(&self) -> bool {
        let mut state = self.state();
        self.expire_and_publish(&mut state);
        state.showing()
    }

    /// Subscribe to the latest ring state. Every close path republishes the state.
    /// Rapid updates may replace intermediate states before a subscriber reads them.
    pub fn subscribe(&self) -> watch::Receiver<RingObservation> {
        self.published.subscribe()
    }

    /// Serve one [`Agent::observe_action_ring`](openlogi_ipc::Agent::observe_action_ring).
    pub async fn observe(&self, since: Generation) -> RingObservation {
        let mut rx = self.published.subscribe();
        let changed = rx.wait_for(|observed| observed.generation != since);
        match tokio::time::timeout(OBSERVE_HOLD, changed).await {
            Ok(Ok(observed)) => observed.clone(),
            // Hold elapsed, or the manager is gone: answer with what we have.
            Ok(Err(_)) | Err(_) => self.published.borrow().clone(),
        }
    }

    /// Republish what the overlay should be showing. Called after every change
    /// to [`Self::state`], and a republish that says the same thing wakes
    /// nobody.
    fn publish(&self, state: &State) {
        let invocation = state
            .active
            .as_ref()
            .map(|session| session.invocation.clone());
        self.published.send_if_modified(|observed| {
            if observed.invocation == invocation {
                return false;
            }
            observed.invocation = invocation;
            observed.generation += 1;
            true
        });
    }

    /// Route native and diverted wheel movement to the open slider.
    /// The hook may call this on its input thread, so it never waits for the lock.
    pub fn try_scroll_slider(&self, delta: ScrollDelta) -> bool {
        let (x, y) = (delta.x(), delta.y());
        if !delta.is_finite() || (x == 0.0 && y == 0.0) {
            return false;
        }
        let dominant = if x.abs() > y.abs() { x } else { y };
        let scale = match delta {
            ScrollDelta::WheelTicks { .. } => 200.0,
            ScrollDelta::Pixels { .. } => 10.0,
        };
        #[expect(
            clippy::cast_possible_truncation,
            reason = "rounded and bounded to one full slider range"
        )]
        let change = (dominant * scale).round().clamp(-10_000.0, 10_000.0) as i32;
        let Ok(mut state) = self.state.try_lock() else {
            return false;
        };
        self.expire_and_publish(&mut state);
        let Some(session) = state
            .active
            .as_mut()
            .filter(|session| session.invocation.slider.is_some())
        else {
            return false;
        };
        session.invocation.slider_scroll = session.invocation.slider_scroll.saturating_add(change);
        self.publish(&state);
        true
    }

    /// Record a changed highlighted slot. Repeated hover reports are ignored so
    /// one stationary pointer cannot flood the HID++ haptic queue.
    pub fn hover(
        &self,
        session_id: u64,
        slot: ActionRingSlot,
    ) -> Result<Option<ActionRingHover>, ActionRingCommandError> {
        let mut state = self.state();
        self.expire_and_publish(&mut state);
        let session = state.active_session(session_id)?;
        if !session.layout.slots.contains_key(&slot) {
            return Err(ActionRingCommandError::SlotEmpty);
        }
        if session.hovered == Some(slot) {
            return Ok(None);
        }
        session.hovered = Some(slot);
        Ok(Some(ActionRingHover {
            haptic_route: session.haptic_route.clone(),
        }))
    }

    /// Record that the pointer left every slot, so releasing the held trigger
    /// now cancels rather than choosing the slot it last passed over.
    pub fn unhover(&self, session_id: u64) -> Result<(), ActionRingCommandError> {
        let mut state = self.state();
        self.expire_and_publish(&mut state);
        state.active_session(session_id)?.hovered = None;
        Ok(())
    }

    /// Navigate into the selected folder, or close and return its shortcut.
    pub fn activate(
        &self,
        session_id: u64,
        slot: ActionRingSlot,
    ) -> Result<Option<ActionRingActivation>, ActionRingCommandError> {
        let mut state = self.state();
        self.expire_and_publish(&mut state);
        state.active_session(session_id)?;
        self.activate_slot(&mut state, slot)
    }

    /// Select an entry in the visible parent. Clicking the current anchor
    /// returns to that parent without executing anything.
    pub fn activate_parent(
        &self,
        session_id: u64,
        slot: ActionRingSlot,
    ) -> Result<Option<ActionRingActivation>, ActionRingCommandError> {
        let mut state = self.state();
        self.expire_and_publish(&mut state);
        let session = state.active_session(session_id)?;
        if !session
            .parent
            .as_ref()
            .is_some_and(|parent| parent.slots.contains_key(&slot))
        {
            return Err(ActionRingCommandError::SlotEmpty);
        }
        if let Some(parent) = session.parent.take() {
            session.layout = parent;
        }
        let slider = session.invocation.slider.take().map(|slider| slider.slot);
        let anchor = session.invocation.flyout.take().or(slider);
        session.invocation.slots = std::mem::take(&mut session.invocation.parent_slots);
        session.hovered = None;
        if anchor == Some(slot) {
            self.publish(&state);
            Ok(None)
        } else {
            self.activate_slot(&mut state, slot)
        }
    }

    fn activate_slot(
        &self,
        state: &mut State,
        slot: ActionRingSlot,
    ) -> Result<Option<ActionRingActivation>, ActionRingCommandError> {
        let session = state
            .active
            .as_mut()
            .ok_or(ActionRingCommandError::SessionNotFound)?;
        if !session.layout.slots.contains_key(&slot) {
            return Err(ActionRingCommandError::SlotEmpty);
        }
        if session.enter_folder(slot) {
            self.publish(state);
            return Ok(None);
        }
        let activation = state
            .active
            .take()
            .and_then(|session| session.activate(slot));
        self.publish(state);
        Ok(activation)
    }

    /// The trigger that opened the showing ring was released.
    ///
    /// Released within [`HOLD_TO_CHOOSE`] it was a click, and the ring stays
    /// open. Held longer, the release chooses: the highlighted slot runs, and
    /// with none highlighted the ring closes. Only the first release after a
    /// ring opens counts.
    pub fn release(&self) -> ActionRingRelease {
        let mut state = self.state();
        self.expire_and_publish(&mut state);
        let Some(session) = state.active.as_mut().filter(|session| session.trigger_held) else {
            return ActionRingRelease::Kept;
        };
        session.trigger_held = false;
        if session.opened_at.elapsed() < HOLD_TO_CHOOSE {
            return ActionRingRelease::Kept;
        }
        if session
            .hovered
            .is_some_and(|slot| session.enter_folder(slot))
        {
            self.publish(&state);
            return ActionRingRelease::Kept;
        }
        let Some(session) = state.active.take() else {
            return ActionRingRelease::Kept;
        };
        self.publish(&state);
        let session_id = session.invocation.session_id;
        match session.hovered.and_then(|slot| session.activate(slot)) {
            Some(activation) => ActionRingRelease::Activated {
                session_id,
                activation,
            },
            None => ActionRingRelease::Cancelled { session_id },
        }
    }

    /// Cancel `session_id` if it is still active.
    pub fn cancel(&self, session_id: u64) {
        let mut state = self.state();
        if state
            .active
            .as_ref()
            .is_some_and(|session| session.invocation.session_id == session_id)
        {
            state.active = None;
        }
        self.publish(&state);
    }

    fn expire_and_publish(&self, state: &mut State) {
        state.expire();
        // Publish expiration before an interaction can return an error.
        self.publish(state);
    }

    fn state(&self) -> MutexGuard<'_, State> {
        self.state.lock().unwrap_or_else(PoisonError::into_inner)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use openlogi_core::binding::{ActionRingConfig, ActionRingIcon, RingAction};

    fn spec() -> ActionRingSessionSpec {
        ActionRingSessionSpec {
            device_key: "mouse-a".to_string(),
            haptic_route: None,
            layout: ActionRingConfig::default().default,
            language: None,
            size: ActionRingSize::default(),
        }
    }

    #[tokio::test]
    async fn an_overlay_that_has_seen_nothing_gets_the_showing_ring() {
        let manager = ActionRingManager::default();
        let expected = manager.begin(spec());
        // Generation 0 is "seen nothing", so this answers at once — which is
        // also how an overlay restarted mid-ring picks up the live one.
        let observed = manager.observe(0).await;
        assert_eq!(observed.invocation, Some(expected));
    }

    #[test]
    fn invocation_contains_presentation_but_not_execution_payloads() {
        let manager = ActionRingManager::default();
        let mut spec = spec();
        spec.layout
            .set_icon(ActionRingSlot::Top, Some(ActionRingIcon::Keyboard));
        spec.language = Some("fr".to_string());
        spec.size = ActionRingSize::Large;
        let invocation = manager.begin(spec);
        assert_eq!(
            invocation.slots[&ActionRingSlot::Top],
            ActionRingPresentation {
                label: "Cut".to_string(),
                literal: false,
                icon: ActionRingIcon::Keyboard,
                opens_folder: false,
            }
        );
        assert_eq!(invocation.language.as_deref(), Some("fr"));
        assert_eq!(invocation.size, ActionRingSize::Large);
    }

    #[tokio::test]
    async fn second_trigger_press_dismisses_and_third_reopens() {
        let manager = ActionRingManager::default();

        // Nothing showing yet: the first press must open, not dismiss.
        assert_eq!(manager.dismiss_active(), None);
        let opened = manager.begin(spec());
        let showing = manager.observe(0).await;
        assert_eq!(showing.invocation, Some(opened.clone()));

        // Second press: dismissed, and the overlay observes "no ring" rather
        // than a placeholder invocation it has to acknowledge.
        assert_eq!(manager.dismiss_active(), Some(opened.session_id));
        let dismissed = manager.observe(showing.generation).await;
        assert_eq!(dismissed.invocation, None);

        // Nothing is showing, so a third press opens again.
        assert_eq!(manager.dismiss_active(), None);
        let reopened = manager.begin(spec());
        assert!(!reopened.slots.is_empty());

        // A stale Cancel for the dismissed session must not kill the new one.
        manager.cancel(opened.session_id);
        assert_eq!(manager.dismiss_active(), Some(reopened.session_id));
    }

    #[test]
    fn custom_slot_labels_override_the_action_label() {
        let manager = ActionRingManager::default();
        let mut spec = spec();
        spec.layout
            .set_label(ActionRingSlot::Top, Some("Copy Invoice".to_string()));
        let invocation = manager.begin(spec);
        assert_eq!(invocation.slots[&ActionRingSlot::Top].label, "Copy Invoice");
        // Custom labels are literal so the overlay renders them verbatim even
        // when they collide with a localization key.
        assert!(invocation.slots[&ActionRingSlot::Top].literal);
    }

    #[test]
    fn activation_consumes_the_session() {
        let manager = ActionRingManager::default();
        let invocation = manager.begin(spec());
        let activation = manager
            .activate(invocation.session_id, ActionRingSlot::Top)
            .expect("a live session must activate its top slot")
            .expect("the top slot is a shortcut");
        assert_eq!(activation.device_key, "mouse-a");
        assert_eq!(activation.action, Action::Cut);
        assert!(matches!(
            manager.activate(invocation.session_id, ActionRingSlot::Top),
            Err(ActionRingCommandError::SessionNotFound)
        ));
    }

    #[tokio::test(start_paused = true)]
    async fn selecting_a_folder_keeps_the_ring_open_until_a_nested_action_is_chosen() {
        use ActionRingSlot::{Bottom, Top};
        for choose_by_release in [false, true] {
            let manager = ActionRingManager::default();
            let mut spec = spec();
            spec.layout.set_action(Top, None);
            spec.layout.add_folder(Top);
            spec.layout
                .folder_mut(Top)
                .unwrap()
                .set_action(Bottom, Some(RingAction::new(Action::Copy).unwrap()));
            let invocation = manager.begin(spec);
            assert!(invocation.slots[&Top].opens_folder);

            if choose_by_release {
                manager.hover(invocation.session_id, Top).unwrap();
                tokio::time::advance(HOLD_TO_CHOOSE).await;
                assert!(matches!(manager.release(), ActionRingRelease::Kept));
            } else {
                assert!(
                    manager
                        .activate(invocation.session_id, Top)
                        .unwrap()
                        .is_none()
                );
            }

            let folder = manager.observe(0).await.invocation.unwrap();
            assert_eq!(folder.session_id, invocation.session_id);
            assert_eq!(folder.slots.len(), 1);
            assert!(!folder.slots[&Bottom].opens_folder);
            let activation = manager
                .activate(folder.session_id, Bottom)
                .unwrap()
                .unwrap();
            assert_eq!(activation.action, Action::Copy);
            assert!(!manager.is_showing());
        }
    }

    #[tokio::test]
    async fn compact_folder_keeps_the_parent_and_runs_the_selected_child() {
        let manager = ActionRingManager::default();
        let mut spec = spec();
        let anchor = ActionRingSlot::Left;
        spec.layout.set_action(anchor, None);
        spec.layout.add_folder(anchor);
        spec.layout.folder_mut(anchor).unwrap().set_action(
            ActionRingSlot::Right,
            Some(RingAction::new(Action::MuteVolume).unwrap()),
        );
        spec.layout.set_folder_flyout(anchor, true);
        let root = manager.begin(spec);
        assert_eq!(root.slots[&anchor].icon, ActionRingIcon::CompactFlyout);
        assert!(manager.activate(root.session_id, anchor).unwrap().is_none());
        let page = manager.observe(0).await.invocation.unwrap();
        assert_eq!(page.flyout, Some(anchor));
        assert_eq!(page.parent_slots, root.slots);
        assert_eq!(page.slots.len(), 1);
        let action = manager
            .activate(page.session_id, ActionRingSlot::Top)
            .unwrap()
            .unwrap();
        assert_eq!(action.action, Action::MuteVolume);
    }

    #[cfg(target_os = "macos")]
    #[tokio::test]
    async fn volume_action_opens_a_slider_beside_the_parent() {
        let manager = ActionRingManager::default();
        let mut spec = spec();
        let slot = ActionRingSlot::Right;
        spec.layout
            .set_action(slot, Some(RingAction::new(Action::VolumeControls).unwrap()));
        let root = manager.begin(spec);
        assert!(root.slots[&slot].opens_folder);
        assert!(manager.activate(root.session_id, slot).unwrap().is_none());
        let opened = manager.observe(0).await.invocation.unwrap();
        assert_eq!(
            opened.slider.map(|slider| (slider.slot, slider.kind)),
            Some((slot, SliderKind::Volume))
        );
        assert_eq!(opened.parent_slots, root.slots);
        assert!(opened.slots.is_empty());
        assert!(
            manager
                .activate_parent(root.session_id, slot)
                .unwrap()
                .is_none()
        );
        let collapsed = manager.observe(0).await.invocation.unwrap();
        assert_eq!(collapsed, root);
        assert!(manager.activate(root.session_id, slot).unwrap().is_none());
        let sibling = manager
            .activate_parent(root.session_id, ActionRingSlot::Top)
            .unwrap()
            .unwrap();
        assert_eq!(sibling.action, Action::Cut);
    }

    #[cfg(target_os = "macos")]
    #[tokio::test]
    async fn volume_child_keeps_the_main_ring_and_all_flyout_actions_visible() {
        use ActionRingSlot::{Left, Top, TopRight};
        let manager = ActionRingManager::default();
        let mut spec = spec();
        spec.layout.set_action(Left, None);
        spec.layout.add_folder(Left);
        spec.layout.set_folder_flyout(Left, true);
        let folder = spec.layout.folder_mut(Left).unwrap();
        folder.insert_compact_action(false, RingAction::new(Action::VolumeControls).unwrap());
        folder.insert_compact_action(false, RingAction::new(Action::Copy).unwrap());
        let root = manager.begin(spec);
        manager.activate(root.session_id, Left).unwrap();
        let flyout = manager.observe(0).await.invocation.unwrap();

        manager.activate(root.session_id, Top).unwrap();
        let opened = manager.observe(0).await.invocation.unwrap();
        assert_eq!(
            opened.slider.map(|slider| (slider.slot, slider.kind)),
            Some((Top, SliderKind::Volume))
        );
        assert_eq!(opened.flyout, Some(Left));
        assert_eq!(opened.parent_slots, root.slots);
        assert_eq!(opened.slots, flyout.slots);

        manager.activate(root.session_id, Top).unwrap();
        assert_eq!(manager.observe(0).await.invocation.unwrap(), flyout);
        manager.activate(root.session_id, Top).unwrap();
        let action = manager
            .activate(root.session_id, TopRight)
            .unwrap()
            .unwrap();
        assert_eq!(action.action, Action::Copy);
    }

    #[cfg(target_os = "macos")]
    #[tokio::test]
    async fn both_wheels_adjust_volume_only_while_the_slider_is_open() {
        use ActionRingSlot::{Left, Top};
        let manager = ActionRingManager::default();
        let mut spec = spec();
        spec.layout.set_action(Left, None);
        spec.layout.add_folder(Left);
        spec.layout.set_folder_flyout(Left, true);
        spec.layout
            .folder_mut(Left)
            .unwrap()
            .insert_compact_action(false, RingAction::new(Action::VolumeControls).unwrap());
        let ring = manager.begin(spec);
        let vertical = ScrollDelta::wheel_ticks(0.0, 1.0);
        let horizontal = ScrollDelta::wheel_ticks(-1.5, 0.0);
        assert!(!manager.try_scroll_slider(vertical));
        manager.activate(ring.session_id, Left).unwrap();
        assert!(!manager.try_scroll_slider(horizontal));
        manager.activate(ring.session_id, Top).unwrap();

        assert!(manager.try_scroll_slider(vertical));
        assert!(manager.try_scroll_slider(horizontal));
        assert!(manager.try_scroll_slider(ScrollDelta::pixels(20.0, 0.0)));
        let updated = manager.observe(0).await.invocation.unwrap();
        assert_eq!(updated.slider_scroll, 100);
        assert_eq!(updated.flyout, Some(Left));
        assert_eq!(
            updated.slider.map(|slider| (slider.slot, slider.kind)),
            Some((Top, SliderKind::Volume))
        );

        manager.activate(ring.session_id, Top).unwrap();
        assert!(!manager.try_scroll_slider(vertical));
        manager.cancel(ring.session_id);
        assert!(!manager.try_scroll_slider(horizontal));
    }

    #[test]
    fn repeated_hover_is_deduplicated() {
        let manager = ActionRingManager::default();
        let invocation = manager.begin(spec());
        assert!(
            manager
                .hover(invocation.session_id, ActionRingSlot::Top)
                .is_ok_and(|hover| hover.is_some())
        );
        assert_eq!(
            manager.hover(invocation.session_id, ActionRingSlot::Top),
            Ok(None)
        );
    }

    /// A click on the trigger, even one that ends with the pointer over a
    /// slot, leaves the ring open; a later release does not reach it either.
    #[tokio::test(start_paused = true)]
    async fn a_click_on_the_trigger_leaves_the_ring_open() {
        let manager = ActionRingManager::default();
        let invocation = manager.begin(spec());
        manager
            .hover(invocation.session_id, ActionRingSlot::Right)
            .expect("the open session must take a hover");
        tokio::time::advance(HOLD_TO_CHOOSE.saturating_sub(Duration::from_millis(1))).await;

        assert!(matches!(manager.release(), ActionRingRelease::Kept));
        tokio::time::advance(HOLD_TO_CHOOSE).await;
        assert!(
            matches!(manager.release(), ActionRingRelease::Kept),
            "only the release of the press that opened the ring chooses"
        );
        assert!(manager.is_showing());
    }

    #[tokio::test(start_paused = true)]
    async fn releasing_a_held_trigger_runs_the_highlighted_slot() {
        let manager = ActionRingManager::default();
        let mut ring = manager.subscribe();
        let invocation = manager.begin(spec());
        ring.borrow_and_update();
        manager
            .hover(invocation.session_id, ActionRingSlot::Top)
            .expect("the open session must take a hover");
        manager
            .hover(invocation.session_id, ActionRingSlot::Right)
            .expect("the open session must take a hover");
        tokio::time::advance(HOLD_TO_CHOOSE).await;

        let ActionRingRelease::Activated {
            session_id,
            activation,
        } = manager.release()
        else {
            panic!("a held release over a slot must run it");
        };
        assert_eq!(session_id, invocation.session_id);
        assert_eq!(activation.action, Action::Paste, "the last slot hovered");
        assert_eq!(ring.borrow_and_update().invocation, None);
    }

    /// Back over the centre after passing a slot, a held release cancels.
    #[tokio::test(start_paused = true)]
    async fn releasing_a_held_trigger_over_no_slot_cancels() {
        let manager = ActionRingManager::default();
        let invocation = manager.begin(spec());
        manager
            .hover(invocation.session_id, ActionRingSlot::Top)
            .expect("the open session must take a hover");
        manager
            .unhover(invocation.session_id)
            .expect("the open session must take an unhover");
        tokio::time::advance(HOLD_TO_CHOOSE).await;

        assert!(matches!(
            manager.release(),
            ActionRingRelease::Cancelled { session_id } if session_id == invocation.session_id
        ));
        assert!(!manager.is_showing());
    }

    #[tokio::test]
    async fn cancellation_stops_the_ring_being_shown() {
        let manager = ActionRingManager::default();
        let invocation = manager.begin(spec());
        manager.cancel(invocation.session_id);
        assert_eq!(manager.observe(0).await.invocation, None);
    }

    #[test]
    fn is_showing_tracks_the_session_without_dismissing_it() {
        let manager = ActionRingManager::default();
        assert!(!manager.is_showing());
        let invocation = manager.begin(spec());
        assert!(manager.is_showing());
        assert!(manager.is_showing(), "asking must not dismiss the ring");
        manager.cancel(invocation.session_id);
        assert!(!manager.is_showing());
    }

    #[test]
    fn a_subscriber_hears_every_way_a_ring_closes() {
        let manager = ActionRingManager::default();
        let mut ring = manager.subscribe();
        let closes: [fn(&ActionRingManager, u64); 3] = [
            |manager, session| assert_eq!(manager.dismiss_active(), Some(session)),
            |manager, session| {
                manager
                    .activate(session, ActionRingSlot::Top)
                    .expect("the open session must be activatable")
                    .expect("the top slot is a shortcut");
            },
            |manager, session| manager.cancel(session),
        ];
        for close in closes {
            let invocation = manager.begin(spec());
            assert!(ring.has_changed().unwrap(), "opening must notify");
            ring.borrow_and_update();
            close(&manager, invocation.session_id);
            assert!(ring.has_changed().unwrap(), "closing must notify");
            assert_eq!(ring.borrow_and_update().invocation, None);
        }
    }

    #[test]
    fn replacement_invalidates_the_previous_session() {
        let manager = ActionRingManager::default();
        let first = manager.begin(spec());
        let second = manager.begin(spec());
        assert!(matches!(
            manager.activate(first.session_id, ActionRingSlot::Top),
            Err(ActionRingCommandError::SessionNotFound)
        ));
        manager
            .activate(second.session_id, ActionRingSlot::Top)
            .expect("the replacement session must still be activatable")
            .expect("the top slot is a shortcut");
    }

    /// The two clocks start at different moments — the session when `begin`
    /// stamps it, the window once the long poll has delivered and the overlay
    /// is up — so equal durations expire the session while the ring is still
    /// on screen, and the click that lands there is silently dropped.
    #[test]
    fn a_session_outlives_the_window_it_serves() {
        assert!(
            SESSION_LIFETIME > DISPLAY_LIFETIME,
            "a click on a still-visible ring must still find its session"
        );
    }

    #[tokio::test(start_paused = true)]
    async fn expired_hover_publishes_the_closed_session() {
        let manager = ActionRingManager::default();
        let mut ring = manager.subscribe();
        let invocation = manager.begin(spec());
        ring.borrow_and_update();
        tokio::time::advance(SESSION_LIFETIME + Duration::from_secs(1)).await;

        assert_eq!(
            manager.hover(invocation.session_id, ActionRingSlot::Top),
            Err(ActionRingCommandError::SessionNotFound)
        );
        assert!(ring.has_changed().unwrap(), "expiration must notify");
        assert_eq!(ring.borrow_and_update().invocation, None);
    }

    #[tokio::test(start_paused = true)]
    async fn expired_activation_publishes_the_closed_session() {
        let manager = ActionRingManager::default();
        let mut ring = manager.subscribe();
        let invocation = manager.begin(spec());
        ring.borrow_and_update();
        tokio::time::advance(SESSION_LIFETIME + Duration::from_secs(1)).await;

        assert!(matches!(
            manager.activate(invocation.session_id, ActionRingSlot::Top),
            Err(ActionRingCommandError::SessionNotFound)
        ));
        assert!(ring.has_changed().unwrap(), "expiration must notify");
        assert_eq!(ring.borrow_and_update().invocation, None);
    }
}
