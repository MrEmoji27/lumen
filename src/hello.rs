//! Windows Hello: the person's face, finger or PIN, before Lumen shows what they locked (Lumen
//! itself, a tab, the secret downloads).
//!
//! The prompt is Windows' own and has to be: anything can draw a card that looks like a PIN
//! box, and only Windows can check a face or a finger. Everything around it (the locked tab,
//! the lock screen, the button that asks) is drawn by Lumen.
//!
//! Both calls wait on Windows, so both run on a thread of their own and answer through a
//! callback. The UI thread never waits for a face.

use std::thread;

use windows::{
    Security::Credentials::UI::{UserConsentVerificationResult, UserConsentVerifier, UserConsentVerifierAvailability},
    Win32::{Foundation::HWND, System::WinRT::IUserConsentVerifierInterop},
    core::{HSTRING, factory},
};
use windows_future::IAsyncOperation;

/// What the person's answer came to.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Verdict {
    /// Windows says it is them.
    Yes,
    /// They cancelled, or it was not them.
    No,
    /// This PC has no Windows Hello set up: no face, finger or PIN to ask for.
    Unavailable,
}

/// Is Windows Hello set up on this PC? Blocking: call it off the window's thread.
pub fn available() -> bool {
    // A WinRT call needs this thread in an apartment; an already-initialized one keeps its own.
    unsafe {
        let _ = windows::Win32::System::Com::CoInitializeEx(None, windows::Win32::System::Com::COINIT_MULTITHREADED);
    }
    UserConsentVerifier::CheckAvailabilityAsync()
        .and_then(|op| op.join())
        .is_ok_and(|a| a == UserConsentVerifierAvailability::Available)
}

/// Is Windows Hello set up on this PC? Answers on a thread of its own.
pub fn check(done: impl FnOnce(bool) + Send + 'static) {
    let _ = thread::Builder::new().name("lumen-hello".into()).spawn(move || done(available()));
}

/// Ask Windows Hello, over Lumen's window, with `why` as the line the prompt shows.
pub fn ask(hwnd: isize, why: &str, done: impl FnOnce(Verdict) + Send + 'static) {
    let why = HSTRING::from(why);
    let _ = thread::Builder::new().name("lumen-hello".into()).spawn(move || done(verify(hwnd, &why)));
}

fn verify(hwnd: isize, why: &HSTRING) -> Verdict {
    // A desktop app asks through the interop, so the prompt sits over its own window.
    let Ok(interop) = factory::<UserConsentVerifier, IUserConsentVerifierInterop>() else {
        return Verdict::Unavailable;
    };
    let op: windows::core::Result<IAsyncOperation<UserConsentVerificationResult>> =
        unsafe { interop.RequestVerificationForWindowAsync(HWND(hwnd as _), why) };
    match op.and_then(|op| op.join()) {
        Ok(r) => verdict(r),
        Err(_) => Verdict::Unavailable,
    }
}

/// Windows' answer, in Lumen's three words.
fn verdict(r: UserConsentVerificationResult) -> Verdict {
    match r {
        UserConsentVerificationResult::Verified => Verdict::Yes,
        UserConsentVerificationResult::DeviceNotPresent
        | UserConsentVerificationResult::NotConfiguredForUser
        | UserConsentVerificationResult::DisabledByPolicy => Verdict::Unavailable,
        _ => Verdict::No,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn only_verified_opens_anything() {
        assert_eq!(verdict(UserConsentVerificationResult::Verified), Verdict::Yes);
        assert_eq!(verdict(UserConsentVerificationResult::Canceled), Verdict::No);
        assert_eq!(verdict(UserConsentVerificationResult::RetriesExhausted), Verdict::No);
        assert_eq!(verdict(UserConsentVerificationResult::DeviceBusy), Verdict::No);
        assert_eq!(verdict(UserConsentVerificationResult::NotConfiguredForUser), Verdict::Unavailable);
    }
}
