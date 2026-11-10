//! Per-process cache: opening another window or starting a turn must not reread Keychain.
use std::{collections::HashMap, sync::Mutex};

#[derive(Default)]
pub struct Credentials(Mutex<HashMap<String, Result<String, String>>>);
impl Credentials {
    pub fn get(
        &self,
        base: &str,
        retry: bool,
        load: impl FnOnce() -> Result<String, String>,
    ) -> Result<String, String> {
        let mut entries = self.0.lock().unwrap();
        if let Some(value) = entries.get(base) {
            if value.is_ok() || !retry {
                return value.clone();
            }
        }
        let value = load();
        entries.insert(base.to_owned(), value.clone());
        value
    }
    pub fn write(
        &self,
        base: &str,
        key: Option<&str>,
        save: impl FnOnce() -> Result<(), String>,
    ) -> Result<(), String> {
        let mut entries = self.0.lock().unwrap();
        save()?;
        entries.insert(
            base.to_owned(),
            key.map(str::to_owned)
                .ok_or_else(|| "请在设置中保存当前地址的 API Key".into()),
        );
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn reads_once_per_address_and_does_not_repeat_denied_prompts() {
        let cache = Credentials::default();
        assert_eq!(
            cache.get("a", false, || Ok("key-a".into())).unwrap(),
            "key-a"
        );
        assert_eq!(cache.get("a", true, || panic!("cached")).unwrap(), "key-a");
        assert!(cache.get("b", false, || Err("denied".into())).is_err());
        assert!(cache
            .get("b", false, || panic!("no repeated prompt"))
            .is_err());
        assert_eq!(
            cache.get("b", true, || Ok("key-b".into())).unwrap(),
            "key-b"
        );
    }
    #[test]
    fn successful_writes_refresh_cache_failed_writes_preserve_it() {
        let cache = Credentials::default();
        cache.write("a", Some("old"), || Ok(())).unwrap();
        assert!(cache
            .write("a", Some("new"), || Err("failed".into()))
            .is_err());
        assert_eq!(cache.get("a", false, || panic!()).unwrap(), "old");
        cache.write("a", Some("new"), || Ok(())).unwrap();
        assert_eq!(cache.get("a", false, || panic!()).unwrap(), "new");
        cache.write("a", None, || Ok(())).unwrap();
        assert!(cache.get("a", false, || panic!()).is_err());
    }
}
