//! macOS output volume for the ring’s local slider. This uses the system
//! audio device directly and does not capture audio or synthesize input.

#[cfg(target_os = "macos")]
#[expect(unsafe_code, reason = "CoreAudio C property ABI")]
mod macos {
    use std::{ffi::c_void, ptr};

    #[repr(C)]
    struct Address {
        selector: u32,
        scope: u32,
        element: u32,
    }

    const DEFAULT_OUTPUT: Address = Address {
        selector: u32::from_be_bytes(*b"dOut"),
        scope: u32::from_be_bytes(*b"glob"),
        element: 0,
    };
    // Virtual main volume also handles devices with independent stereo
    // controls, preserving their balance.
    const VOLUME: Address = Address {
        selector: u32::from_be_bytes(*b"vmvc"),
        scope: u32::from_be_bytes(*b"outp"),
        element: 0,
    };

    #[link(name = "CoreAudio", kind = "framework")]
    unsafe extern "C" {
        fn AudioObjectGetPropertyData(
            object: u32,
            address: *const Address,
            qualifier_size: u32,
            qualifier: *const c_void,
            size: *mut u32,
            data: *mut c_void,
        ) -> i32;
        fn AudioObjectIsPropertySettable(
            object: u32,
            address: *const Address,
            settable: *mut u8,
        ) -> i32;
        fn AudioObjectSetPropertyData(
            object: u32,
            address: *const Address,
            qualifier_size: u32,
            qualifier: *const c_void,
            size: u32,
            data: *const c_void,
        ) -> i32;
    }

    pub(crate) struct OutputVolume {
        device: u32,
        pub(crate) fraction: f32,
    }

    impl OutputVolume {
        fn default_device() -> Option<u32> {
            let mut device = 0u32;
            let mut size = 4;
            // SAFETY: The system object is 1. The property writes one
            // AudioDeviceID (u32) into the supplied four-byte buffer.
            let status = unsafe {
                AudioObjectGetPropertyData(
                    1,
                    &DEFAULT_OUTPUT,
                    0,
                    ptr::null(),
                    &mut size,
                    ptr::from_mut(&mut device).cast(),
                )
            };
            (status == 0 && size == 4 && device != 0).then_some(device)
        }

        pub(crate) fn read() -> Option<Self> {
            let device = Self::default_device()?;
            let mut settable = 0u8;
            // SAFETY: The out pointer refers to a writable CoreAudio Boolean.
            if unsafe { AudioObjectIsPropertySettable(device, &VOLUME, &mut settable) } != 0
                || settable == 0
            {
                return None;
            }
            let mut fraction = 0.0f32;
            let mut size = 4;
            // SAFETY: Virtual main volume is one Float32. The buffer and
            // writable size match the property, and there is no qualifier.
            let status = unsafe {
                AudioObjectGetPropertyData(
                    device,
                    &VOLUME,
                    0,
                    ptr::null(),
                    &mut size,
                    ptr::from_mut(&mut fraction).cast(),
                )
            };
            (status == 0 && size == 4 && (0.0..=1.0).contains(&fraction))
                .then_some(Self { device, fraction })
        }

        pub(crate) fn write(&mut self, fraction: f32) -> bool {
            if Self::default_device() != Some(self.device) {
                return false;
            }
            // SAFETY: The property expects one Float32 between 0 and 1.
            let fraction = fraction.clamp(0.0, 1.0);
            let status = unsafe {
                AudioObjectSetPropertyData(
                    self.device,
                    &VOLUME,
                    0,
                    ptr::null(),
                    4,
                    ptr::from_ref(&fraction).cast(),
                )
            };
            if status == 0 {
                self.fraction = fraction;
            }
            status == 0
        }
    }
}

#[cfg(target_os = "macos")]
pub(crate) use macos::OutputVolume;

#[cfg(not(target_os = "macos"))]
pub(crate) struct OutputVolume {
    pub(crate) fraction: f32,
}

#[cfg(not(target_os = "macos"))]
impl OutputVolume {
    pub(crate) fn read() -> Option<Self> {
        None
    }

    pub(crate) fn write(&mut self, _: f32) -> bool {
        false
    }
}
