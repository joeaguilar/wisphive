import type { ConfigAlert } from "../hooks/useWisphive";

const KIND_LABEL: Record<ConfigAlert["kind"], string> = {
  policy_widened: "Approval policy widened",
  untrusted_config: "Config untrusted",
};

/**
 * Non-dismissable evidence banner for config trust loss and approval-policy
 * widening. The daemon clears each kind once the corresponding state recovers.
 */
export function ConfigAlertBanner({ alerts }: { alerts: ConfigAlert[] }) {
  if (alerts.length === 0) return null;
  const live = alerts.some((alert) => alert.kind === "untrusted_config")
    ? "assertive"
    : "polite";
  return (
    <div className="config-alert-banner" role="alert" aria-live={live}>
      {alerts.map((alert) => (
        <div key={alert.kind} className={`config-alert config-alert-${alert.kind}`}>
          <span className="config-alert-icon" aria-hidden="true">
            ⚠
          </span>
          <span className="config-alert-label">{KIND_LABEL[alert.kind]}</span>
          <span className="config-alert-message">{alert.message}</span>
        </div>
      ))}
    </div>
  );
}
