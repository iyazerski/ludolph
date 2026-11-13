//! This Mac's battery, which the phone shows beside the Mac's name. IOKit
//! reports it, and a Darwin notification says when it changes, so nothing
//! polls.

use std::ffi::{CStr, c_char, c_int};
use std::os::fd::RawFd;

use anyhow::{Result, bail};
use ludolph_relay_proto::{Battery, BatteryState};
use objc2_core_foundation::{
    CFArray, CFBoolean, CFDictionary, CFNumber, CFRetained, CFString, CFType,
};
use objc2_io_kit::{
    IOPSCopyPowerSourcesInfo, IOPSCopyPowerSourcesList, IOPSGetPowerSourceDescription,
    kIOPSACPowerValue, kIOPSCurrentCapacityKey, kIOPSInternalBatteryType, kIOPSIsChargingKey,
    kIOPSMaxCapacityKey, kIOPSNotifyAnyPowerSource, kIOPSPowerSourceStateKey, kIOPSTypeKey,
};
use tokio::io::Interest;
use tokio::io::unix::AsyncFd;

// `notify(3)`, part of libSystem, which the `libc` crate leaves out.
unsafe extern "C" {
    fn notify_register_file_descriptor(
        name: *const c_char,
        notify_fd: *mut c_int,
        flags: c_int,
        out_token: *mut c_int,
    ) -> u32;
    fn notify_cancel(token: c_int) -> u32;
}

/// The battery now; `None` on a Mac without one.
pub fn battery() -> Option<Battery> {
    let info = IOPSCopyPowerSourcesInfo()?;
    // SAFETY: `info` is the blob `IOPSCopyPowerSourcesInfo` returned.
    let list = unsafe { IOPSCopyPowerSourcesList(Some(&info)) }?;
    // SAFETY: the list holds power source handles, which are CF objects.
    let list = unsafe { CFRetained::cast_unchecked::<CFArray<CFType>>(list) };
    list.iter().find_map(|source| {
        // SAFETY: `source` is one of `info`'s power sources.
        let description = unsafe { IOPSGetPowerSourceDescription(Some(&info), Some(&source)) }?;
        // SAFETY: IOKit's descriptions map string keys to CF values.
        let description =
            unsafe { CFRetained::cast_unchecked::<CFDictionary<CFString, CFType>>(description) };
        from_description(&description)
    })
}

/// The battery a power source describes, if it's the internal battery.
fn from_description(description: &CFDictionary<CFString, CFType>) -> Option<Battery> {
    let value = |key: &CStr| description.get(&CFString::from_str(key.to_str().ok()?));
    let text = |key| {
        value(key)?
            .downcast::<CFString>()
            .ok()
            .map(|s| s.to_string())
    };
    let number = |key| value(key)?.downcast::<CFNumber>().ok()?.as_i64();
    if text(kIOPSTypeKey)? != kIOPSInternalBatteryType.to_str().ok()? {
        return None;
    }
    let (current, max) = (
        number(kIOPSCurrentCapacityKey)?,
        number(kIOPSMaxCapacityKey)?,
    );
    let percent = u8::try_from(current * 100 / max.max(1)).ok()?.min(100);
    let charging = value(kIOPSIsChargingKey)
        .and_then(|v| v.downcast::<CFBoolean>().ok())
        .is_some_and(|v| v.as_bool());
    let plugged = text(kIOPSPowerSourceStateKey)? == kIOPSACPowerValue.to_str().ok()?;
    let state = match (plugged, charging) {
        (false, _) => BatteryState::Unplugged,
        (true, true) => BatteryState::Charging,
        // Full, or macOS holds the charge.
        (true, false) => BatteryState::Plugged,
    };
    Some(Battery { percent, state })
}

/// Wakes when any power source changes: plugged in, unplugged, or a new
/// charge level. It's the broad key, since `kIOPSNotifyPowerSource`
/// stays quiet while the charge level changes. That's about once a minute
/// while charging or on battery.
#[derive(Debug)]
pub struct Changes {
    /// `None` only while dropping.
    fd: Option<AsyncFd<RawFd>>,
    token: c_int,
}

impl Changes {
    /// Start listening.
    ///
    /// # Errors
    ///
    /// Fails if the notification can't be registered.
    pub fn new() -> Result<Self> {
        let (mut fd, mut token) = (-1, -1);
        // SAFETY: the name is NUL-terminated, and both out pointers are
        // valid. Flags 0 asks for a new descriptor.
        let status = unsafe {
            notify_register_file_descriptor(
                kIOPSNotifyAnyPowerSource.as_ptr(),
                &raw mut fd,
                0,
                &raw mut token,
            )
        };
        if status != 0 {
            bail!("registering for power changes failed with status {status}");
        }
        let mut changes = Changes { fd: None, token };
        // SAFETY: `fd` is the open descriptor notify(3) just made.
        if unsafe { libc::fcntl(fd, libc::F_SETFL, libc::O_NONBLOCK) } == -1 {
            let error = std::io::Error::last_os_error();
            bail!("can't make the power notification non-blocking: {error}");
        }
        // SAFETY: `fd` stays open until `drop` cancels the registration,
        // which is after the `AsyncFd` is gone.
        let fd = unsafe { AsyncFd::register_with_interest(fd, Interest::READABLE) };
        changes.fd = Some(fd.map_err(|e| e.into_parts().1)?);
        Ok(changes)
    }

    /// Wait for the next change.
    ///
    /// # Errors
    ///
    /// Fails if the descriptor can't be read.
    pub async fn next(&mut self) -> Result<()> {
        let fd = self.fd.as_ref().expect("set until dropped");
        loop {
            let mut ready = fd.readable().await?;
            // Each notice is the token as 4 bytes. Reading them all makes a
            // burst one change.
            let mut drained = false;
            loop {
                let mut buffer = [0u8; 64];
                // SAFETY: `buffer` is valid for its length.
                let read =
                    unsafe { libc::read(*fd.get_ref(), buffer.as_mut_ptr().cast(), buffer.len()) };
                match read {
                    n if n > 0 => drained = true,
                    0 => bail!("the power notification closed"),
                    _ => {
                        let error = std::io::Error::last_os_error();
                        if error.kind() != std::io::ErrorKind::WouldBlock {
                            return Err(error.into());
                        }
                        ready.clear_ready();
                        break;
                    }
                }
            }
            if drained {
                return Ok(());
            }
        }
    }
}

impl Drop for Changes {
    fn drop(&mut self) {
        // Canceling the last registration on a descriptor closes it, so the
        // `AsyncFd` deregisters first. It doesn't close it, since a `RawFd`
        // doesn't own it.
        drop(self.fd.take());
        // SAFETY: `token` came from `notify_register_file_descriptor`.
        unsafe { notify_cancel(self.token) };
    }
}
