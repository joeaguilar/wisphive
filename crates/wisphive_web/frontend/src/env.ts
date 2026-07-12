/**
 * Build-time frontend environment settings.
 *
 * Vite replaces `import.meta.env` while building, but a configured blank is
 * otherwise indistinguishable from an omitted value when callers use `||`.
 * Validate it once here so split-origin deployments fail at startup instead
 * of silently connecting to the current origin.
 */
function optionalUrl(name: string, value: string | undefined): string | undefined {
  if (value !== undefined && value.trim() === "") {
    throw new Error(`${name} must not be empty; omit it to use the default origin.`);
  }
  return value;
}

export const environment = Object.freeze({
  apiUrl: optionalUrl("VITE_API_URL", import.meta.env.VITE_API_URL),
  wsUrl: optionalUrl("VITE_WS_URL", import.meta.env.VITE_WS_URL),
});
