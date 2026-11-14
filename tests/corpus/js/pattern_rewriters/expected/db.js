import { createClient } from "db";

export const primary = connect({ host: "db", timeoutMs: 5 * 1000, maxRetries: 3 });

export const replica = connect({
  host: "replica",
  timeoutMs: 10 * 1000,
  tls: true,
});

export const old = createClient({ legacy: true, timeout: 1 });

export const job = schedule({ timeout: 60 });
