//! Hardware brightness for the Mac’s built-in display.
#[cfg(target_os = "macos")]
#[expect(unsafe_code, reason = "DisplayServices has no public SDK binding")]
mod macos {
    use std::sync::OnceLock;

    use core_graphics::display::CGDisplay;
    use libloading::Library;

    type GetBrightness = unsafe extern "C" fn(u32, *mut f32) -> i32;
    type SetBrightness = unsafe extern "C" fn(u32, f32) -> i32;
    type CanChangeBrightness = unsafe extern "C" fn(u32) -> bool;

    struct Api {
        _library: Library,
        get: GetBrightness,
        set: SetBrightness,
        can_change: Option<CanChangeBrightness>,
    }

    fn api() -> Option<&'static Api> {
        static API: OnceLock<Option<Api>> = OnceLock::new();
        API.get_or_init(|| {
            // SAFETY: Load the system framework from its fixed system path.
            // The library is kept alive alongside the resolved functions.
            let library = unsafe {
                Library::new(
                    "/System/Library/PrivateFrameworks/DisplayServices.framework/DisplayServices",
                )
                .ok()?
            };
            // SAFETY: These are the DisplayServices C signatures for a
            // CoreGraphics display ID and normalized Float32 brightness.
            let (get, set, can_change) = unsafe {
                (
                    *library
                        .get::<GetBrightness>(b"DisplayServicesGetBrightness\0")
                        .ok()?,
                    *library
                        .get::<SetBrightness>(b"DisplayServicesSetBrightness\0")
                        .ok()?,
                    library
                        .get::<CanChangeBrightness>(b"DisplayServicesCanChangeBrightness\0")
                        .ok()
                        .map(|symbol| *symbol),
                )
            };
            Some(Api {
                _library: library,
                get,
                set,
                can_change,
            })
        })
        .as_ref()
    }

    pub(crate) struct DisplayBrightness {
        display: u32,
        pub(crate) fraction: f32,
    }

    impl DisplayBrightness {
        pub(crate) fn read(display: u32) -> Option<Self> {
            if !CGDisplay::new(display).is_builtin() {
                return None;
            }
            let api = api()?;
            if api.can_change.is_some_and(|check| {
                // SAFETY: The function only queries a display ID from CoreGraphics.
                !unsafe { check(display) }
            }) {
                return None;
            }
            let mut fraction = 0.0;
            // SAFETY: The out pointer is valid for one f32.
            let status = unsafe { (api.get)(display, &raw mut fraction) };
            (status == 0 && (0.0..=1.0).contains(&fraction)).then_some(Self { display, fraction })
        }

        pub(crate) fn write(&mut self, fraction: f32) -> bool {
            if !CGDisplay::new(self.display).is_builtin() {
                return false;
            }
            let Some(api) = api() else { return false };
            // SAFETY: DisplayServices accepts the original display ID and
            // a normalized brightness value.
            let status = unsafe { (api.set)(self.display, fraction.clamp(0.0, 1.0)) };
            if status == 0 {
                self.fraction = fraction;
            }
            status == 0
        }
    }
}

#[cfg(target_os = "macos")]
pub(crate) use macos::DisplayBrightness;

#[cfg(not(target_os = "macos"))]
pub(crate) struct DisplayBrightness {
    pub(crate) fraction: f32,
}

#[cfg(not(target_os = "macos"))]
impl DisplayBrightness {
    pub(crate) fn read(_: u32) -> Option<Self> {
        None
    }

    pub(crate) fn write(&mut self, _: f32) -> bool {
        false
    }
}
