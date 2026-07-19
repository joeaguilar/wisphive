import type { UiError } from "../hooks/useWisphive";

const SOURCE_LABEL: Record<UiError["source"], string> = {
  daemon: "Daemon error",
  terminal: "Terminal error",
  client: "Not delivered",
};

/**
 * Dismissible banner for surfaced errors (itr#567): daemon `error` /
 * `command_error` frames, terminal `term_error` frames, and client-side
 * send-while-disconnected failures. Follows the DiskAlertBanner /
 * ConfigAlertBanner pattern, but rows are operator-dismissible because these
 * are events, not latched conditions the daemon will clear.
 *
 * `message` is server/agent-derived untrusted display data — rendered as an
 * inert React text node only.
 *
 * ── Spawn-refusal path audit (itr#567 AC4) ──────────────────────────────
 *
 * Every known way a web-originated `spawn_agent` can fail or be refused, and
 * where the operator now sees it. "Modal" = the SpawnModal's correlated
 * status (useWisphive `state.spawn`); "Banner" = this component fed by
 * `state.errors`. Correlated refusals reach the modal while it is open and
 * fall through to the banner after it closes (`clearSpawnStatus`).
 *
 * | #  | Path                                                        | Where it originates                                   | Surfaced as                                            |
 * |----|-------------------------------------------------------------|-------------------------------------------------------|--------------------------------------------------------|
 * | 1  | Sudo reauth gate on approve                                 | sudo_gate → WebReauthRequired                         | SudoModal (pre-existing)                               |
 * | 2  | WS not open — send() no-op                                  | useWisphive send()                                    | Modal ("not connected" refusal); banner for other cmds |
 * | 3  | Bridge drops malformed browser frame                        | ws_bridge.rs rewrap failure                           | Wire `error` frame → Banner (was warn!-only)           |
 * | 4  | Mode not active preflight                                   | server.rs ensure_spawn_mode_active                    | Correlated `command_error` → Modal                     |
 * | 5  | Invalid spawn request                                       | server.rs validate_spawn_request                      | Correlated `command_error` → Modal                     |
 * | 6  | Pending-id collision (managed spawn)                        | server.rs enqueue_spawn_for_approval bail             | Correlated `command_error` → Modal                     |
 * | 7  | MAX_PENDING_SPAWNS limit                                    | server.rs enqueue_spawn_for_approval bail             | Correlated `command_error` → Modal                     |
 * | 8  | Pending persist/DB failure while queueing                   | server.rs enqueue_spawn_for_approval Err              | Correlated `command_error` → Modal                     |
 * | 9  | Duplicate decision id (hook-origin, queue.rs enqueue None)  | queue.rs enqueue_inner                                | Hook gets a Deny DecisionResponse (server.rs hook arm);|
 * |    |                                                             |                                                       | NOT broadcast to TUI/web — queue.rs is outside this    |
 * |    |                                                             |                                                       | change's ownership; needs a follow-up wire event       |
 * | 10 | 5-min approval expiry                                       | server.rs spawn expiry task → worker Denied           | Correlated `command_error` → Modal/Banner, message     |
 * |    |                                                             |                                                       | names the true cause ("SpawnAgent approval expired")   |
 * | 11 | Human deny                                                  | finalize_spawn_decision → worker Denied               | Correlated `command_error` → Banner (+ queue resolves; |
 * |    |                                                             |                                                       | an operator deny message travels through verbatim)     |
 * | 12 | Post-approval mode re-check refusal                         | worker action ensure_spawn_mode_active                | Correlated `command_error` → Modal/Banner              |
 * | 13 | Claude hook-gate refusal                                    | process_registry spawn gate → worker Action err       | Correlated `command_error` → Modal/Banner              |
 * | 14 | Codex gate / foreign-hook refusal                           | process_registry codex gate → worker Action err       | Correlated `command_error` → Modal/Banner              |
 * | 15 | Approval-not-durably-recorded fail-closed                   | finalize_spawn_decision persistence arm → worker Deny | Correlated `command_error` → Modal/Banner, message     |
 * |    |                                                             |                                                       | names the true cause ("approval could not be recorded")|
 * | 16 | Approval channel dropped                                    | worker SpawnRunError::ChannelClosed                   | Correlated `command_error` → Modal/Banner              |
 *
 * Rows 4–8 answer synchronously on the submitting connection; rows 10–16 are
 * async worker outcomes sent to the originating connection over conn_tx.
 * The typed `command_error` (with the submit's `correlation_id` echoed) goes
 * only to WEB-ORIGIN callers — the split keys on the envelope's
 * authenticated `device_id`, never on `correlation_id`, because the CLI
 * stamps correlation ids too but only understands the legacy bare `error`
 * (spawn_error_reply, server.rs). Rows 10/15 carry the resolving decision's
 * own message so expiry/persistence-failure no longer masquerade as a human
 * deny (SpawnRunError::Denied threads the RichDecision message; audit
 * attribution was already correct and is unchanged).
 * Row 9 is the one path this change could not surface to the operator: its
 * only wire reply targets the *hook* connection, and the general enqueue path
 * lives in queue.rs, outside this change's file ownership.
 */
export function ErrorBanner({
  errors,
  onDismiss,
}: {
  errors: UiError[];
  onDismiss: (id: number) => void;
}) {
  if (errors.length === 0) return null;
  return (
    <div className="error-banner" role="alert" aria-live="assertive">
      {errors.map((e) => (
        <div key={e.id} className={`error-alert error-alert-${e.source}`}>
          <span className="error-alert-icon" aria-hidden="true">
            ⚠
          </span>
          <span className="error-alert-label">{SOURCE_LABEL[e.source]}</span>
          <span className="error-alert-message">{e.message}</span>
          {e.count > 1 && (
            <span
              className="error-alert-count"
              title={`Repeated ${e.count} times`}
            >
              ×{e.count}
            </span>
          )}
          <button
            className="error-alert-dismiss"
            aria-label={`Dismiss ${SOURCE_LABEL[e.source].toLowerCase()}: ${e.message}`}
            onClick={() => onDismiss(e.id)}
          >
            ✕
          </button>
        </div>
      ))}
    </div>
  );
}
