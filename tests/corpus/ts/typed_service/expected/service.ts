import { Callback } from "./promises";
import type { Config } from "./config";

interface AsyncHandler {
  name: string;
  run(input: string): void;
}

export class Service {
  private readonly config: Config;

  static async load(path: string): Promise<Config> {
    read(path);
  }

  fetch(id: string): Promise<number> {
    lookup(id);
  }
}
