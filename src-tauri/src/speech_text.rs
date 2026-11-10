//! Spoken prose is a derived view; the original Markdown stays in chat/history.
use pulldown_cmark::{Event, Options, Parser, Tag, TagEnd};
use regex::Regex;
use std::sync::LazyLock;

static URL: LazyLock<Regex> =
    LazyLock::new(|| Regex::new(r"(?i)(?:https?://|www\.)[^\s<>（），。！？；、\]\)]+").unwrap());
static DOMAIN: LazyLock<Regex> = LazyLock::new(|| {
    Regex::new(r"(?i)^(?:https?://|www\.|[a-z0-9.-]+\.[a-z]{2,}(?:/|$))").unwrap()
});
static CITATION: LazyLock<Regex> =
    LazyLock::new(|| Regex::new(r"[^]*|\[\d+(?:[,，]\s*\d+)*\]").unwrap());
static EMPTY_PARENS: LazyLock<Regex> = LazyLock::new(|| Regex::new(r"[（(]\s*[）)]").unwrap());

pub fn for_speech(markdown: &str) -> String {
    let body = markdown
        .lines()
        .take_while(|line| {
            let heading = line.trim().trim_start_matches('#').trim();
            !matches!(
                heading,
                "来源："
                    | "来源:"
                    | "来源"
                    | "参考来源："
                    | "参考来源:"
                    | "Sources:"
                    | "Sources"
                    | "References:"
            )
        })
        .collect::<Vec<_>>()
        .join("\n");
    let clean = CITATION.replace_all(&body, "");
    let mut plain = String::new();
    let mut skipped = 0usize;
    let mut in_link = false;
    for event in Parser::new_ext(
        &clean,
        Options::ENABLE_STRIKETHROUGH | Options::ENABLE_TABLES | Options::ENABLE_FOOTNOTES,
    ) {
        if skipped > 0 {
            match event {
                Event::Start(_) => skipped += 1,
                Event::End(_) => skipped -= 1,
                _ => {}
            }
            continue;
        }
        match event {
            Event::Start(Tag::CodeBlock(_) | Tag::Image { .. } | Tag::FootnoteDefinition(_)) => {
                skipped = 1;
                plain.push('\n');
            }
            Event::Start(Tag::Link { .. }) => in_link = true,
            Event::End(TagEnd::Link) => in_link = false,
            Event::Text(text) | Event::Code(text) => {
                if !in_link || !DOMAIN.is_match(text.trim()) {
                    plain.push_str(&text);
                }
            }
            Event::SoftBreak | Event::HardBreak | Event::Rule => plain.push('\n'),
            Event::End(
                TagEnd::Paragraph | TagEnd::Heading(_) | TagEnd::Item | TagEnd::TableRow,
            ) => plain.push('\n'),
            Event::End(TagEnd::TableCell) => plain.push(' '),
            _ => {}
        }
    }
    let plain = URL.replace_all(&plain, "");
    let plain = EMPTY_PARENS.replace_all(&plain, "");
    plain
        .lines()
        .map(str::trim)
        .filter(|line| {
            !line.is_empty()
                && !matches!(
                    *line,
                    "来源："
                        | "来源:"
                        | "参考来源："
                        | "参考来源:"
                        | "Sources:"
                        | "Sources"
                        | "References:"
                )
        })
        .collect::<Vec<_>>()
        .join("\n")
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn search_citations_and_urls_are_not_spoken() {
        let input = "你是说**魔兽世界**啊？([zh.wikipedia.org](https://zh.wikipedia.org/wiki/test?utm_source=openai))\n\n来源：\nhttps://zh.wikipedia.org/wiki/test?utm_source=openai";
        assert_eq!(for_speech(input), "你是说魔兽世界啊？");
        assert_eq!(
            for_speech("正文。\n\n## 来源\n[百科](https://example.org)"),
            "正文。"
        );
    }
    #[test]
    fn meaningful_link_labels_lists_and_inline_code_survive() {
        assert_eq!(
            for_speech(
                "## 建议\n- 打开[设置页面](https://example.org/settings)。\n- 输入 `hello`。"
            ),
            "建议\n打开设置页面。\n输入 hello。"
        );
    }
    #[test]
    fn code_images_footnotes_and_native_citations_are_omitted() {
        assert_eq!(for_speech("可以这样做。[1]citeturn0search0\n\n```js\nalert('x')\n```\n![图](https://example.org/a.png)\n\n结束[^a]。\n\n[^a]: 不朗读脚注"), "可以这样做。\n结束。");
        assert_eq!(for_speech("[https://example.org](https://example.org)"), "");
    }
}
