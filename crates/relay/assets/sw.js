// Service worker: push notifications, and the app shell for when the relay is away.
"use strict";

const SHELL = "zeron-shell-v4";
const FILES = ["/", "/app.css", "/app.js", "/hljs.js", "/file-icons.js", "/icon.svg", "/icon-192.png", "/badge.png"];

self.addEventListener("install", event => {
  event.waitUntil(caches.open(SHELL).then(cache => cache.addAll(FILES)));
  self.skipWaiting();
});

self.addEventListener("activate", event => {
  event.waitUntil((async () => {
    for (const key of await caches.keys()) if (key !== SHELL) await caches.delete(key);
    await self.clients.claim();
  })());
});

// Network first: the relay serves the newest app; the cache covers it being away.
self.addEventListener("fetch", event => {
  const url = new URL(event.request.url);
  if (event.request.method !== "GET" || url.origin !== location.origin || url.pathname === "/connect") return;
  event.respondWith((async () => {
    try {
      const response = await fetch(event.request);
      if (response.ok) {
        const copy = response.clone();
        caches.open(SHELL).then(cache => cache.put(event.request, copy));
      }
      return response;
    } catch (error) {
      return (await caches.match(event.request, { ignoreSearch: true })) || (await caches.match("/")) || Response.error();
    }
  })());
});

self.addEventListener("push", event => {
  const notice = event.data ? event.data.json() : { title: "Zeron", body: "" };
  event.waitUntil((async () => {
    const windows = await self.clients.matchAll({ type: "window", includeUncontrolled: true });
    const visible = windows.filter(w => w.visibilityState === "visible");
    // The app is on screen: it shows its own banner instead of a system one.
    if (visible.length && notice.kind !== "test") {
      for (const w of visible) w.postMessage({ type: "notice", notice });
      return;
    }
    await self.registration.showNotification(notice.title, {
      body: notice.body,
      tag: notice.chat || notice.kind,
      renotify: true,
      icon: "/icon-192.png",
      badge: "/badge.png",
      vibrate: [40, 30, 40],
      data: notice,
    });
  })());
});

self.addEventListener("notificationclick", event => {
  event.notification.close();
  const { machine, chat } = event.notification.data || {};
  const url = machine && chat ? `/#/m/${encodeURIComponent(machine)}/chat/${encodeURIComponent(chat)}` : "/";
  event.waitUntil((async () => {
    const windows = await self.clients.matchAll({ type: "window", includeUncontrolled: true });
    for (const w of windows) {
      await w.focus();
      w.postMessage({ type: "open", url });
      return;
    }
    await self.clients.openWindow(url);
  })());
});

// The push service rotated the subscription: hand the new one to the relay.
self.addEventListener("pushsubscriptionchange", event => {
  event.waitUntil((async () => {
    const post = (path, body) => fetch(path, {
      method: "POST",
      headers: { "Content-Type": "application/json" },
      body: JSON.stringify(body),
    });
    const { publicKey } = await (await post("/push/key", {})).json();
    const key = Uint8Array.from(atob(publicKey.replace(/-/g, "+").replace(/_/g, "/")), c => c.charCodeAt(0));
    const sub = await self.registration.pushManager.subscribe({ userVisibleOnly: true, applicationServerKey: key });
    await post("/push/subscribe", sub.toJSON());
  })());
});
