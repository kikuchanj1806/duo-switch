//! A desktop app, running: whether it runs, asking it to quit, and opening
//! it again. Claude Desktop before its profile changes, Codex Desktop before
//! its login does. An app is never killed: one that will not quit stops the
//! change instead.

use std::process::{Command, Stdio};
use std::time::{Duration, Instant};

use super::{refuse, Code};
use crate::error::Result;

pub trait DesktopApp {
    fn running(&self) -> Result<bool>;
    /// Ask the app to quit, and wait until it has.
    fn quit(&self) -> Result<()>;
    fn open(&self) -> Result<()>;
}

impl<T: DesktopApp + ?Sized> DesktopApp for Box<T> {
    fn running(&self) -> Result<bool> {
        (**self).running()
    }
    fn quit(&self) -> Result<()> {
        (**self).quit()
    }
    fn open(&self) -> Result<()> {
        (**self).open()
    }
}

/// A real app, found by its bundle id through AppleScript and `open`.
#[derive(Debug, Clone, Copy)]
pub struct MacApp {
    /// What people call it, for messages.
    pub name: &'static str,
    pub bundle_id: &'static str,
}

impl MacApp {
    pub const CLAUDE: MacApp = MacApp {
        name: "Claude",
        bundle_id: "com.anthropic.claudefordesktop",
    };
    /// The Codex app; it may be installed as ChatGPT.app.
    pub const CODEX: MacApp = MacApp {
        name: "Codex",
        bundle_id: "com.openai.codex",
    };
}

const QUIT_ASK: Duration = Duration::from_secs(35);
const QUIT_WAIT: Duration = Duration::from_secs(30);

impl DesktopApp for MacApp {
    /// Asking whether an app runs sends it nothing, so it needs no
    /// permission; an app that is not installed does not run.
    fn running(&self) -> Result<bool> {
        let out = Command::new("/usr/bin/osascript")
            .args([
                "-e",
                &format!(r#"application id "{}" is running"#, self.bundle_id),
            ])
            .stdin(Stdio::null())
            .stderr(Stdio::null())
            .output()
            .map_err(|e| refuse(Code::App, format!("could not look for {}: {e}", self.name)))?;
        Ok(out.status.success() && String::from_utf8_lossy(&out.stdout).trim() == "true")
    }

    fn quit(&self) -> Result<()> {
        if !self.running()? {
            return Ok(());
        }
        let mut ask = Command::new("/usr/bin/osascript")
            .args([
                "-e",
                &format!(r#"tell application id "{}" to quit"#, self.bundle_id),
            ])
            .stdout(Stdio::null())
            .stderr(Stdio::piped())
            .spawn()
            .map_err(|e| {
                refuse(
                    Code::App,
                    format!("could not ask {} to quit: {e}", self.name),
                )
            })?;
        let asked = Instant::now();
        // Only the AppleScript is ever stopped, never the app.
        let status = loop {
            if let Some(status) = ask.try_wait().ok().flatten() {
                break Some(status);
            }
            if asked.elapsed() > QUIT_ASK {
                let _ = ask.kill();
                let _ = ask.wait();
                break None;
            }
            std::thread::sleep(Duration::from_millis(200));
        };
        if status.is_some_and(|s| !s.success()) {
            let mut said = String::new();
            if let Some(mut err) = ask.stderr.take() {
                let _ = std::io::Read::read_to_string(&mut err, &mut said);
            }
            if refused_automation(&said) {
                return Err(refuse(
                    Code::AutomationDenied,
                    format!(
                        "macOS does not let Duo Switch ask {0} to quit: allow it in System Settings > Privacy & Security > Automation > Duo Switch > {0}, then try again.",
                        self.name
                    ),
                ));
            }
        }
        let deadline = Instant::now() + QUIT_WAIT;
        while self.running()? {
            if Instant::now() >= deadline {
                return Err(refuse(
                    Code::AppWontQuit,
                    format!(
                        "{} has not quit. Finish what it is doing, quit it with Cmd+Q, then try again.",
                        self.name
                    ),
                ));
            }
            std::thread::sleep(Duration::from_millis(500));
        }
        Ok(())
    }

    fn open(&self) -> Result<()> {
        let status = Command::new("/usr/bin/open")
            .args(["-b", self.bundle_id])
            .stdout(Stdio::null())
            .stderr(Stdio::null())
            .status()
            .map_err(|e| refuse(Code::App, format!("could not open {}: {e}", self.name)))?;
        if status.success() {
            Ok(())
        } else {
            Err(refuse(Code::App, format!("could not open {}", self.name)))
        }
    }
}

/// Whether `osascript` was refused the Apple Event: the user said no to
/// "Duo Switch wants to control Claude", or was never asked.
fn refused_automation(stderr: &str) -> bool {
    stderr.contains("-1743") || stderr.contains("Not authorized to send Apple events")
}

/// No app at all: a sandbox (`PITSTOP_HOME`) must not quit or open the real
/// apps.
#[derive(Debug, Default, Clone, Copy)]
pub struct NoApp;

impl DesktopApp for NoApp {
    fn running(&self) -> Result<bool> {
        Ok(false)
    }
    fn quit(&self) -> Result<()> {
        Ok(())
    }
    fn open(&self) -> Result<()> {
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    #[test]
    fn a_refused_apple_event_is_told_apart() {
        assert!(super::refused_automation(
            "execution error: Not authorized to send Apple events to Claude. (-1743)"
        ));
        assert!(!super::refused_automation(
            "execution error: Claude got an error (-128)"
        ));
    }
}

#[cfg(test)]
pub(crate) mod fake {
    use std::cell::RefCell;

    use super::*;

    /// An app that records what it was asked, for tests.
    #[derive(Default)]
    pub struct FakeApp {
        pub running: RefCell<bool>,
        pub calls: RefCell<Vec<&'static str>>,
        pub refuses_to_quit: bool,
    }

    impl DesktopApp for FakeApp {
        fn running(&self) -> Result<bool> {
            Ok(*self.running.borrow())
        }
        fn quit(&self) -> Result<()> {
            self.calls.borrow_mut().push("quit");
            if self.refuses_to_quit && *self.running.borrow() {
                return Err(refuse(Code::AppWontQuit, "Claude has not quit"));
            }
            *self.running.borrow_mut() = false;
            Ok(())
        }
        fn open(&self) -> Result<()> {
            self.calls.borrow_mut().push("open");
            *self.running.borrow_mut() = true;
            Ok(())
        }
    }
}
