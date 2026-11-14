import { createClient } from "db";

export const primary = createClient({ host: "db", timeout: 5, retries: 3 });

export const replica = createClient({
  host: "replica",
  timeout: 10,
  tls: true,
});

export const old = createClient({ legacy: true, timeout: 1 });

export const job = schedule({ timeout: 60 });
