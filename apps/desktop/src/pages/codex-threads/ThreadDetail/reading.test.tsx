import { describe, expect, it } from "vitest";
import { renderToStaticMarkup } from "react-dom/server";
import type { CodexThreadTimelineItem } from "../../../types";
import { highlightedText, matchingItems, nextMatch, ReadableMessage, safeReadingUrl } from "./reading";

const item = (id: string, overrides: Partial<CodexThreadTimelineItem> = {}): CodexThreadTimelineItem => ({
  id, kind: "message:assistant", text: "A saved reply", toolName: null, toolInput: null, toolOutput: null,
  toolStatus: null, timestamp: null, truncated: false, ...overrides,
});

describe("bounded session reading", () => {
  it("renders headings, emphasis, lists, quotes and fenced code without treating code as Markdown", () => {
    const html = renderToStaticMarkup(<ReadableMessage value={'## Result\n\n**Done** and `npm test`\n\n- first\n- second\n\n3. third\n\n> quoted\n\n```ts\nconst x = "**literal**";\n```'} />);
    for (const expected of ["<h3>Result</h3>", "<strong>Done</strong>", "<code>npm test</code>", "<ul><li>first", '<ol start="3">', "<blockquote>quoted", 'data-language="ts"', "**literal**"]) expect(html).toContain(expected);
    expect(html).not.toContain("<strong>literal</strong>");
  });

  it("keeps injected HTML inert, rejects executable/private links and never fetches images", () => {
    const html = renderToStaticMarkup(<ReadableMessage value={'<img src=x onerror=alert(1)>\n\n[bad](javascript:alert) [local](file:///etc/passwd) [ok](https://example.com/page)\n\n![pixel](https://example.com/track.png)'} />);
    expect(html).toContain("&lt;img");
    expect(html).not.toContain("<img");
    expect(html).not.toContain('href="javascript:');
    expect(html).not.toContain('href="file:');
    expect(html).toContain('href="https://example.com/page"');
    expect(html).toContain('rel="noopener noreferrer"');
    expect(safeReadingUrl("https://user:password@example.com/")).toBeNull();
  });

  it("preserves the content of an unterminated code fence", () => {
    const html = renderToStaticMarkup(<ReadableMessage value={'```sh\necho one\necho two'} query="two" />);
    expect(html).toContain("echo one\necho <mark>two</mark>");
  });

  it("renders tables while preserving pipes inside code and escaped cells", () => {
    const html = renderToStaticMarkup(<ReadableMessage value={'| Field | Value |\n| --- | --- |\n| command | `a|b` |\n| label | one\\|two |'} />);
    expect(html).toContain("<table>");
    expect(html).toContain("<td><code>a|b</code></td>");
    expect(html).toContain("<td>one|two</td>");
  });

  it("finds literal case-insensitive queries in loaded messages and tool data without counting occurrences as records", () => {
    const items = [item("a", { text: "Hello HELLO" }), item("b", { kind: "tool_call", text: null, toolOutput: "hello [a-z]" }), item("c")];
    expect(matchingItems(items, " hello ")).toEqual(["a", "b"]);
    expect(matchingItems(items, "[a-z]")).toEqual(["b"]);
    expect(matchingItems(items, "")).toEqual([]);
    expect(renderToStaticMarkup(<>{highlightedText("[a-z] is literal", "[a-z]")}</>)).toBe("<mark>[a-z]</mark> is literal");
    expect(renderToStaticMarkup(<>{highlightedText("İstanbul: quota", "QUOTA")}</>)).toBe("İstanbul: <mark>quota</mark>");
    expect(nextMatch(["a", "b"], "b", 1)).toBe("a");
    expect(nextMatch(["a", "b"], "a", -1)).toBe("b");
    expect(nextMatch(["a", "b"], "unloaded", 1)).toBe("a");
    expect(nextMatch([], null, 1)).toBeNull();
  });
});
