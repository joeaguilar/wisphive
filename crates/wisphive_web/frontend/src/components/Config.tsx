import { useCallback, useEffect, useState } from "react";
import { apiFetch } from "../api";

const LEVELS = ["off", "read", "write", "execute", "all"] as const;
type Level = typeof LEVELS[number];

// Tool tiers come from GET /api/tool-tiers (sourced from wisphive_protocol — the
// single source of truth, itr#121) instead of being hardcoded here, so the SPA
// can't drift from the tiers the hook actually enforces.
type ToolTiers = { read: string[]; write: string[]; execute: string[]; always_ask: string[] };

interface Config {
  auto_approve_level?: Level;
  auto_approve_add?: string[];
  auto_approve_remove?: string[];
}

function getToolStatus(tool: string, level: Level, add: string[], remove: string[], levelTools: Record<string, string[]>): { auto: boolean; source: string } {
  const isRemoved = remove.includes(tool);
  const isAdded = add.includes(tool);

  if (isAdded) return { auto: true, source: "override" };
  if (isRemoved) return { auto: false, source: "override" };

  // Check if the level includes this tool
  const levelOrder = ["off", "read", "write", "execute", "all"];
  const currentIdx = levelOrder.indexOf(level);

  if (level === "all") return { auto: true, source: "level" };

  for (const [lvl, tools] of Object.entries(levelTools)) {
    const lvlIdx = levelOrder.indexOf(lvl);
    if (lvlIdx <= currentIdx && tools.includes(tool)) {
      return { auto: true, source: "level" };
    }
  }

  return { auto: false, source: "level" };
}

export function ConfigView() {
  const [level, setLevel] = useState<Level>("read");
  const [add, setAdd] = useState<string[]>([]);
  const [remove, setRemove] = useState<string[]>([]);
  const [levelTools, setLevelTools] = useState<Record<string, string[]>>({ read: [], write: [], execute: [] });
  const [loading, setLoading] = useState(true);

  const loadConfig = useCallback(async () => {
    try {
      const [cfgRes, tiersRes] = await Promise.all([
        apiFetch(`/api/config`),
        apiFetch(`/api/tool-tiers`),
      ]);
      const data: Config = await cfgRes.json();
      const tiers: ToolTiers = await tiersRes.json();
      setLevel(data.auto_approve_level || "read");
      setAdd(data.auto_approve_add || []);
      setRemove(data.auto_approve_remove || []);
      setLevelTools({ read: tiers.read, write: tiers.write, execute: tiers.execute });
    } catch (e) {
      console.warn("Failed to load config:", e);
    }
    setLoading(false);
  }, []);

  // eslint-disable-next-line react-hooks/set-state-in-effect
  useEffect(() => { loadConfig(); }, [loadConfig]);

  const saveConfig = useCallback(async (newLevel: Level, newAdd: string[], newRemove: string[]) => {
    const config: Config = {
      auto_approve_level: newLevel,
      auto_approve_add: newAdd.length > 0 ? newAdd : undefined,
      auto_approve_remove: newRemove.length > 0 ? newRemove : undefined,
    };
    try {
      await apiFetch(`/api/config`, {
        method: "PUT",
        headers: { "Content-Type": "application/json" },
        body: JSON.stringify(config),
      });
    } catch (e) {
      console.warn("Failed to save config:", e);
    }
  }, []);

  const toggleTool = (tool: string) => {
    const status = getToolStatus(tool, level, add, remove, levelTools);
    let newAdd = [...add];
    let newRemove = [...remove];

    if (status.auto) {
      // Currently auto-approved → remove it
      if (status.source === "override") {
        newAdd = newAdd.filter((t) => t !== tool);
      } else {
        newRemove.push(tool);
      }
    } else {
      // Currently queued → add it
      if (status.source === "override") {
        newRemove = newRemove.filter((t) => t !== tool);
      } else {
        newAdd.push(tool);
      }
    }

    setAdd(newAdd);
    setRemove(newRemove);
    saveConfig(level, newAdd, newRemove);
  };

  const changeLevel = (newLevel: Level) => {
    setLevel(newLevel);
    saveConfig(newLevel, add, remove);
  };

  if (loading) return <div className="history-empty">Loading config...</div>;

  return (
    <div className="config-view">
      <div className="sessions-toolbar">
        <h2>Auto-Approve Configuration</h2>
      </div>

      <div className="config-level">
        <span className="config-label">Level:</span>
        <div className="level-selector">
          {LEVELS.map((l) => (
            <button
              key={l}
              className={`level-btn ${level === l ? "active" : ""}`}
              onClick={() => changeLevel(l)}
            >
              {l.charAt(0).toUpperCase() + l.slice(1)}
            </button>
          ))}
        </div>
      </div>

      <div className="config-tools">
        {["Read Tier", "Write Tier", "Execute Tier"].map((tierLabel, tierIdx) => {
          const tierKey = ["read", "write", "execute"][tierIdx];
          const tools = levelTools[tierKey];
          return (
            <div key={tierKey} className="config-tier">
              <h3>{tierLabel}</h3>
              <div className="tool-list">
                {tools.map((tool) => {
                  const status = getToolStatus(tool, level, add, remove, levelTools);
                  return (
                    <div
                      key={tool}
                      className={`tool-row ${status.auto ? "auto" : "queued"} ${status.source}`}
                      onClick={() => toggleTool(tool)}
                    >
                      <span className={`tool-checkbox ${status.auto ? "checked" : ""}`}>
                        {status.auto ? "✓" : " "}
                      </span>
                      <span className="tool-name-config">{tool}</span>
                      <span className={`tool-status ${status.auto ? "status-auto" : "status-queued"}`}>
                        {status.auto ? "AUTO" : "QUEUED"}
                        {status.source === "override" && " (override)"}
                      </span>
                    </div>
                  );
                })}
              </div>
            </div>
          );
        })}
      </div>
    </div>
  );
}
