import { render, screen } from "@testing-library/react";
import { describe, expect, it } from "vitest";
import type { ConfigAlert } from "../hooks/useWisphive";
import { ConfigAlertBanner } from "./ConfigAlertBanner";

describe("ConfigAlertBanner", () => {
  it("renders nothing when there are no alerts", () => {
    const { container } = render(<ConfigAlertBanner alerts={[]} />);
    expect(container).toBeEmptyDOMElement();
  });

  it("renders an untrusted-config alert with its message and an alert role", () => {
    const alerts: ConfigAlert[] = [
      {
        kind: "untrusted_config",
        message: "config.json is group writable; using the safe read tier.",
        at: "2026-07-12T00:00:00Z",
      },
    ];
    render(<ConfigAlertBanner alerts={alerts} />);
    expect(screen.getByRole("alert")).toHaveTextContent(/using the safe read tier/);
    expect(screen.getByText("Config untrusted")).toBeInTheDocument();
  });

  it("renders both kinds with distinct severity classes", () => {
    const alerts: ConfigAlert[] = [
      { kind: "policy_widened", message: "level increased", at: "2026-07-12T00:00:00Z" },
      { kind: "untrusted_config", message: "mode loose", at: "2026-07-12T00:00:01Z" },
    ];
    const { container } = render(<ConfigAlertBanner alerts={alerts} />);
    expect(container.querySelector(".config-alert-policy_widened")).toBeTruthy();
    expect(container.querySelector(".config-alert-untrusted_config")).toBeTruthy();
    expect(screen.getByText("level increased")).toBeInTheDocument();
    expect(screen.getByText("mode loose")).toBeInTheDocument();
  });
});
