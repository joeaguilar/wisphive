import type { DiskAlert } from "../hooks/useWisphive";

const KIND_LABEL: Record<DiskAlert["kind"], string> = {
  archive_size: "Audit archive large",
  low_disk_space: "Low disk space",
};

/**
 * Non-dismissable banner for daemon resource alerts. Wisphive never deletes
 * audit data; instead the daemon raises these so the operator can act (move or
 * compress the archive, free disk). The banner clears on its own when the
 * daemon sends a `disk_alert` with `active:false` (itr#340).
 */
export function DiskAlertBanner({ alerts }: { alerts: DiskAlert[] }) {
  if (alerts.length === 0) return null;
  return (
    <div className="disk-alert-banner" role="alert" aria-live="polite">
      {alerts.map((a) => (
        <div key={a.kind} className={`disk-alert disk-alert-${a.kind}`}>
          <span className="disk-alert-icon" aria-hidden="true">
            ⚠
          </span>
          <span className="disk-alert-label">{KIND_LABEL[a.kind]}</span>
          <span className="disk-alert-message">{a.message}</span>
        </div>
      ))}
    </div>
  );
}
