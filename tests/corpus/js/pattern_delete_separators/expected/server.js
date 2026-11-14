import { serve } from "./server.js";

serve({ host: "0.0.0.0", port: 8080 });
serve({ port: 3000 });
serve({
  port: 3000,
});
