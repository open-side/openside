//! Biometric presence check for "Unlock with biometrics", through the nearest OS mechanism:
//! fprintd on the system D-Bus (Linux; its own polkit action
//! `net.reactivated.fprint.device.verify` admits the active local session) and
//! LocalAuthentication Touch ID (macOS). Elsewhere it is unsupported.
//!
//! A successful check proves a person enrolled on this computer touched the sensor just now.
//! It derives no key: the vault still opens with its own unlock method, the check only gates
//! that unlock (and fresh-authentication steps such as export).

use std::time::Duration;

/// Which OS mechanism verifies.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum BiometricMechanism {
    /// fprintd fingerprint verification (Linux).
    Fprintd,
    /// LocalAuthentication Touch ID (macOS).
    LocalAuthentication,
}

impl BiometricMechanism {
    /// Stable snake_case name.
    pub fn wire(self) -> &'static str {
        match self {
            Self::Fprintd => "fprintd",
            Self::LocalAuthentication => "local_authentication",
        }
    }
}

/// Why a check did not pass. Value-free.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum BiometricError {
    /// No usable mechanism: `unsupported_platform`, `no_sensor`, `not_enrolled` or
    /// `service_unavailable`.
    Unavailable(&'static str),
    /// The finger or face did not match, or the person cancelled.
    NoMatch,
    /// Nobody answered in time.
    Timeout,
    /// The sensor or service failed.
    Failed,
}

impl BiometricError {
    /// Stable snake_case reason.
    pub fn wire(self) -> &'static str {
        match self {
            Self::Unavailable(reason) => reason,
            Self::NoMatch => "biometric_no_match",
            Self::Timeout => "biometric_timeout",
            Self::Failed => "biometric_failed",
        }
    }
}

/// Whether this computer can verify a person biometrically right now, without asking them.
/// Blocking (D-Bus or LocalAuthentication); call off async runtimes.
pub fn biometric_probe() -> Result<BiometricMechanism, BiometricError> {
    imp::probe()
}

/// Ask the person to verify (touch the sensor). `reason` is shown by the OS prompt where it
/// has one (macOS). Blocking for up to `timeout`; call off async runtimes.
pub fn biometric_verify(reason: &str, timeout: Duration) -> Result<(), BiometricError> {
    imp::verify(reason, timeout)
}

#[cfg(target_os = "linux")]
mod imp {
    use std::{sync::mpsc, time::Duration};

    use zbus::blocking::{Connection, Proxy};
    use zvariant::OwnedObjectPath;

    use super::{BiometricError, BiometricMechanism};

    const SERVICE: &str = "net.reactivated.Fprint";
    const MANAGER_PATH: &str = "/net/reactivated/Fprint/Manager";
    const MANAGER: &str = "net.reactivated.Fprint.Manager";
    const DEVICE: &str = "net.reactivated.Fprint.Device";

    fn device(connection: &Connection) -> Result<Proxy<'static>, BiometricError> {
        let manager = Proxy::new(connection, SERVICE, MANAGER_PATH, MANAGER)
            .map_err(|_| BiometricError::Unavailable("service_unavailable"))?;
        let path: OwnedObjectPath = manager
            .call("GetDefaultDevice", &())
            .map_err(|_| BiometricError::Unavailable("no_sensor"))?;
        let device = Proxy::new(connection, SERVICE, path, DEVICE)
            .map_err(|_| BiometricError::Unavailable("service_unavailable"))?;
        // An empty user name means the calling process's user.
        let fingers: Vec<String> = device
            .call("ListEnrolledFingers", &("",))
            .map_err(|_| BiometricError::Unavailable("not_enrolled"))?;
        if fingers.is_empty() {
            return Err(BiometricError::Unavailable("not_enrolled"));
        }
        Ok(device)
    }

    fn system() -> Result<Connection, BiometricError> {
        Connection::system().map_err(|_| BiometricError::Unavailable("service_unavailable"))
    }

    pub(super) fn probe() -> Result<BiometricMechanism, BiometricError> {
        device(&system()?).map(|_| BiometricMechanism::Fprintd)
    }

    pub(super) fn verify(_reason: &str, timeout: Duration) -> Result<(), BiometricError> {
        let connection = system()?;
        let device = device(&connection)?;
        device
            .call_method("Claim", &("",))
            .map_err(|_| BiometricError::Failed)?;
        let outcome = (|| {
            // Subscribe before starting, so no status can be missed.
            let statuses = device
                .receive_signal("VerifyStatus")
                .map_err(|_| BiometricError::Failed)?;
            device
                .call_method("VerifyStart", &("any",))
                .map_err(|_| BiometricError::Failed)?;
            // The signal iterator has no deadline; closing the connection ends it, and fprintd
            // releases a device whose client went away.
            let (finished, deadline) = mpsc::channel::<()>();
            let watchdog = connection.clone();
            let guard = std::thread::spawn(move || {
                if matches!(
                    deadline.recv_timeout(timeout),
                    Err(mpsc::RecvTimeoutError::Timeout)
                ) {
                    let _ = watchdog.close();
                    true
                } else {
                    false
                }
            });
            let mut result = Err(BiometricError::Timeout);
            for message in statuses {
                let Ok((status, done)) = message.body().deserialize::<(String, bool)>() else {
                    result = Err(BiometricError::Failed);
                    break;
                };
                match status.as_str() {
                    "verify-match" => {
                        result = Ok(());
                        break;
                    }
                    "verify-no-match" => {
                        result = Err(BiometricError::NoMatch);
                        if done {
                            break;
                        }
                    }
                    "verify-disconnected" | "verify-unknown-error" => {
                        result = Err(BiometricError::Failed);
                        break;
                    }
                    // Retry prompts (`verify-retry-scan`, `verify-swipe-too-short`, ...).
                    _ if done => break,
                    _ => {}
                }
            }
            let _ = finished.send(());
            if guard.join().unwrap_or(true) {
                return Err(BiometricError::Timeout);
            }
            let _ = device.call_method("VerifyStop", &());
            result
        })();
        let _ = device.call_method("Release", &());
        outcome
    }
}

// LocalAuthentication is reached through Objective-C FFI; every call site states its contract.
#[cfg(target_os = "macos")]
#[allow(unsafe_code)]
mod imp {
    use std::{sync::mpsc, time::Duration};

    use block2::RcBlock;
    use objc2::runtime::Bool;
    use objc2_foundation::{NSError, NSString};
    use objc2_local_authentication::{LAContext, LAPolicy};

    use super::{BiometricError, BiometricMechanism};

    const POLICY: LAPolicy = LAPolicy::DeviceOwnerAuthenticationWithBiometrics;

    pub(super) fn probe() -> Result<BiometricMechanism, BiometricError> {
        // SAFETY: a fresh context, used on this thread only.
        let context = unsafe { LAContext::new() };
        // SAFETY: plain query without a prompt.
        unsafe { context.canEvaluatePolicy_error(POLICY) }
            .map(|()| BiometricMechanism::LocalAuthentication)
            .map_err(|_| BiometricError::Unavailable("not_enrolled"))
    }

    pub(super) fn verify(reason: &str, timeout: Duration) -> Result<(), BiometricError> {
        // SAFETY: a fresh context owned by this call.
        let context = unsafe { LAContext::new() };
        // SAFETY: plain query without a prompt.
        if unsafe { context.canEvaluatePolicy_error(POLICY) }.is_err() {
            return Err(BiometricError::Unavailable("not_enrolled"));
        }
        let (answer, answered) = mpsc::channel::<bool>();
        let reply = RcBlock::new(move |success: Bool, _error: *mut NSError| {
            let _ = answer.send(success.as_bool());
        });
        let reason = NSString::from_str(reason);
        // SAFETY: the reply block only sends on a channel, which is safe from any thread.
        unsafe { context.evaluatePolicy_localizedReason_reply(POLICY, &reason, &reply) };
        match answered.recv_timeout(timeout) {
            Ok(true) => Ok(()),
            Ok(false) => Err(BiometricError::NoMatch),
            Err(_) => {
                // SAFETY: dismisses the prompt; a late reply only hits a closed channel.
                unsafe { context.invalidate() };
                Err(BiometricError::Timeout)
            }
        }
    }
}

#[cfg(not(any(target_os = "linux", target_os = "macos")))]
mod imp {
    use std::time::Duration;

    use super::{BiometricError, BiometricMechanism};

    pub(super) fn probe() -> Result<BiometricMechanism, BiometricError> {
        Err(BiometricError::Unavailable("unsupported_platform"))
    }

    pub(super) fn verify(_reason: &str, _timeout: Duration) -> Result<(), BiometricError> {
        Err(BiometricError::Unavailable("unsupported_platform"))
    }
}
