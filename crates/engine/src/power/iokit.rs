use std::ffi::CStr;

use anyhow::Result;
use ludolph_relay_proto::{Battery, BatteryState};
use objc2_core_foundation::{
    CFArray, CFBoolean, CFDictionary, CFNumber, CFRetained, CFString, CFType,
};
use objc2_io_kit::{
    IOPSCopyPowerSourcesInfo, IOPSCopyPowerSourcesList, IOPSGetPowerSourceDescription,
    kIOPSACPowerValue, kIOPSCurrentCapacityKey, kIOPSInternalBatteryType, kIOPSIsChargingKey,
    kIOPSMaxCapacityKey, kIOPSNotifyAnyPowerSource, kIOPSPowerSourceStateKey, kIOPSTypeKey,
};

use crate::notify::Changes;

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
        (true, false) => BatteryState::Plugged,
    };
    Some(Battery { percent, state })
}

/// Use the broad IOKit key: kIOPSNotifyPowerSource misses charge-level changes.
pub(crate) fn changes() -> Result<Changes> {
    Changes::new(kIOPSNotifyAnyPowerSource)
}
