//! Windows Hello.

use std::thread;

use windows::{
    Security::Credentials::UI::{UserConsentVerificationResult, UserConsentVerifier, UserConsentVerifierAvailability},
    Win32::{Foundation::HWND, System::WinRT::IUserConsentVerifierInterop},
    core::{HSTRING, factory},
};
use windows_future::IAsyncOperation;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Verdict {
    Yes,
    No,
    Unavailable,
}

pub fn available() -> bool {
    unsafe {
        let _ = windows::Win32::System::Com::CoInitializeEx(None, windows::Win32::System::Com::COINIT_MULTITHREADED);
    }
    UserConsentVerifier::CheckAvailabilityAsync()
        .and_then(|op| op.join())
        .is_ok_and(|a| a == UserConsentVerifierAvailability::Available)
}

pub fn check(done: impl FnOnce(bool) + Send + 'static) {
    let _ = thread::Builder::new().name("lumen-hello".into()).spawn(move || done(available()));
}

pub fn ask(hwnd: isize, why: &str, done: impl FnOnce(Verdict) + Send + 'static) {
    let why = HSTRING::from(why);
    let _ = thread::Builder::new().name("lumen-hello".into()).spawn(move || done(verify(hwnd, &why)));
}

fn verify(hwnd: isize, why: &HSTRING) -> Verdict {
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
