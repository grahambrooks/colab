const config = require("./config");

function start(port) {
  console.log("starting on", port);
  if (config.debug) {
    console.log("config:", config);
  }
  return listen(port, () => console.log("ready"));
}

module.exports = { start };
