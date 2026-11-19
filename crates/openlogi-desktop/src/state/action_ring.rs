//! Actions Ring settings and editor scope for the selected device.

use openlogi_core::binding::{
    ActionRingConfig, ActionRingEntry, ActionRingIcon, ActionRingLayout, ActionRingSize,
    ActionRingSlot, RingAction,
};

use super::events::StateEvents;
use super::{AppState, DeviceKey, DeviceRecord, StateEvent};

#[derive(Clone, Default)]
pub(super) struct FolderLocation {
    device: String,
    app: Option<String>,
    path: Vec<ActionRingSlot>,
}

impl AppState {
    /// Actions Ring settings for the active device, including its implicit
    /// default layout when nothing has been persisted yet.
    #[must_use]
    pub fn current_action_ring(&self) -> ActionRingConfig {
        self.current_record()
            .and_then(DeviceRecord::persistent_config_key)
            .map(|key| self.config.action_ring(key))
            .unwrap_or_default()
    }

    /// Application profile open in the Actions Ring editor, or `None` for the
    /// default layout.
    #[must_use]
    pub fn editing_action_ring_app(&self) -> Option<&str> {
        let key = self
            .current_record()
            .and_then(DeviceRecord::persistent_config_key)?;
        self.action_ring_editing_apps.get(key).map(String::as_str)
    }

    /// Open an application-specific Actions Ring layout, or the default
    /// layout with `None`. This window-local choice is not persisted.
    pub fn set_editing_action_ring_app(&mut self, app: Option<String>) -> StateEvents {
        let events = self.for_current_device(StateEvent::BindingsChanged);
        let Some(key) = self
            .current_record()
            .and_then(DeviceRecord::persistent_config_key)
            .map(str::to_string)
        else {
            return events;
        };
        self.action_ring_folder = None;
        if let Some(app) = app {
            self.action_ring_editing_apps.insert(key, app);
        } else {
            self.action_ring_editing_apps.remove(&key);
        }
        events
    }

    /// Complete layout shown by the Actions Ring editor: the layout saved under
    /// the profile key it edits. A key with no saved layout shows the default,
    /// which its first edit copies — never another key's layout that an alias
    /// or `exe:` key would apply to the same application.
    #[must_use]
    pub fn current_action_ring_layout(&self) -> ActionRingLayout {
        let root = self
            .current_action_ring()
            .profile_layout(self.editing_action_ring_app());
        self.action_ring_folder_path()
            .iter()
            .try_fold(&root, |layout, slot| {
                layout.slots.get(slot).and_then(ActionRingEntry::folder)
            })
            .unwrap_or(&root)
            .clone()
    }

    pub(crate) fn action_ring_folder_path(&self) -> &[ActionRingSlot] {
        self.action_ring_folder
            .as_ref()
            .filter(|folder| {
                self.current_record()
                    .and_then(DeviceRecord::persistent_config_key)
                    == Some(folder.device.as_str())
                    && self.editing_action_ring_app() == folder.app.as_deref()
            })
            .map_or(&[], |folder| folder.path.as_slice())
    }

    pub(crate) fn navigate_action_ring_folder(
        &mut self,
        destination: Option<ActionRingSlot>,
    ) -> StateEvents {
        let events = self.for_current_device(StateEvent::BindingsChanged);
        let Some(device) = self
            .current_record()
            .and_then(DeviceRecord::persistent_config_key)
        else {
            return events;
        };
        let mut path = self.action_ring_folder_path().to_vec();
        if let Some(slot) = destination {
            if self
                .current_action_ring_layout()
                .slots
                .get(&slot)
                .and_then(ActionRingEntry::folder)
                .is_none()
            {
                return events;
            }
            path.push(slot);
        } else {
            path.pop();
        }
        self.action_ring_folder = Some(FolderLocation {
            device: device.to_owned(),
            app: self.editing_action_ring_app().map(str::to_owned),
            path,
        });
        events
    }

    pub(crate) fn create_action_ring_folder(
        &mut self,
        slot: ActionRingSlot,
        compact: bool,
    ) -> StateEvents {
        self.edit_action_ring_layout("Actions Ring folder", |layout| {
            if layout
                .slots
                .get(&slot)
                .is_some_and(|entry| entry.folder().is_some())
            {
                return;
            }
            let shortcut = layout.slots.remove(&slot);
            layout.add_folder(slot);
            if let (Some(shortcut), Some(folder)) = (shortcut, layout.folder_mut(slot)) {
                folder.slots.insert(ActionRingSlot::Top, shortcut);
            }
            layout.set_folder_flyout(slot, compact);
        })
    }

    pub(crate) fn set_ring_folder_flyout(
        &mut self,
        slot: ActionRingSlot,
        flyout: bool,
    ) -> StateEvents {
        self.edit_action_ring_layout("Actions Ring folder layout", |layout| {
            layout.set_folder_flyout(slot, flyout);
        })
    }

    pub(crate) fn edit_compact_ring(
        &mut self,
        anchor: ActionRingSlot,
        edit: impl FnOnce(&mut ActionRingLayout),
    ) -> StateEvents {
        self.edit_action_ring_layout("Actions Ring flyout", |layout| {
            if layout
                .slots
                .get(&anchor)
                .is_some_and(ActionRingEntry::shows_flyout)
                && let Some(children) = layout.folder_mut(anchor)
            {
                children.normalize_compact_slots();
                edit(children);
            }
        })
    }

    pub(crate) fn rename_action_ring_folder(
        &mut self,
        slot: ActionRingSlot,
        name: Option<String>,
    ) -> StateEvents {
        self.edit_action_ring_layout("Actions Ring folder name", |layout| {
            layout.set_label(slot, name);
        })
    }

    /// Replace or clear one slot in the open Actions Ring layout.
    pub fn commit_action_ring_slot(
        &mut self,
        slot: ActionRingSlot,
        action: Option<RingAction>,
    ) -> StateEvents {
        self.edit_action_ring_layout("Actions Ring slot", |layout| {
            layout.set_action(slot, action);
        })
    }

    /// Set or restore the action-derived icon for one slot in the open layout.
    pub fn commit_action_ring_icon(
        &mut self,
        slot: ActionRingSlot,
        icon: Option<ActionRingIcon>,
    ) -> StateEvents {
        self.edit_action_ring_layout("Actions Ring icon", |layout| {
            layout.set_icon(slot, icon);
        })
    }

    fn edit_action_ring_layout(
        &mut self,
        what: &str,
        edit: impl FnOnce(&mut ActionRingLayout),
    ) -> StateEvents {
        let events = self.for_current_device(StateEvent::BindingsChanged);
        let Some(key) = self
            .current_record()
            .and_then(DeviceRecord::persistent_config_key)
            .map(str::to_string)
        else {
            return events;
        };
        let app = self.editing_action_ring_app().map(str::to_string);
        let path = self.action_ring_folder_path().to_vec();
        self.config.edit(|config| {
            let ring = &mut config.devices.entry(key).or_default().action_ring;
            let layout = match app {
                Some(app) => {
                    let inherited = ring.default.clone();
                    ring.per_app.entry(app).or_insert(inherited)
                }
                None => &mut ring.default,
            };
            if let Some(layout) = path
                .into_iter()
                .try_fold(layout, ActionRingLayout::folder_mut)
            {
                edit(layout);
            }
        });
        self.persist_and_reload(what);
        events
    }

    /// Restore live default inheritance while keeping the application's editor open.
    pub fn reset_action_ring_profile(&mut self, key: &DeviceKey, app: &str) -> StateEvents {
        self.clear_action_ring_profile(key.as_str(), app);
        self.action_ring_folder = None;
        StateEvent::BindingsChanged(key.clone()).into()
    }

    /// Remove the named ring profile without touching button bindings or a
    /// different editor selection. Failed saves preserve the open editor.
    pub fn remove_action_ring_profile(&mut self, key: &DeviceKey, app: &str) -> StateEvents {
        if self.clear_action_ring_profile(key.as_str(), app)
            && self
                .action_ring_editing_apps
                .get(key.as_str())
                .is_some_and(|selected| selected == app)
        {
            self.action_ring_editing_apps.remove(key.as_str());
        }
        StateEvent::BindingsChanged(key.clone()).into()
    }

    fn clear_action_ring_profile(&mut self, key: &str, app: &str) -> bool {
        if !self
            .config
            .devices
            .get(key)
            .is_some_and(|device| device.action_ring.per_app.contains_key(app))
        {
            return true;
        }
        self.config.edit(|config| {
            if let Some(device) = config.devices.get_mut(key) {
                device.action_ring.per_app.remove(app);
            }
        });
        self.persist_and_reload("Actions Ring application profile")
    }

    /// Enable or disable the active device's Actions Ring.
    pub fn commit_action_ring_enabled(&mut self, enabled: bool) -> StateEvents {
        let events = self.for_current_device(StateEvent::BindingsChanged);
        let Some(key) = self
            .current_record()
            .and_then(DeviceRecord::persistent_config_key)
            .map(str::to_string)
        else {
            return events;
        };
        self.config
            .edit(|config| config.set_action_ring_enabled(&key, enabled));
        self.persist_and_reload("Actions Ring enabled state");
        events
    }

    /// Enable or disable hover and activation haptics for the active ring.
    pub fn commit_action_ring_haptics(&mut self, enabled: bool) -> StateEvents {
        let events = self.for_current_device(StateEvent::BindingsChanged);
        let Some(key) = self
            .current_record()
            .and_then(DeviceRecord::persistent_config_key)
            .map(str::to_string)
        else {
            return events;
        };
        self.config
            .edit(|config| config.set_action_ring_haptics(&key, enabled));
        self.persist_and_reload("Actions Ring haptics");
        events
    }

    /// Set how large the active device's ring is drawn.
    pub fn commit_action_ring_size(&mut self, size: ActionRingSize) -> StateEvents {
        let events = self.for_current_device(StateEvent::BindingsChanged);
        let Some(key) = self
            .current_record()
            .and_then(DeviceRecord::persistent_config_key)
            .map(str::to_string)
        else {
            return events;
        };
        self.config
            .edit(|config| config.set_action_ring_size(&key, size));
        self.persist_and_reload("Actions Ring size");
        events
    }
}
