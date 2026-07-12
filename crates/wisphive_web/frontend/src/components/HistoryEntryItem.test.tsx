import { cleanup, fireEvent, render, waitFor, within } from "@testing-library/react";
import { afterEach, describe, expect, it, vi } from "vitest";
import type { HistoryEntry } from "../types/protocol";
import { HistoryEntryItem } from "./HistoryEntryItem";

const entry: HistoryEntry = {
  id: "history-205",
  agent_id: "codex-history-copy",
  agent_type: "codex",
  project: "/tmp/wisphive",
  tool_name: "Bash",
  tool_input: {
    command: "printf 'raw command\\n'",
    description: "Exercise History copy controls",
  },
  decision: "approve",
  requested_at: "2026-07-11T12:00:00Z",
  resolved_at: "2026-07-11T12:00:01Z",
  tool_result: {
    stdout: "raw stdout\\n",
    stderr: "raw stderr\\n",
    raw_output: "raw stdout\\nraw stderr\\n",
  },
};

describe("HistoryEntryItem copy controls", () => {
  afterEach(() => {
    cleanup();
    vi.unstubAllGlobals();
  });

  it("pairs every expanded detail code block with a control that copies its exact text", async () => {
    const writeText = vi.fn().mockResolvedValue(undefined);
    vi.stubGlobal("navigator", {
      ...navigator,
      clipboard: { writeText },
    });

    const { container } = render(
      <HistoryEntryItem
        entry={entry}
        expanded
        onToggle={vi.fn()}
      />,
    );

    const detail = container.querySelector<HTMLElement>(".history-detail");
    expect(detail).not.toBeNull();
    expect(within(detail!).getByRole("heading", { name: "Command" })).toBeInTheDocument();
    expect(within(detail!).getByRole("heading", { name: "Tool Result" })).toBeInTheDocument();

    const codeBlocks = Array.from(detail!.querySelectorAll<HTMLPreElement>("pre"));
    const expectedResult = JSON.stringify(entry.tool_result, null, 2);
    const expectedPayloads = ["printf 'raw command\\n'", expectedResult];
    expect(codeBlocks.map((block) => block.textContent)).toEqual(expectedPayloads);
    expect(expectedResult).toContain('"stdout": "raw stdout\\\\n"');
    expect(expectedResult).toContain('"stderr": "raw stderr\\\\n"');
    expect(expectedResult).toContain('"raw_output": "raw stdout\\\\nraw stderr\\\\n"');

    for (const [index, block] of codeBlocks.entries()) {
      const wrapper = block.closest<HTMLElement>(".code-block-wrap");
      expect(wrapper).not.toBeNull();
      const copyButton = within(wrapper!).getByRole("button", { name: "Copy" });

      fireEvent.click(copyButton);
      await waitFor(() => {
        expect(writeText).toHaveBeenNthCalledWith(index + 1, block.textContent);
      });
    }

    expect(writeText).toHaveBeenCalledTimes(codeBlocks.length);
  });
});
