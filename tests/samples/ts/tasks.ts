// A task tracker: the property suite's TypeScript sample.
import { readFile, writeFile } from "node:fs/promises";
import type { Stats } from "node:fs";

export type Priority = "low" | "normal" | "high";

export interface Task {
  readonly id: number;
  title: string;
  priority: Priority;
  done: boolean;
  due?: Date;
  tags: string[];
}

export enum Column {
  Todo = "todo",
  Doing = "doing",
  Done = "done",
}

function sealed(constructor: Function): void {
  Object.seal(constructor);
  Object.seal(constructor.prototype);
}

export class TaskError extends Error {
  constructor(public readonly taskId: number, message: string) {
    super(`task ${taskId}: ${message}`);
  }
}

@sealed
export class Board<T extends Task = Task> {
  private tasks: Map<number, T> = new Map();
  private nextId = 1;
  protected readonly columns: Record<Column, number[]> = {
    [Column.Todo]: [],
    [Column.Doing]: [],
    [Column.Done]: [],
  };
  static instances = 0;

  constructor(public readonly name: string) {
    Board.instances += 1;
  }

  add(fields: Omit<T, "id" | "done">): T {
    const task = { ...fields, id: this.nextId++, done: false } as T;
    this.tasks.set(task.id, task);
    this.columns[Column.Todo].push(task.id);
    return task;
  }

  get(id: number): T {
    const task = this.tasks.get(id);
    if (task === undefined) {
      throw new TaskError(id, "not found");
    }
    return task;
  }

  move(id: number, to: Column): void {
    for (const ids of Object.values(this.columns)) {
      const at = ids.indexOf(id);
      if (at >= 0) {
        ids.splice(at, 1);
      }
    }
    this.columns[to].push(id);
    this.get(id).done = to === Column.Done;
  }

  private static weight(priority: Priority): number {
    switch (priority) {
      case "high":
        return 3;
      case "normal":
        return 2;
      default:
        return 1;
    }
  }

  public sorted(): T[] {
    return [...this.tasks.values()].sort(
      (a, b) => Board.weight(b.priority) - Board.weight(a.priority) || a.id - b.id,
    );
  }

  overdue(now: Date = new Date()): T[] {
    return this.sorted().filter((t) => !t.done && t.due !== undefined && t.due < now);
  }

  tagged(...tags: string[]): T[] {
    return this.sorted().filter((t) => tags.every((tag) => t.tags.includes(tag)));
  }

  get size(): number {
    return this.tasks.size;
  }

  async save(path: string): Promise<void> {
    const data = JSON.stringify({ name: this.name, tasks: this.sorted() }, null, 2);
    await writeFile(path, data, "utf8");
  }

  static async load(path: string): Promise<Board> {
    const { name, tasks } = JSON.parse(await readFile(path, "utf8")) as {
      name: string;
      tasks: Task[];
    };
    const board = new Board(name);
    for (const { title, priority, tags } of tasks) {
      board.add({ title, priority, tags });
    }
    return board;
  }
}

export function summarize<T extends Task>(board: Board<T>): string {
  const done = board.sorted().filter((t) => t.done).length;
  return `${board.name}: ${done}/${board.size} done`;
}

export const isUrgent = (task: Task): boolean => task.priority === "high" && !task.done;

export function describe(stats: Stats | null): string {
  if (stats === null) {
    return "missing";
  }
  return stats.isDirectory() ? "a directory" : `${stats.size} bytes`;
}

async function demo(): Promise<void> {
  const board = new Board("home");
  const paint = board.add({ title: "Paint the fence", priority: "low", tags: ["outside"] });
  board.add({ title: "Pay rent", priority: "high", tags: ["money"], due: new Date(2026, 0, 1) });
  board.move(paint.id, Column.Done);
  console.log(summarize(board), board.overdue().map((t) => t.title));
}

void demo();
