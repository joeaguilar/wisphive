import { describe, expect, it } from "vitest";
import { render, screen } from "@testing-library/react";

import { MarkdownText } from "./MarkdownText";

describe("MarkdownText", () => {
  it("renders hostile HTML-looking markdown as inert text", () => {
    const payload = [
      "# Plan",
      "<script>window.__markdownTextXss = true</script>",
      '<img src=x onerror="window.__markdownTextXss = true">',
      "## </h2><script>window.__markdownTextXss = true</script>",
    ].join("\n");

    const { container } = render(<MarkdownText text={payload} />);

    expect(container.querySelector("script")).toBeNull();
    expect(container.querySelector("img")).toBeNull();
    expect(container.querySelector(".markdown-content")).toHaveTextContent(
      "<script>window.__markdownTextXss = true</script>",
    );
    expect(container.querySelector(".markdown-content")).toHaveTextContent(
      '<img src=x onerror="window.__markdownTextXss = true">',
    );
    expect(container.querySelector(".markdown-content")).toHaveTextContent(
      "</h2><script>window.__markdownTextXss = true</script>",
    );
  });

  it("leaves javascript protocol markdown links as non-clickable text", () => {
    const { container } = render(<MarkdownText text={"# Links\n[x](javascript:alert(1))"} />);

    expect(screen.queryByRole("link", { name: "x" })).not.toBeInTheDocument();
    expect(container.querySelector("a[href^='javascript:']")).toBeNull();
    expect(container.querySelector(".markdown-content")).toHaveTextContent("[x](javascript:alert(1))");
  });

  it("still renders allowed markdown links", () => {
    render(<MarkdownText text={"[docs](https://example.test/path)"} />);

    expect(screen.getByRole("link", { name: "docs" })).toHaveAttribute(
      "href",
      "https://example.test/path",
    );
  });
});
