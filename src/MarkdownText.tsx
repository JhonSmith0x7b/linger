import { memo, useState } from "react";
import type { ReactNode } from "react";
import Markdown from "react-markdown";
import remarkGfm from "remark-gfm";
import { invoke } from "@tauri-apps/api/core";

function safeUrl(url: string): string {
  try {
    const parsed = new URL(url);
    return ["http:", "https:"].includes(parsed.protocol) && !parsed.username && !parsed.password ? parsed.href : "";
  } catch { return ""; }
}

function ExternalLink({ href, children }: { href?: string; children?: ReactNode }) {
  const [error, setError] = useState("");
  if (!href) return <span>{children}</span>;
  return <>
    <a href={href} title={href} onClick={event => {
      event.preventDefault();
      setError("");
      void invoke("open_external_url", { url: href }).catch(() => setError("链接未能打开，请稍后重试"));
    }}>{children}</a>
    {error && <span className="markdown-link-error" role="alert">{error}</span>}
  </>;
}
const plugins = [remarkGfm];
const components = {
  a: ExternalLink,
  // Remote images are neither fetched nor used as navigation targets.
  img: ({ alt }: { alt?: string }) => <span className="markdown-image-alt">{alt || "图片"}</span>,
  table: ({ children }: { children?: ReactNode }) => <div className="markdown-table"><table>{children}</table></div>,
};

export const MarkdownText = memo(function MarkdownText({ text, className = "" }: { text: string; className?: string }) {
  return <div className={`markdown-text ${className}`}><Markdown remarkPlugins={plugins} components={components} urlTransform={safeUrl} skipHtml>{text}</Markdown></div>;
});
