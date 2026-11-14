import http from 'http';
import path from "path";
const { readFile } = require('fs/promises');
import helpers from './helpers';

function lazy() {
  const heavy = require('heavy');
  return heavy.load();
}

if (process.env.DEBUG) {
  const debug = require('debug');
  debug.enable('*');
}

export const port = 8080;
export const root = path.join(__dirname, 'public');

function start() {
  return http.createServer(helpers.handler).listen(exports.port);
}

export default start;
