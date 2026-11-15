import { expect, test } from "bun:test";
import { createContext, runInContext } from "node:vm";

const app = await Bun.file(new URL("../assets/app.js", import.meta.url)).text();
const sealing = await Bun.file(new URL("../assets/seal.js", import.meta.url)).text();

for (const [name, outcomes, accepted] of [
  ["a first offline answer fails immediately", ["this Mac is offline"], false],
  ["a disconnect stays uncertain while the Mac is offline", ["this Mac went offline", "this Mac is offline", "ok"], true],
  ["a network failure stays uncertain while the Mac is offline", ["network", "this Mac is offline", "ok"], true],
  ["a pending call keeps waiting while the Mac is offline", ["pending", "this Mac is offline", "ok"], true],
  ["a busy Mac has not accepted the call", ["this Mac is busy; try again", "this Mac is offline"], false],
]) {
  test(name, async () => {
    const context = createContext({
      crypto, TextEncoder, TextDecoder, btoa, atob, AbortSignal,
      matchMedia: () => ({ matches: false }),
      localStorage: { getItem: () => null },
      document: { addEventListener() {} },
      window: { addEventListener() {} },
      navigator: {},
      setTimeout: callback => queueMicrotask(callback),
    });
    runInContext(sealing, context);
    const seal = runInContext("Seal", context);
    const link = await seal.derive("AAECAwQFBgcICQoLDA0ODxAREhMUFRYXGBkaGxwdHh8");
    // Leave startup waiting for storage, without opening a page or using real links.
    seal.links = () => new Promise(() => {});
    const calls = [];
    context.fetch = async (path, options) => {
      expect(path).toBe("/api/call/mac");
      const { data } = JSON.parse(options.body);
      calls.push(JSON.parse(await seal.open(link.key, seal.aad.call(), data)));
      const outcome = outcomes[calls.length - 1];
      if (outcome === "network") throw new TypeError("connection lost");
      if (outcome !== "ok" && outcome !== "pending") {
        return Response.json({ error: outcome }, { status: 503 });
      }
      const nonce = Uint8Array.from(atob(data.replace(/-/g, "+").replace(/_/g, "/")), c => c.charCodeAt(0)).slice(1, 13);
      const result = outcome === "pending" ? { pending: true } : { ok: { delivery: "run" } };
      const sealed = await seal.seal(link.key, seal.aad.reply(nonce), JSON.stringify(result));
      return Response.json({ sealed: sealed.data });
    };
    runInContext(app, context);
    context.link = link;
    runInContext("links.set('mac', link)", context);
    const sending = runInContext("call('mac', 'send_message', {text: 'hello'})", context);
    if (accepted) {
      expect(await sending).toEqual({ delivery: "run" });
    } else {
      await expect(sending).rejects.toThrow("this Mac is offline");
    }
    expect(calls).toHaveLength(outcomes.length);
    expect(new Set(calls.map(call => call.op)).size).toBe(1);
    expect(calls.every(call => call.tool === "send_message" && call.args.text === "hello")).toBe(true);
  });
}
