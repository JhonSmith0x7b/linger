//! Per-turn limits; changing queries cannot reset the allowance.
use serde_json::Value;
use std::collections::HashSet;
#[derive(Default)]
pub struct HistoryBudget {
    searches: usize,
    reads: usize,
    images: HashSet<i64>,
    loaded: HashSet<i64>,
    text_tokens: usize,
}
impl HistoryBudget {
    pub fn search(&mut self) -> Result<(), String> {
        if self.searches >= 2 {
            return Err("本轮历史搜索已达 2 次，请根据已有候选选择或向用户澄清".into());
        }
        self.searches += 1;
        Ok(())
    }
    pub fn read(&mut self) -> Result<(), String> {
        if self.reads >= 2 {
            return Err("本轮历史展开已达 2 次".into());
        }
        self.reads += 1;
        Ok(())
    }
    pub fn image(&mut self, id: i64) -> Result<bool, String> {
        if self.loaded.contains(&id) {
            return Ok(false);
        }
        if !self.images.contains(&id) && self.images.len() >= 2 {
            return Err("本轮历史图片已达 2 张，不再加载新图片".into());
        }
        self.images.insert(id);

        Ok(true)
    }
    pub fn image_loaded(&mut self, id: i64) {
        self.loaded.insert(id);
    }
    pub fn images_removed(&mut self) {
        self.loaded.clear();
    }
    pub fn text(&mut self, value: &Value) -> Result<(), String> {
        let size = tiktoken_rs::o200k_base_singleton()
            .encode_ordinary(&value.to_string())
            .len();
        if self.text_tokens + size > 2000 {
            return Err("本轮历史资料已达文字额度，请使用已有内容或缩小问题".into());
        }
        self.text_tokens += size;
        Ok(())
    }
}
#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn allowances_are_cumulative_and_images_deduplicate() {
        let mut b = HistoryBudget::default();
        assert!(b.search().is_ok());
        assert!(b.search().is_ok());
        assert!(b.search().is_err());
        assert!(b.read().is_ok());
        assert!(b.read().is_ok());
        assert!(b.read().is_err());
        assert_eq!(b.image(1), Ok(true));
        assert_eq!(b.image(1), Ok(true)); // Failed loads are never reported loaded.
        b.image_loaded(1);
        assert_eq!(b.image(1), Ok(false));
        assert_eq!(b.image(2), Ok(true));
        assert!(b.image(3).is_err());
        b.images_removed();
        assert_eq!(b.image(1), Ok(true));
        assert!(b.image(3).is_err());
        assert!(b
            .text(&serde_json::json!({"content":"a ".repeat(1100)}))
            .is_ok());
        assert!(b
            .text(&serde_json::json!({"content":"a ".repeat(1100)}))
            .is_err());
    }
}
