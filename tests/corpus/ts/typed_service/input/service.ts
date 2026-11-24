import { Callback } from "./callbacks";
import type { Config } from "./config";

interface Handler {
  name: string;
  run(input: string): void;
}

export class Service {
  private readonly config: Config;

  static async load(path: string, done: Callback<Config>): void {
    read(path);
  }

  fetch(id: string, done: Callback<number>): void {
    lookup(id);
  }
}
