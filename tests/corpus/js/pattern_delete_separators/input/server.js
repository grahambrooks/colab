import { serve } from "./server.js";

serve({ host: "0.0.0.0", debug: true, port: 8080 });
serve({ port: 3000, debug: true });
serve({
  port: 3000,
  debug: true,
});
