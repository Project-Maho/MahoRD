//! Process-wide inhibition exists only while streaming sessions hold a guard.
use std::sync::Mutex;

#[derive(Default)]
struct State {
    sessions: usize,
    native: Option<Native>,
}

static STATE: Mutex<State> = Mutex::new(State {
    sessions: 0,
    native: None,
});
static WARNED: std::sync::atomic::AtomicBool = std::sync::atomic::AtomicBool::new(false);

pub(crate) struct StreamingGuard;

impl StreamingGuard {
    pub(crate) fn acquire() -> Self {
        let mut state = STATE.lock().unwrap_or_else(|e| e.into_inner());
        if state.sessions == 0 {
            match Native::acquire() {
                Ok(native) => state.native = Some(native),
                Err(error) => {
                    if !WARNED.swap(true, std::sync::atomic::Ordering::Relaxed) {
                        tracing::warn!(%error, "session sleep inhibition unavailable");
                    }
                }
            }
        }
        state.sessions += 1;
        Self
    }
}

impl Drop for StreamingGuard {
    fn drop(&mut self) {
        let mut state = STATE.lock().unwrap_or_else(|e| e.into_inner());
        state.sessions -= 1;
        if state.sessions == 0 {
            state.native = None;
        }
    }
}

#[cfg(target_os = "macos")]
struct Native(u32);

#[cfg(target_os = "macos")]
#[link(name = "IOKit", kind = "framework")]
unsafe extern "C" {
    fn IOPMAssertionCreateWithName(
        kind: core_foundation::string::CFStringRef,
        level: u32,
        name: core_foundation::string::CFStringRef,
        id: *mut u32,
    ) -> i32;
    fn IOPMAssertionRelease(id: u32) -> i32;
}

#[cfg(target_os = "macos")]
impl Native {
    fn acquire() -> std::io::Result<Self> {
        use core_foundation::{base::TCFType, string::CFString};
        let kind = CFString::new("PreventUserIdleDisplaySleep");
        let name = CFString::new("MahoRD active remote session");
        let mut id = 0;
        // SAFETY: CF strings and initialized output remain live for the call;
        // a successful assertion ID is released exactly once by Native::drop.
        let status = unsafe {
            IOPMAssertionCreateWithName(
                kind.as_concrete_TypeRef(),
                255,
                name.as_concrete_TypeRef(),
                &mut id,
            )
        };
        if status != 0 {
            return Err(std::io::Error::other(format!("IOPM assertion: {status}")));
        }
        Ok(Self(id))
    }
}

#[cfg(target_os = "macos")]
impl Drop for Native {
    fn drop(&mut self) {
        // SAFETY: self owns the successfully acquired assertion ID.
        let status = unsafe { IOPMAssertionRelease(self.0) };
        if status != 0 {
            tracing::warn!(status, "IOPM assertion release failed");
        }
    }
}

#[cfg(target_os = "linux")]
struct Native(std::process::Child);

#[cfg(target_os = "linux")]
impl Native {
    fn acquire() -> std::io::Result<Self> {
        use std::process::{Command, Stdio};
        let mut child = Command::new("systemd-inhibit")
            .args([
                "--what=idle:sleep",
                "--mode=block",
                "--who=MahoRD",
                "--why=Active remote desktop session",
                "sh",
                "-c",
                "printf 'READY\\n'; exec cat",
            ])
            .stdin(Stdio::piped())
            .stdout(Stdio::piped())
            .stderr(Stdio::inherit())
            .spawn()?;
        let output = child
            .stdout
            .take()
            .ok_or_else(|| std::io::Error::other("missing inhibitor pipe"))?;
        let (tx, rx) = std::sync::mpsc::sync_channel(1);
        let reader = std::thread::spawn(move || {
            use std::io::BufRead;
            let mut line = String::new();
            let result = std::io::BufReader::new(output)
                .read_line(&mut line)
                .and_then(|_| {
                    if line == "READY\n" {
                        Ok(())
                    } else {
                        Err(std::io::Error::other("inhibitor acquisition failed"))
                    }
                });
            let _ = tx.send(result);
        });
        match rx.recv_timeout(std::time::Duration::from_secs(2)) {
            Ok(Ok(())) => {
                let _ = reader.join();
                Ok(Self(child))
            }
            result => {
                let _ = child.kill();
                let _ = child.wait();
                let _ = reader.join();
                match result {
                    Ok(Err(error)) => Err(error),
                    _ => Err(std::io::Error::new(
                        std::io::ErrorKind::TimedOut,
                        "inhibitor acquisition timed out",
                    )),
                }
            }
        }
    }
}

#[cfg(target_os = "linux")]
impl Drop for Native {
    fn drop(&mut self) {
        drop(self.0.stdin.take());
        if let Err(error) = self.0.kill() {
            tracing::debug!(%error, "inhibitor already exited");
        }
        if let Err(error) = self.0.wait() {
            tracing::warn!(%error, "inhibitor reap failed");
        }
    }
}

#[cfg(not(any(target_os = "linux", target_os = "macos")))]
struct Native;
#[cfg(not(any(target_os = "linux", target_os = "macos")))]
impl Native {
    fn acquire() -> std::io::Result<Self> {
        Ok(Self)
    }
}
