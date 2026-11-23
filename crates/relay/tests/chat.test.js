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
      crypto, TextEncoder, TextDecoder, btoa, atob, AbortSignal, AbortController,
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
          // Wait for the routed view too: happy-dom fires hashchange after other mutations.
          if (document.querySelector(".toast") || document.querySelector("#app .view:not(.leave)") !== view) {
            observer.disconnect();
            resolve();
          }
        });
        observer.observe(document.body, { childList: true, subtree: true });
      });
      creation.resolve(fails ? { error: "pi couldn't start" } : { ok: { chatId: "chat", text: "Build a clock" } });
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
        const opened = document.querySelector("#app .view:not(.leave)");
        expect(opened).not.toBe(view);
        expect(opened.querySelector(".msg.user .bubble")?.textContent).toBe("Build a clock");
      }
    } finally {
      runInContext("current?.destroy()", context);
      await window.happyDOM.close();
    }
  });
}


test("chat details show the context used and the provider's usage limits", async () => {
  const window = new Window({ url: "https://relay.example/#/m/mac/chat/test/details" });
  const { document } = window;
  document.body.innerHTML = '<div id="app"></div><div id="toasts"></div>';
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
  const details = {
    compactions: 0, pullRequests: [], spent: { tokens: 2_000_000, cost: 0.5 }, changes: null,
    contextUsage: { tokens: 1500, window: 10000 },
    quota: [
      { label: "5h", used: 0.12, resetsAt: Date.now() + 3 * 3600_000 + 60_000 },
      { label: "Week", used: 0.8, resetsAt: Date.now() + 5 * 86400_000, spent: 160, limit: 200 },
    ],
  };
  context.fetch = async (path, options) => {
    if (path.startsWith("/api/watch/")) return new Promise(() => {});
    const { data } = JSON.parse(options.body);
    const request = JSON.parse(await seal.open(link.key, seal.aad.call(), data));
    const nonce = Uint8Array.from(atob(data.replace(/-/g, "+").replace(/_/g, "/")), c => c.charCodeAt(0)).slice(1, 13);
    const sealed = await seal.seal(link.key, seal.aad.reply(nonce), JSON.stringify({ ok: request.tool === "chat_details" ? details : {} }));
    return Response.json({ sealed: sealed.data });
  };
  try {
    runInContext(app, context);
    context.link = link;
    const drawn = new Promise(resolve => {
      const observer = new window.MutationObserver(() => {
        if (document.querySelector(".limit")) {
          observer.disconnect();
          resolve();
        }
      });
      observer.observe(document.body, { childList: true, subtree: true });
    });
    runInContext("links.set('mac', link); store.machines = [{id: 'mac', name: 'Mac', status: {online: true}}]; route()", context);
    await drawn;
    const limits = [...document.querySelectorAll(".limit")];
    expect(limits.map(l => [...l.querySelectorAll(".limit-head span")].map(s => s.textContent))).toEqual([["5h", "12% used"], ["Week", "$160.00 of $200.00"]]);
    expect(limits[0].querySelector("p").textContent).toBe("Resets in 3h");
    expect(limits[1].querySelector("p").textContent).toStartWith("Resets ");
    expect(limits[1].querySelector(".meter > div").style.width).toBe("80%");
    expect(limits[1].querySelector(".meter > div").style.background).toBe("var(--warning)");
    expect([...document.querySelectorAll(".chat-details h2")].at(-1).textContent).toBe("Current session");
    expect([...document.querySelectorAll(".chat-details p")].at(-1).textContent).toBe("0 compactions · 1.5k/10k context · $0.50");
  } finally {
    runInContext("current?.destroy()", context);
    await window.happyDOM.close();
  }
});

test("user and agent skill mentions share styling without becoming links on the phone", async () => {
  const window = new Window({ url: "https://relay.example/#/m/mac/chat/test" });
  const { document } = window;
  document.body.innerHTML = '<div id="app"></div><div id="toasts"></div>';
  const context = createContext({
    window, document, location: window.location, history: window.history,
    navigator: window.navigator, localStorage: window.localStorage,
    Event: window.Event, ResizeObserver: window.ResizeObserver,
    matchMedia: window.matchMedia.bind(window),
    setTimeout: window.setTimeout.bind(window), clearTimeout: window.clearTimeout.bind(window),
    setInterval: () => 0, clearInterval() {},
    crypto, TextEncoder, TextDecoder, TextDecoderStream, btoa, atob, AbortSignal, AbortController,
    PROVIDER_MARKS: {},
  });
  runInContext(sealing, context);
  const seal = runInContext("Seal", context);
  const link = await seal.derive("AAECAwQFBgcICQoLDA0ODxAREhMUFRYXGBkaGxwdHh8");
  seal.links = () => new Promise(() => {});
  const source = "Use $review and $team:lint: with **$bold_skill**, $5 and `$literal` and [site $linked](https://example.org).\n\n```\n$block\n```";
  const calls = [];
  context.fetch = async (path, options) => {
    if (path.startsWith("/api/watch/")) {
      const data = new URL(path, window.location).searchParams.get("data");
      const nonce = Uint8Array.from(atob(data.replace(/-/g, "+").replace(/_/g, "/")), c => c.charCodeAt(0)).slice(1, 13);
      const frame = {
        reset: true, chat: { id: "test", name: "Skills", cwd: "/tmp", status: "idle" },
        ids: ["user", "assistant"], turnChanges: [], queued: [], olderRemaining: 0,
        messages: ["user", "assistant"].map(role => ({
          id: role, role, status: "complete", createdAt: 0, endedAt: 0,
          parts: [{ kind: "text", text: source }],
        })),
      };
      const sealed = await seal.seal(link.key, seal.aad.frame(nonce, 0), JSON.stringify(frame));
      return new Response(new ReadableStream({
        start(controller) {
          controller.enqueue(new TextEncoder().encode(`data: ${JSON.stringify({ sealed: sealed.data })}\n\n`));
          options.signal.addEventListener("abort", () => controller.close(), { once: true });
        },
      }));
    }
    const { data } = JSON.parse(options.body);
    const request = JSON.parse(await seal.open(link.key, seal.aad.call(), data));
    calls.push(request.tool);
    const nonce = Uint8Array.from(atob(data.replace(/-/g, "+").replace(/_/g, "/")), c => c.charCodeAt(0)).slice(1, 13);
    const replies = { get_settings: {}, list_models: { models: [] }, composer_info: {} };
    const sealed = await seal.seal(link.key, seal.aad.reply(nonce), JSON.stringify({ ok: replies[request.tool] }));
    return Response.json({ sealed: sealed.data });
  };
  try {
    runInContext(app, context);
    context.link = link;
    const drawn = new Promise(resolve => {
      const observer = new window.MutationObserver(() => {
        if (document.querySelector(".msg.assistant .skill-mention")) {
          observer.disconnect();
          resolve();
        }
      });
      observer.observe(document.body, { childList: true, subtree: true });
    });
    runInContext("links.set('mac', link); store.machines = [{id: 'mac', name: 'Mac', status: {online: true}}]; route()", context);
    await drawn;
    for (const role of ["user", "assistant"]) {
      const message = document.querySelector(`.msg.${role}`);
      const mentions = [...message.querySelectorAll(".skill-mention")];
      expect(mentions.map(node => node.textContent)).toEqual(["$review", "$team:lint", "$bold_skill"]);
      expect(message.querySelector("code").textContent).toBe("$literal");
      expect(message.querySelector("pre code").textContent).toBe("$block");
      expect(message.querySelector("a").textContent).toBe("site $linked");
      expect(mentions.every(node => node.tagName === "SPAN" && !node.hasAttribute("tabindex"))).toBe(true);
      const before = calls.length;
      mentions[0].click();
      expect(calls).toHaveLength(before);
      expect(window.location.hash).toBe("#/m/mac/chat/test");
    }
  } finally {
    runInContext("current?.destroy()", context);
    await window.happyDOM.close();
  }
});

test("model sheet shows provider labels, filters by model or provider, and uses a reasoning slider", async () => {
  const window = new Window({ url: "https://relay.example/#/m/mac/chat/test" });
  const { document } = window;
  document.body.innerHTML = '<div id="app"></div><div id="sheet-root"></div><div id="toasts"></div>';
  const context = createContext({
    window, document, location: window.location, history: window.history,
    navigator: window.navigator, localStorage: window.localStorage,
    Event: window.Event, ResizeObserver: window.ResizeObserver,
    matchMedia: window.matchMedia.bind(window),
    setTimeout: window.setTimeout.bind(window), clearTimeout: window.clearTimeout.bind(window),
    setInterval: () => 0, clearInterval() {},
    crypto, TextEncoder, TextDecoder, TextDecoderStream, btoa, atob, AbortSignal, AbortController,
    PROVIDER_MARKS: {},
  });
  runInContext(sealing, context);
  const seal = runInContext("Seal", context);
  const link = await seal.derive("AAECAwQFBgcICQoLDA0ODxAREhMUFRYXGBkaGxwdHh8");
  seal.links = () => new Promise(() => {});
  const models = [
    { id: "anthropic/claude-sonnet-4-5", label: "Claude Sonnet 4.5", reasoningLevels: ["off", "low", "medium", "high", "xhigh"] },
    { id: "openai/gpt-5", label: "GPT-5", reasoningLevels: ["low", "medium", "high"] },
    { id: "bifrost/fast", reasoningLevels: ["off"] },
  ];
  let current = { model: "anthropic/claude-sonnet-4-5", reasoning: "medium" };
  const modelCalls = [];
  context.fetch = async (path, options) => {
    if (path.startsWith("/api/watch/")) {
      const data = new URL(path, window.location).searchParams.get("data");
      const nonce = Uint8Array.from(atob(data.replace(/-/g, "+").replace(/_/g, "/")), c => c.charCodeAt(0)).slice(1, 13);
      const frame = {
        reset: true, chat: { id: "test", name: "Models", cwd: "/tmp", status: "idle" },
        ids: [], turnChanges: [], queued: [], olderRemaining: 0, messages: [],
      };
      const sealed = await seal.seal(link.key, seal.aad.frame(nonce, 0), JSON.stringify(frame));
      return new Response(new ReadableStream({
        start(controller) {
          controller.enqueue(new TextEncoder().encode(`data: ${JSON.stringify({ sealed: sealed.data })}\n\n`));
          options.signal.addEventListener("abort", () => controller.close(), { once: true });
        },
      }));
    }
    const { data } = JSON.parse(options.body);
    const request = JSON.parse(await seal.open(link.key, seal.aad.call(), data));
    const nonce = Uint8Array.from(atob(data.replace(/-/g, "+").replace(/_/g, "/")), c => c.charCodeAt(0)).slice(1, 13);
    if (request.tool === "set_chat_model") {
      modelCalls.push(request.args);
      current = {
        model: request.args.model || current.model,
        reasoning: request.args.reasoning || current.reasoning,
      };
    }
    const replies = {
      get_settings: {},
      list_models: { models, default: models[0].id, defaultReasoning: "medium" },
      composer_info: current,
      set_chat_model: current,
    };
    const sealed = await seal.seal(link.key, seal.aad.reply(nonce), JSON.stringify({ ok: replies[request.tool] }));
    return Response.json({ sealed: sealed.data });
  };
  try {
    runInContext(app, context);
    context.link = link;
    const chipReady = new Promise(resolve => {
      const observer = new window.MutationObserver(() => {
        if (document.querySelector(".model-chip .name")?.textContent === "Claude Sonnet 4.5") {
          observer.disconnect();
          resolve();
        }
      });
      observer.observe(document.body, { childList: true, subtree: true });
    });
    runInContext("links.set('mac', link); store.machines = [{id: 'mac', name: 'Mac', status: {online: true}}]; route()", context);
    await chipReady;

    const sheetReady = new Promise(resolve => {
      const observer = new window.MutationObserver(() => {
        if (document.querySelector("#sheet-root .item-row")) {
          observer.disconnect();
          resolve();
        }
      });
      observer.observe(document.body, { childList: true, subtree: true });
    });
    document.querySelector(".model-chip").click();
    await sheetReady;

    const rows = [...document.querySelectorAll("#sheet-root .item-row")];
    expect(rows.map(r => [r.querySelector(".label").firstChild.textContent, r.querySelector(".sub")?.textContent])).toEqual([
      ["Claude Sonnet 4.5", "anthropic"],
      ["GPT-5", "openai"],
      ["fast", "bifrost"],
    ]);

    const search = document.querySelector("#sheet-root .model-search");
    search.value = "openai";
    search.dispatchEvent(new window.Event("input"));
    expect([...document.querySelectorAll("#sheet-root .item-row .sub")].map(n => n.textContent)).toEqual(["openai"]);

    search.value = "no-such-model";
    search.dispatchEvent(new window.Event("input"));
    expect(document.querySelector("#sheet-root .item-row")).toBeNull();
    expect(document.querySelector("#sheet-root .note")?.textContent).toBe("No matching models");

    expect(document.querySelector("#sheet-root .pill")).toBeNull();
    const slider = document.querySelector('#sheet-root input[type="range"]');
    expect(slider.value).toBe("2");
    expect(document.querySelector("#sheet-root .slider-val").textContent).toBe("Medium");

    // Dragging previews without calling the engine until released.
    slider.value = "4";
    slider.dispatchEvent(new window.Event("input"));
    expect(document.querySelector("#sheet-root .slider-val").textContent).toBe("X-High");
    expect(modelCalls).toHaveLength(0);

    const saved = new Promise(resolve => {
      const observer = new window.MutationObserver(() => {
        if (document.querySelector(".model-chip .lvl")?.textContent === "X-High") {
          observer.disconnect();
          resolve();
        }
      });
      observer.observe(document.body, { childList: true, subtree: true });
    });
    slider.dispatchEvent(new window.Event("change"));
    await saved;
    expect(modelCalls).toEqual([{ chat: "test", reasoning: "xhigh" }]);
  } finally {
    runInContext("current?.destroy()", context);
    await window.happyDOM.close();
  }
});
