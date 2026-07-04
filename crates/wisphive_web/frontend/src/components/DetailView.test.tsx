import { render, screen, fireEvent, cleanup } from "@testing-library/react";
import { afterEach, describe, expect, it, vi } from "vitest";
import { DetailView } from "./DetailView";
import type { DecisionRequest } from "../types/protocol";

// globals: false in vitest.config → testing-library's auto-cleanup doesn't run.
afterEach(cleanup);

const base: DecisionRequest = {
  id: "req-1",
  agent_id: "cc-1",
  agent_type: "claude_code",
  project: "/proj",
  tool_name: "Bash",
  tool_input: {},
  timestamp: "2026-07-03T00:00:00Z",
  hook_event_name: "PreToolUse",
};

const make = (over: Partial<DecisionRequest>): DecisionRequest => ({ ...base, ...over });

describe("DetailView — AskUserQuestion answer payload (itr#250)", () => {
  const askReq = make({
    tool_name: "AskUserQuestion",
    hook_event_name: "PermissionRequest",
    tool_input: {
      questions: [
        { header: "Deploy?", question: "Ship to prod?", options: [{ label: "Yes" }, { label: "No" }] },
      ],
    },
  });

  it("passes the selected option back as updated_input.answers, not a bare approve", () => {
    const onApprove = vi.fn();
    render(<DetailView request={askReq} onApprove={onApprove} onDeny={vi.fn()} />);

    fireEvent.click(screen.getByText("Yes"));

    expect(onApprove).toHaveBeenCalledTimes(1);
    const [id, opts] = onApprove.mock.calls[0];
    expect(id).toBe("req-1");
    expect(opts.updated_input).toEqual({
      questions: askReq.tool_input!.questions,
      answers: { "Ship to prod?": "Yes" },
    });
  });

  it("offers no bare Approve button that would drop the answer", () => {
    render(<DetailView request={askReq} onApprove={vi.fn()} onDeny={vi.fn()} />);
    // The option buttons ARE the approve path; only Deny variants in the action row.
    expect(screen.queryByRole("button", { name: /^Approve$/ })).toBeNull();
    expect(screen.getByRole("button", { name: /^Deny$/ })).toBeInTheDocument();
  });
});

describe("DetailView — ExitPlanMode (itr#249 limitation note, itr#253 error state)", () => {
  it("renders the plan and the hook-layer limitation note", () => {
    const req = make({
      tool_name: "ExitPlanMode",
      hook_event_name: "PermissionRequest",
      event_data: { plan_content: "# Plan\ndo the thing" },
    });
    render(<DetailView request={req} onApprove={vi.fn()} onDeny={vi.fn()} />);
    expect(screen.getByText(/do the thing/)).toBeInTheDocument();
    expect(screen.getByText(/only supports/i)).toBeInTheDocument();
  });

  it("renders an explicit failure when plan extraction errored", () => {
    const req = make({
      tool_name: "ExitPlanMode",
      hook_event_name: "PermissionRequest",
      event_data: { plan_content: { error: "transcript unreadable (/x): nope", path: "/x" } },
    });
    render(<DetailView request={req} onApprove={vi.fn()} onDeny={vi.fn()} />);
    expect(screen.getByText("Plan unavailable")).toBeInTheDocument();
    expect(screen.getByText(/transcript unreadable/)).toBeInTheDocument();
  });
});
