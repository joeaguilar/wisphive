import { render, screen } from "@testing-library/react";
import { describe, expect, it } from "vitest";
import { DiskAlertBanner } from "./DiskAlertBanner";
import type { DiskAlert } from "../hooks/useWisphive";

describe("DiskAlertBanner", () => {
  it("renders nothing when there are no alerts", () => {
    const { container } = render(<DiskAlertBanner alerts={[]} />);
    expect(container).toBeEmptyDOMElement();
  });

  it("renders an archive-size alert with its message and an alert role", () => {
    const alerts: DiskAlert[] = [
      { kind: "archive_size", message: "Audit archive is 11.0 GiB (over the 10.0 GiB alert threshold).", at: "2026-05-29T00:00:00Z" },
    ];
    render(<DiskAlertBanner alerts={alerts} />);
    expect(screen.getByRole("alert")).toHaveTextContent(/Audit archive is 11\.0 GiB/);
    expect(screen.getByText("Audit archive large")).toBeInTheDocument();
  });

  it("renders both kinds with distinct severity classes", () => {
    const alerts: DiskAlert[] = [
      { kind: "archive_size", message: "archive big", at: "2026-05-29T00:00:00Z" },
      { kind: "low_disk_space", message: "disk low", at: "2026-05-29T00:00:00Z" },
    ];
    const { container } = render(<DiskAlertBanner alerts={alerts} />);
    expect(container.querySelector(".disk-alert-archive_size")).toBeTruthy();
    expect(container.querySelector(".disk-alert-low_disk_space")).toBeTruthy();
    expect(screen.getByText("disk low")).toBeInTheDocument();
    expect(screen.getByText("archive big")).toBeInTheDocument();
  });
});
