export const SENTINEL_STATUS_LINE_COMMAND = "agent-sentinel claude-status-line";
export type ClaudeSettings = Record<string, unknown>;
export type StatusLineBackup = { previous: unknown; existed: boolean };

export function installClaudeStatusLineBridge(settings: ClaudeSettings): { settings: ClaudeSettings; backup: StatusLineBackup } | null {
  const existed = Object.prototype.hasOwnProperty.call(settings, "statusLine");
  const previous = settings.statusLine;
  if (existed && JSON.stringify(previous) !== JSON.stringify({ type: "command", command: SENTINEL_STATUS_LINE_COMMAND })) return null;
  return { settings: { ...settings, statusLine: { type: "command", command: SENTINEL_STATUS_LINE_COMMAND } }, backup: { previous, existed } };
}
export function restoreClaudeStatusLineBridge(settings: ClaudeSettings, backup: StatusLineBackup): ClaudeSettings {
  if (JSON.stringify(settings.statusLine) !== JSON.stringify({ type: "command", command: SENTINEL_STATUS_LINE_COMMAND })) return settings;
  const restored = { ...settings }; if (backup.existed) restored.statusLine = backup.previous; else delete restored.statusLine; return restored;
}
