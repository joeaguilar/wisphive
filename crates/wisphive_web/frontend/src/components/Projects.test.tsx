import { cleanup, fireEvent, render, screen } from "@testing-library/react";
import { afterEach, describe, expect, it, vi } from "vitest";
import type { ComponentProps } from "react";
import { Projects } from "./Projects";
import type { ProjectHookStatus, ProjectSummary } from "../types/protocol";

function summary(project: string, overrides: Partial<ProjectSummary> = {}): ProjectSummary {
  return {
    project,
    first_seen: "2026-07-04T11:00:00Z",
    last_seen: "2026-07-04T12:00:00Z",
    total_calls: 3,
    approved: 2,
    denied: 1,
    agent_count: 0,
    ...overrides,
  };
}

function hookStatus(project: string, overrides: Partial<ProjectHookStatus>): ProjectHookStatus {
  return {
    project,
    mode: "active",
    claude_installed: false,
    codex_installed: false,
    missing_events: [],
    all_installed: false,
    all_enabled: false,
    ...overrides,
  };
}

function renderProjects(props: Partial<ComponentProps<typeof Projects>> = {}) {
  return render(
    <Projects
      projects={[]}
      hookStatus={{}}
      hookErrors={{}}
      onLoad={vi.fn()}
      onSpawnInProject={vi.fn()}
      onDrillDown={vi.fn()}
      onInstallHooks={vi.fn()}
      onQueryHookStatus={vi.fn()}
      {...props}
    />,
  );
}

describe("Projects hook-gating (itr#460)", () => {
  afterEach(() => cleanup());

  it("renders the three gate states from a hookStatus fixture", () => {
    const gated = "/repo/gated";
    const repair = "/repo/repair";
    const notGated = "/repo/ungated";
    renderProjects({
      projects: [summary(gated), summary(repair), summary(notGated)],
      hookStatus: {
        // Gated: everything installed + mode active.
        [gated]: hookStatus(gated, {
          claude_installed: true,
          codex_installed: true,
          all_installed: true,
          all_enabled: true,
        }),
        // Needs repair: partially installed (claude only), events missing.
        [repair]: hookStatus(repair, {
          claude_installed: true,
          missing_events: ["Stop"],
        }),
        // Not gated: nothing installed.
        [notGated]: hookStatus(notGated, { mode: "missing" }),
      },
    });

    expect(screen.getByText("Gated")).toBeInTheDocument();
    expect(screen.getByText("Needs repair")).toBeInTheDocument();
    expect(screen.getByText("Not gated")).toBeInTheDocument();
  });

  it("sends install_hooks from the path input only AFTER the confirm modal is accepted", () => {
    const onInstallHooks = vi.fn();
    renderProjects({ onInstallHooks });

    const path = "/Users/j/controller";
    fireEvent.change(screen.getByPlaceholderText("/absolute/path/to/project"), {
      target: { value: path },
    });
    fireEvent.click(screen.getByRole("button", { name: "Gate project" }));

    // Confirm modal is up but nothing written yet.
    expect(onInstallHooks).not.toHaveBeenCalled();
    expect(screen.getByText("Gate this project?")).toBeInTheDocument();
    // The modal states plainly what will be written.
    expect(screen.getByText(new RegExp(`${path}/\\.claude/settings\\.json`))).toBeInTheDocument();

    // Only on confirm does the install go out.
    fireEvent.click(screen.getByRole("button", { name: "Install hooks" }));
    expect(onInstallHooks).toHaveBeenCalledTimes(1);
    expect(onInstallHooks).toHaveBeenCalledWith(path);
  });

  it("cancelling the confirm modal does not install", () => {
    const onInstallHooks = vi.fn();
    renderProjects({ onInstallHooks });
    fireEvent.change(screen.getByPlaceholderText("/absolute/path/to/project"), {
      target: { value: "/x/y" },
    });
    fireEvent.click(screen.getByRole("button", { name: "Gate project" }));
    fireEvent.click(screen.getByRole("button", { name: "Cancel" }));
    expect(onInstallHooks).not.toHaveBeenCalled();
    expect(screen.queryByText("Gate this project?")).not.toBeInTheDocument();
  });

  it("per-card Gate/Repair button routes through the same confirm flow", () => {
    const onInstallHooks = vi.fn();
    const project = "/repo/ungated";
    renderProjects({
      projects: [summary(project)],
      hookStatus: { [project]: hookStatus(project, { mode: "missing" }) },
      onInstallHooks,
    });

    fireEvent.click(screen.getByRole("button", { name: "Gate" }));
    expect(onInstallHooks).not.toHaveBeenCalled();
    fireEvent.click(screen.getByRole("button", { name: "Install hooks" }));
    expect(onInstallHooks).toHaveBeenCalledWith(project);
  });

  it("surfaces an install error as inert text", () => {
    const project = "/repo/ungated";
    renderProjects({
      projects: [summary(project)],
      hookStatus: { [project]: hookStatus(project, { mode: "missing" }) },
      hookErrors: { [project]: "settings.json is not writable" },
    });
    expect(screen.getByText("settings.json is not writable")).toBeInTheDocument();
  });
});
