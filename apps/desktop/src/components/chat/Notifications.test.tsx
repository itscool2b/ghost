import { renderToStaticMarkup } from "react-dom/server";
import { describe, expect, it } from "vitest";
import { bellAttention, bellCount, type NoticeItem, Notifications } from "./Notifications";

const item = (key: string, level?: NoticeItem["level"]): NoticeItem => ({
  key,
  ...(level ? { level } : {}),
  node: <li key={key}>{key}</li>,
});

describe("the notifications bell", () => {
  it("says what needs a look: a failure, then a warning, then anything not yet seen", () => {
    const seen = new Set(["a", "b"]);
    expect(bellAttention([], seen)).toBeNull();
    expect(bellAttention([item("a"), item("b")], seen)).toBeNull();
    expect(bellAttention([item("a"), item("c")], seen)).toBe("new");
    expect(bellAttention([item("a", "warning"), item("c")], seen)).toBe("warning");
    expect(bellAttention([item("a", "warning"), item("b", "error")], seen)).toBe("error");
  });

  it("names how many wait and how many are new, and lists them in its panel", () => {
    const html = renderToStaticMarkup(<Notifications items={[item("one"), item("two")]} />);
    expect(html).toContain('aria-label="Notifications, 2, 2 new"');
    expect(html).toContain('data-attention="new"');
    expect(html).toContain('<h2 class="chat-inbox-title">Notifications</h2>');
    expect(html).toContain("<li>one</li><li>two</li>");
  });

  it("says when one needs attention, and is quiet with nothing to show", () => {
    expect(renderToStaticMarkup(<Notifications items={[item("x", "error")]} />)).toContain(
      'aria-label="Notifications, 1, one needs attention"',
    );
    const empty = renderToStaticMarkup(<Notifications items={[]} />);
    expect(empty).toContain('aria-label="Notifications, none"');
    expect(empty).not.toContain("data-attention");
    expect(empty).toContain("No notifications.");
  });

  it("counts what is new, or what needs attention while one does, on the bell", () => {
    const seen = new Set(["a"]);
    const news = [item("a"), item("b"), item("c")];
    expect(bellCount(news, seen, bellAttention(news, seen))).toBe("2");
    const failing = [item("a", "error"), item("b"), item("c", "error")];
    expect(bellCount(failing, seen, bellAttention(failing, seen))).toBe("2");
    expect(bellCount([item("a")], seen, null)).toBeNull();
    const many = Array.from({ length: 12 }, (_, index) => item(String(index)));
    expect(bellCount(many, new Set(), "new")).toBe("9+");
    const html = renderToStaticMarkup(<Notifications items={news} />);
    expect(html).toContain('<span class="chat-inbox-count" aria-hidden="true">3</span>');
    expect(renderToStaticMarkup(<Notifications items={[]} />)).not.toContain("chat-inbox-count");
  });
});
