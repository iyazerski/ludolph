import { expect, test } from "bun:test";
import { createContext, runInContext } from "node:vm";
import { Window } from "happy-dom";

const app = await Bun.file(new URL("../assets/app.js", import.meta.url)).text();
const sealing = await Bun.file(new URL("../assets/seal.js", import.meta.url)).text();

for (const fails of [false, true]) {
  test(`new chat shows its first message while starting, then ${fails ? "restores a failed draft" : "opens the chat"}`, async () => {
    const window = new Window({ url: "https://relay.example/#/new" });
    const { document } = window;
    document.body.innerHTML = '<div id="app"></div><div id="toasts"></div>';
    const context = createContext({
      window, document, location: window.location, history: window.history,
      navigator: window.navigator, localStorage: window.localStorage,
      Event: window.Event, ResizeObserver: window.ResizeObserver,
      matchMedia: window.matchMedia.bind(window),
      setTimeout: window.setTimeout.bind(window), clearTimeout: window.clearTimeout.bind(window),
      setInterval: () => 0, clearInterval() {},
      crypto, TextEncoder, TextDecoder, btoa, atob, AbortSignal,
      PROVIDER_MARKS: {},
    });
    runInContext(sealing, context);
    const seal = runInContext("Seal", context);
    const link = await seal.derive("AAECAwQFBgcICQoLDA0ODxAREhMUFRYXGBkaGxwdHh8");
    seal.links = () => new Promise(() => {});
    const creation = Promise.withResolvers();
    const calls = [];
    context.fetch = async (path, options) => {
      if (path.startsWith("/api/watch/")) return new Promise(() => {});
      expect(path).toBe("/api/call/mac");
      const { data } = JSON.parse(options.body);
      const request = JSON.parse(await seal.open(link.key, seal.aad.call(), data));
      calls.push(request);
      const replies = { get_settings: {}, list_projects: { projects: [] }, list_models: { models: [] }, composer_info: {} };
      const reply = request.tool === "create_chat" ? await creation.promise : { ok: replies[request.tool] };
      const nonce = Uint8Array.from(atob(data.replace(/-/g, "+").replace(/_/g, "/")), c => c.charCodeAt(0)).slice(1, 13);
      const sealed = await seal.seal(link.key, seal.aad.reply(nonce), JSON.stringify(reply));
      return Response.json({ sealed: sealed.data });
    };
    try {
      runInContext(app, context);
      context.link = link;
      runInContext("links.set('mac', link); store.machines = [{id: 'mac', name: 'Mac', status: {online: true}}]; route()", context);
      const view = document.querySelector(".view");
      const textarea = view.querySelector('[aria-label="Message"]');
      const send = view.querySelector('[aria-label="Send"]');
      textarea.value = "Build a clock";
      textarea.dispatchEvent(new window.Event("input"));
      send.click();

      expect(view.querySelector(".msg.user .bubble")?.textContent).toBe("Build a clock");
      expect(view.querySelector(".hero:not(.hide)")).toBeNull();
      expect(view.querySelector(".working:not(.hide)")?.textContent).toContain("Starting chat");
      expect(textarea.value).toBe("");
      expect(textarea.disabled).toBe(true);
      expect(send.disabled).toBe(true);
      send.click();

      const finished = new Promise(resolve => {
        const observer = new window.MutationObserver(() => {
          if (document.querySelector(".toast") || window.location.hash !== "#/new") {
            observer.disconnect();
            resolve();
          }
        });
        observer.observe(document.body, { childList: true, subtree: true });
      });
      creation.resolve(fails ? { error: "pi couldn't start" } : { ok: { chatId: "chat" } });
      await finished;

      expect(calls.filter(c => c.tool === "create_chat")).toHaveLength(1);
      if (fails) {
        expect(window.location.hash).toBe("#/new");
        expect(view.querySelector(".msg.user")).toBeNull();
        expect(view.querySelector(".working:not(.hide)")).toBeNull();
        expect(view.querySelector(".hero:not(.hide)")).not.toBeNull();
        expect(textarea.value).toBe("Build a clock");
        expect(textarea.disabled).toBe(false);
        expect(send.disabled).toBe(false);
        expect(document.querySelector(".toast").textContent).toContain("pi couldn't start");
      } else {
        expect(window.location.hash).toBe("#/m/mac/chat/chat");
        expect(document.querySelector("#app .view:not(.leave) .msg.user .bubble")?.textContent).toBe("Build a clock");
      }
    } finally {
      runInContext("current?.destroy()", context);
      await window.happyDOM.close();
    }
  });
}
