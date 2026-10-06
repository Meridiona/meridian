//ambient dev tool that watches what you do and updates your PM tickets automatically, boosting developer productivity
//! A small, safe wrapper over the macOS Accessibility C API.
//!
//! Only the handful of calls compose needs are declared, so there is no extra dependency
//! on a bindings crate. Everything unsafe is confined to this file: an [`Element`] owns
//! one retained `AXUIElementRef` and releases it on drop, so a reference can never leak or
//! be released twice, and no raw pointer escapes.
//!
//! # Messaging timeout
//! Every attribute read is synchronous IPC into the other app. A hung app would hang us,
//! so every element is created with a short messaging timeout ([`MESSAGING_TIMEOUT_S`]).
//!
//! # Who calls this
//! [`super::reader`] to read a field and [`super::writer`] to change it.
//!
//! # Related
//! - [`super::ranges`] - the UTF-16 maths for the ranges read and written here.

use std::ffi::c_void;

use core_foundation::base::{
    CFEqual, CFGetTypeID, CFRange, CFRelease, CFRetain, CFType, CFTypeRef, TCFType,
};
use core_foundation::boolean::CFBoolean;
use core_foundation::string::{CFString, CFStringRef};
use core_foundation::url::CFURL;

use super::ranges::Utf16Range;

/// How long one Accessibility call may block before we give up on it.
pub const MESSAGING_TIMEOUT_S: f32 = 1.5;

type AXError = i32;
const AX_SUCCESS: AXError = 0;
const AX_VALUE_TYPE_CFRANGE: u32 = 4;

#[link(name = "ApplicationServices", kind = "framework")]
extern "C" {
    fn AXIsProcessTrusted() -> bool;
    fn AXUIElementCreateSystemWide() -> CFTypeRef;
    fn AXUIElementCreateApplication(pid: i32) -> CFTypeRef;
    fn AXUIElementCopyAttributeValue(
        element: CFTypeRef,
        attribute: CFStringRef,
        value: *mut CFTypeRef,
    ) -> AXError;
    fn AXUIElementSetAttributeValue(
        element: CFTypeRef,
        attribute: CFStringRef,
        value: CFTypeRef,
    ) -> AXError;
    fn AXUIElementGetPid(element: CFTypeRef, pid: *mut i32) -> AXError;
    fn AXUIElementSetMessagingTimeout(element: CFTypeRef, seconds: f32) -> AXError;
    fn AXValueCreate(value_type: u32, value: *const c_void) -> CFTypeRef;
    fn AXValueGetValue(value: CFTypeRef, value_type: u32, out: *mut c_void) -> bool;
}

/// True when this process has been granted Accessibility access.
pub fn is_trusted() -> bool {
    // SAFETY: a plain query with no arguments.
    unsafe { AXIsProcessTrusted() }
}

/// An Accessibility failure, reduced to what callers branch on.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum AxError {
    /// Accessibility is not granted to this process.
    Disabled,
    /// The element no longer exists.
    Invalid,
    /// The attribute does not exist on this element, or has no value.
    Unsupported,
    /// The app could not complete the request (it is busy, hung, or went away mid-call).
    Timeout,
    Other(i32),
}

impl AxError {
    fn from_code(code: AXError) -> AxError {
        match code {
            -25211 => AxError::Disabled,
            -25202 => AxError::Invalid,
            // kAXErrorAttributeUnsupported, kAXErrorNoValue, kAXErrorParameterizedAttributeUnsupported
            -25205 | -25212 | -25213 => AxError::Unsupported,
            // kAXErrorCannotComplete: the app did not answer.
            -25204 => AxError::Timeout,
            other => AxError::Other(other),
        }
    }
}

/// One retained `AXUIElementRef`.
pub struct Element(CFTypeRef);

// SAFETY: an AXUIElementRef is an immutable, thread-safe CoreFoundation object; calls on it
// are routed to the target app by the system. It is moved between threads, never shared.
unsafe impl Send for Element {}

impl Clone for Element {
    fn clone(&self) -> Self {
        // SAFETY: self.0 is a valid retained reference; this adds one retain for the clone.
        unsafe { CFRetain(self.0) };
        Element(self.0)
    }
}

impl Drop for Element {
    fn drop(&mut self) {
        // SAFETY: balances the retain this Element owns.
        unsafe { CFRelease(self.0) };
    }
}

impl Element {
    /// Take ownership of a +1 reference (from a Create or Copy function). `None` for null.
    fn from_owned(raw: CFTypeRef) -> Option<Element> {
        if raw.is_null() {
            return None;
        }
        // SAFETY: raw is a valid +1 reference we now own.
        unsafe { AXUIElementSetMessagingTimeout(raw, MESSAGING_TIMEOUT_S) };
        Some(Element(raw))
    }

    /// The system-wide element, the root for finding what has focus.
    pub fn system_wide() -> Option<Element> {
        // SAFETY: returns a +1 reference or null.
        Element::from_owned(unsafe { AXUIElementCreateSystemWide() })
    }

    /// The application element for a process.
    pub fn application(pid: i32) -> Option<Element> {
        // SAFETY: returns a +1 reference or null.
        Element::from_owned(unsafe { AXUIElementCreateApplication(pid) })
    }

    fn copy_attribute(&self, name: &str) -> Result<CFType, AxError> {
        let attr = CFString::new(name);
        let mut out: CFTypeRef = std::ptr::null();
        // SAFETY: self.0 and attr are valid; `out` is written only on success.
        let code =
            unsafe { AXUIElementCopyAttributeValue(self.0, attr.as_concrete_TypeRef(), &mut out) };
        if code != AX_SUCCESS {
            return Err(AxError::from_code(code));
        }
        if out.is_null() {
            return Err(AxError::Unsupported);
        }
        // SAFETY: Copy functions return +1, which wrap_under_create_rule takes over.
        Ok(unsafe { CFType::wrap_under_create_rule(out) })
    }

    /// A string attribute, or `None` when absent or not a string.
    pub fn string(&self, name: &str) -> Option<String> {
        self.copy_attribute(name)
            .ok()?
            .downcast::<CFString>()
            .map(|s| s.to_string())
    }

    /// An attribute holding another element (`AXFocusedUIElement`, `AXParent`, ...).
    pub fn element(&self, name: &str) -> Option<Element> {
        let value = self.copy_attribute(name).ok()?;
        let raw = value.as_CFTypeRef();
        // The attribute value is an AXUIElement only when its type id matches; checking it
        // before use stops a string or number being treated as an element.
        if !is_ax_element(raw) {
            return None;
        }
        // SAFETY: retain once so the Element owns its own reference independent of `value`.
        unsafe { CFRetain(raw) };
        Element::from_owned(raw)
    }

    /// An attribute holding a list of elements (`AXChildren`).
    pub fn elements(&self, name: &str) -> Vec<Element> {
        let Ok(value) = self.copy_attribute(name) else {
            return Vec::new();
        };
        let Some(array) = value.downcast::<core_foundation::array::CFArray<*const c_void>>() else {
            return Vec::new();
        };
        array
            .iter()
            .filter_map(|item| {
                let raw: CFTypeRef = *item;
                if !is_ax_element(raw) {
                    return None;
                }
                // SAFETY: the array owns its items; retain so this Element outlives the array.
                unsafe { CFRetain(raw) };
                Element::from_owned(raw)
            })
            .collect()
    }

    /// A page address held in an attribute (`AXURL`), as a string.
    pub fn url(&self, name: &str) -> Option<String> {
        let value = self.copy_attribute(name).ok()?;
        if let Some(url) = value.downcast::<CFURL>() {
            return Some(url.get_string().to_string());
        }
        value.downcast::<CFString>().map(|s| s.to_string())
    }

    /// A range attribute (`AXSelectedTextRange`) in UTF-16 units.
    pub fn range(&self, name: &str) -> Option<Utf16Range> {
        let value = self.copy_attribute(name).ok()?;
        let mut range = CFRange {
            location: 0,
            length: 0,
        };
        // SAFETY: AXValueGetValue checks the value's type before writing to `range`.
        let ok = unsafe {
            AXValueGetValue(
                value.as_CFTypeRef(),
                AX_VALUE_TYPE_CFRANGE,
                &mut range as *mut CFRange as *mut c_void,
            )
        };
        if !ok || range.location < 0 || range.length < 0 {
            return None;
        }
        Some(Utf16Range {
            location: range.location as usize,
            length: range.length as usize,
        })
    }

    /// The process that owns this element.
    pub fn pid(&self) -> Option<i32> {
        let mut pid = 0i32;
        // SAFETY: valid element and out pointer.
        let code = unsafe { AXUIElementGetPid(self.0, &mut pid) };
        (code == AX_SUCCESS && pid > 0).then_some(pid)
    }

    /// True when both refer to the same on-screen element.
    pub fn same_as(&self, other: &Element) -> bool {
        // SAFETY: both are valid CF references.
        unsafe { CFEqual(self.0, other.0) != 0 }
    }

    /// True when the element can still be queried. A destroyed field answers `Invalid`.
    pub fn is_alive(&self) -> bool {
        !matches!(
            self.copy_attribute("AXRole"),
            Err(AxError::Invalid) | Err(AxError::Disabled)
        )
    }

    /// Set a string attribute (`AXSelectedText`, `AXValue`).
    pub fn set_string(&self, name: &str, value: &str) -> Result<(), AxError> {
        let attr = CFString::new(name);
        let v = CFString::new(value);
        // SAFETY: valid element, attribute and value.
        let code = unsafe {
            AXUIElementSetAttributeValue(self.0, attr.as_concrete_TypeRef(), v.as_CFTypeRef())
        };
        if code == AX_SUCCESS {
            Ok(())
        } else {
            Err(AxError::from_code(code))
        }
    }

    /// Set a boolean attribute (`AXManualAccessibility`).
    pub fn set_bool(&self, name: &str, value: bool) -> Result<(), AxError> {
        let attr = CFString::new(name);
        let v = if value {
            CFBoolean::true_value()
        } else {
            CFBoolean::false_value()
        };
        // SAFETY: valid element, attribute and value.
        let code = unsafe {
            AXUIElementSetAttributeValue(self.0, attr.as_concrete_TypeRef(), v.as_CFTypeRef())
        };
        if code == AX_SUCCESS {
            Ok(())
        } else {
            Err(AxError::from_code(code))
        }
    }

    /// Set a range attribute (`AXSelectedTextRange`).
    pub fn set_range(&self, name: &str, range: Utf16Range) -> Result<(), AxError> {
        let attr = CFString::new(name);
        let cf = CFRange {
            location: range.location as isize,
            length: range.length as isize,
        };
        // SAFETY: AXValueCreate copies the struct; the returned +1 value is released below.
        let value = unsafe {
            AXValueCreate(
                AX_VALUE_TYPE_CFRANGE,
                &cf as *const CFRange as *const c_void,
            )
        };
        if value.is_null() {
            return Err(AxError::Other(-1));
        }
        // SAFETY: valid element, attribute and value; `value` is released after the call.
        let code =
            unsafe { AXUIElementSetAttributeValue(self.0, attr.as_concrete_TypeRef(), value) };
        unsafe { CFRelease(value) };
        if code == AX_SUCCESS {
            Ok(())
        } else {
            Err(AxError::from_code(code))
        }
    }
}

#[link(name = "ApplicationServices", kind = "framework")]
extern "C" {
    fn AXUIElementGetTypeID() -> usize;
}

fn is_ax_element(raw: CFTypeRef) -> bool {
    // SAFETY: both calls only read type information.
    unsafe { CFGetTypeID(raw) == AXUIElementGetTypeID() }
}

impl std::fmt::Debug for Element {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str("Element(..)")
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn error_codes_map_to_what_callers_branch_on() {
        assert_eq!(AxError::from_code(-25211), AxError::Disabled);
        assert_eq!(AxError::from_code(-25202), AxError::Invalid);
        for unsupported in [-25205, -25212, -25213] {
            assert_eq!(AxError::from_code(unsupported), AxError::Unsupported);
        }
        assert_eq!(AxError::from_code(-25204), AxError::Timeout);
        // Not enough precision and the generic failure are neither of the above.
        assert_eq!(AxError::from_code(-25214), AxError::Other(-25214));
        assert_eq!(AxError::from_code(-25200), AxError::Other(-25200));
    }
}
