// A shopping cart: the property suite's JavaScript sample.
import { EventEmitter } from "node:events";
import fs from "node:fs/promises";

export const TAX_RATE = 0.2;

const formatter = new Intl.NumberFormat("en-GB", { style: "currency", currency: "GBP" });

export function format(cents) {
  return formatter.format(cents / 100);
}

export class Cart extends EventEmitter {
  #lines = new Map();
  static #carts = 0;

  constructor(owner, { currency = "GBP", discounts = [] } = {}) {
    super();
    this.owner = owner;
    this.currency = currency;
    this.discounts = discounts;
    Cart.#carts += 1;
  }

  static get count() {
    return Cart.#carts;
  }

  add(sku, price, quantity = 1) {
    if (quantity <= 0) {
      throw new RangeError(`quantity must be positive, got ${quantity}`);
    }
    const line = this.#lines.get(sku) ?? { sku, price, quantity: 0 };
    line.quantity += quantity;
    this.#lines.set(sku, line);
    this.emit("change", { sku, quantity: line.quantity });
    return this;
  }

  remove(sku) {
    const removed = this.#lines.delete(sku);
    if (removed) {
      this.emit("change", { sku, quantity: 0 });
    }
    return removed;
  }

  get lines() {
    return [...this.#lines.values()];
  }

  subtotal() {
    return this.lines.reduce((sum, { price, quantity }) => sum + price * quantity, 0);
  }

  total() {
    const discounted = this.discounts.reduce((cents, rule) => rule(cents, this), this.subtotal());
    return Math.round(discounted * (1 + TAX_RATE));
  }

  *[Symbol.iterator]() {
    for (const line of this.#lines.values()) {
      yield line;
    }
  }

  async save(path) {
    const data = JSON.stringify({ owner: this.owner, lines: this.lines }, null, 2);
    await fs.writeFile(path, data, "utf8");
  }

  static async load(path) {
    const text = await fs.readFile(path, "utf8");
    const { owner, lines } = JSON.parse(text);
    const cart = new Cart(owner);
    for (const { sku, price, quantity } of lines) {
      cart.add(sku, price, quantity);
    }
    return cart;
  }
}

export const percentOff = (percent) => (cents) => cents * (1 - percent / 100);

export function bulkDiscount(threshold, percent) {
  return function (cents, cart) {
    const items = cart.lines.reduce((n, line) => n + line.quantity, 0);
    return items >= threshold ? cents * (1 - percent / 100) : cents;
  };
}

function describe(cart) {
  switch (cart.lines.length) {
    case 0:
      return "empty";
    case 1:
      return "one line";
    default:
      return `${cart.lines.length} lines`;
  }
}

export async function checkout(cart, pay) {
  if (cart.lines.length === 0) {
    return { ok: false, reason: "empty cart" };
  }
  try {
    const receipt = await pay(cart.total(), cart.currency);
    return { ok: true, receipt };
  } catch (error) {
    console.error("payment failed:", error.message);
    return { ok: false, reason: error.message };
  } finally {
    cart.emit("checkout", describe(cart));
  }
}

const demo = async () => {
  const cart = new Cart("ana", { discounts: [percentOff(10), bulkDiscount(5, 5)] });
  cart.add("MLK", 129, 2).add("RCE", 349);
  cart.on("change", ({ sku, quantity }) => console.log(`${sku} x${quantity}`));
  const result = await checkout(cart, async (cents) => ({ id: "r-1", cents }));
  console.log(format(cart.total()), result.ok ? "paid" : result.reason);
};

demo();
