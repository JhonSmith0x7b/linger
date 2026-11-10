#[derive(Default)]
pub struct ShortcutCapture {
    next: u64,
    active: Option<u64>,
}

impl ShortcutCapture {
    pub fn begin(&mut self) -> Result<u64, String> {
        if self.active.is_some() {
            return Err("快捷键正在录入，请先完成或取消".into());
        }
        self.next += 1;
        self.active = Some(self.next);
        Ok(self.next)
    }

    pub fn active(&self) -> Option<u64> {
        self.active
    }

    pub fn finish(&mut self, token: u64) -> bool {
        if self.active != Some(token) {
            return false;
        }
        self.active = None;
        true
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn stale_timeout_cannot_end_new_capture() {
        let mut capture = ShortcutCapture::default();
        let first = capture.begin().unwrap();
        assert!(capture.begin().is_err());
        assert!(capture.finish(first));
        let second = capture.begin().unwrap();
        assert!(!capture.finish(first));
        assert_eq!(capture.active(), Some(second));
        assert!(capture.finish(second));
        assert!(!capture.finish(second));
    }
    #[test]
    fn captured_keyboard_codes_parse_and_aliases_share_identity() {
        use tauri_plugin_global_shortcut::Shortcut;
        for chord in [
            "Control+Shift+KeyA",
            "Super+Alt+Space",
            "F8",
            "Control+Digit1",
            "Alt+ArrowUp",
            "Control+BracketLeft",
        ] {
            assert!(chord.parse::<Shortcut>().is_ok(), "{chord}");
        }
        assert!("Control+Shift".parse::<Shortcut>().is_err());
        assert_eq!(
            "Ctrl+Shift+A".parse::<Shortcut>().unwrap().id(),
            "Control+Shift+KeyA".parse::<Shortcut>().unwrap().id()
        );
    }
}
