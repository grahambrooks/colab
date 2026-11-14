const http = require('http');
const path = require("path");
const { readFile } = require('fs/promises');
const helpers = require('./helpers');

function lazy() {
  const heavy = require('heavy');
  return heavy.load();
}

if (process.env.DEBUG) {
  const debug = require('debug');
  debug.enable('*');
}

exports.port = 8080;
exports.root = path.join(__dirname, 'public');

function start() {
  return http.createServer(helpers.handler).listen(exports.port);
}

module.exports = start;
