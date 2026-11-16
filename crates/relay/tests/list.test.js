import { expect, test } from "bun:test";
import { createContext, runInContext } from "node:vm";
import { Window } from "happy-dom";

const app = await Bun.file(new URL("../assets/app.js", import.meta.url)).text();
const sealing = await Bun.file(new URL("../assets/seal.js", import.meta.url)).text();

for (const slow of ["hangs", "fails"]) {
  test(`a Mac that ${slow} doesn't hide the other Mac`, async () => {
    const window = new Window({ url: "https://relay.example/#/" });
    const { document } = window;
    document.body.innerHTML = '<div id="app"></div><div id="toasts"></div>';
    window.localStorage.setItem("ludolph.mac", "vpn");
    const context = createContext({
      window, document, location: window.location, history: window.history,
      navigator: window.navigator, localStorage: window.localStorage,
      Event: window.Event, ResizeObserver: window.ResizeObserver,
      matchMedia: window.matchMedia.bind(window),
      setTimeout: window.setTimeout.bind(window), clearTimeout: window.clearTimeout.bind(window),
      setInterval: () => 0, clearInterval() {},
      crypto, TextEncoder, TextDecoder, btoa, atob, AbortSignal, AbortController,
      PROVIDER_MARKS: {},
    });
    runInContext(sealing, context);
    const seal = runInContext("Seal", context);
    const link = await seal.derive("AAECAwQFBgcICQoLDA0ODxAREhMUFRYXGBkaGxwdHh8");
    seal.links = () => new Promise(() => {});
    const online = { online: true, version: "1" };
    context.fetch = async (path, options) => {
      if (path === "/api/machines") {
        return Response.json({ machines: [{ id: "vpn", name: "Work", status: online }, { id: "home", name: "Home", status: online }] });
      }
      if (path === "/api/call/vpn") {
        return slow === "hangs" ? new Promise(() => {}) : Response.json({ error: "this Mac didn't answer in time" }, { status: 400 });
      }
      expect(path).toBe("/api/call/home");
      const { data } = JSON.parse(options.body);
      const chats = [{ id: "chat", title: "Build a clock", status: "idle", lastMessageAt: "2026-01-01T00:00:00Z" }];
      const nonce = Uint8Array.from(atob(data.replace(/-/g, "+").replace(/_/g, "/")), c => c.charCodeAt(0)).slice(1, 13);
      const sealed = await seal.seal(link.key, seal.aad.reply(nonce), JSON.stringify({ ok: { chats } }));
      return Response.json({ sealed: sealed.data });
    };
    try {
      runInContext(app, context);
      context.link = link;
      runInContext("links.set('vpn', link); links.set('home', link); route()", context);
      const settle = () => new Promise(resolve => window.setTimeout(resolve, 20));
      await settle();
      const tabs = [...document.querySelectorAll(".mac-tab")];
      expect(tabs.map(t => t.textContent)).toEqual(["Work", "Home"]);
      const note = document.querySelector(".offline-note")?.textContent;
      expect(note).toBe(slow === "fails" ? "Work isn't answering. Retrying…" : undefined);

      tabs[1].click();
      await settle();
      expect(document.querySelector(".row .title")?.textContent).toBe("Build a clock");
    } finally {
      runInContext("current?.destroy()", context);
      await window.happyDOM.close();
    }
  });
}

test("a notice settles its chat in the list before the next poll", async () => {
  const window = new Window({ url: "https://relay.example/#/" });
  const { document } = window;
  document.body.innerHTML = '<div id="app"></div><div id="toasts"></div>';
  const worker = new window.EventTarget();
  worker.register = () => Promise.resolve();
  const context = createContext({
    window, document, location: window.location, history: window.history,
    navigator: { serviceWorker: worker }, localStorage: window.localStorage,
    Event: window.Event, ResizeObserver: window.ResizeObserver,
    matchMedia: window.matchMedia.bind(window),
    setTimeout: window.setTimeout.bind(window), clearTimeout: window.clearTimeout.bind(window),
    setInterval: () => 0, clearInterval() {},
    crypto, TextEncoder, TextDecoder, btoa, atob, AbortSignal, AbortController,
    PROVIDER_MARKS: {},
  });
  runInContext(sealing, context);
  const seal = runInContext("Seal", context);
  const link = await seal.derive("AAECAwQFBgcICQoLDA0ODxAREhMUFRYXGBkaGxwdHh8");
  seal.links = () => new Promise(() => {});
  let lists = 0;
  context.fetch = async (path, options) => {
    if (path === "/api/machines") return Response.json({ machines: [{ id: "mac", name: "Mac", status: { online: true } }] });
    const { data } = JSON.parse(options.body);
    const request = JSON.parse(await seal.open(link.key, seal.aad.call(), data));
    expect(request.tool).toBe("list_chats");
    // Later polls never answer, so only the notice can change the row.
    if (lists++) return new Promise(() => {});
    const chats = [{ id: "chat", title: "Build a clock", status: "working", lastMessageAt: "2026-01-01T00:00:00Z" }];
    const nonce = Uint8Array.from(atob(data.replace(/-/g, "+").replace(/_/g, "/")), c => c.charCodeAt(0)).slice(1, 13);
    const sealed = await seal.seal(link.key, seal.aad.reply(nonce), JSON.stringify({ ok: { chats } }));
    return Response.json({ sealed: sealed.data });
  };
  try {
    runInContext(app, context);
    context.link = link;
    runInContext("links.set('mac', link); route()", context);
    const corner = async () => {
      await new Promise(resolve => window.setTimeout(resolve, 20));
      return document.querySelector(".row .corner")?.textContent;
    };
    expect(await corner()).toBe("Working");

    const notice = { kind: "finished", machine: "mac", chat: "chat", title: "Build a clock", body: "Done" };
    worker.dispatchEvent(new window.MessageEvent("message", { data: { type: "notice", notice, shown: true } }));

    expect(await corner()).not.toBe("Working");
    expect(lists).toBe(2);
    expect(document.querySelector(".toast")).toBeNull();
  } finally {
    runInContext("current?.destroy()", context);
    await window.happyDOM.close();
  }
});
