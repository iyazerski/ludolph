// Sealing what this phone and a Mac say, so the relay passes it on unread.
// The engine does the same in crates/engine/src/seal.rs; the test vectors there
// hold here too. Linking a phone gives it a secret in the QR code's URL
// fragment, which the relay never sees. Two HMAC-SHA256s of it give the phone's
// id, which the relay routes by, and an AES-256-GCM key, kept in IndexedDB as a
// key that can't be exported. A sealed message is
// `version || nonce || ciphertext || tag` in base64url, and its text starts
// with the time it was sealed. The associated data says what it is, so the relay
// can't pass one off as another. Shared by the app and its service worker.
"use strict";

const Seal = (() => {
  const VERSION = 1;
  const te = new TextEncoder();
  const td = new TextDecoder("utf-8", { fatal: true });
  const b64 = {
    decode: s => Uint8Array.from(atob(s.replace(/-/g, "+").replace(/_/g, "/")), c => c.charCodeAt(0)),
    encode: buf => {
      let s = "";
      // In chunks: spreading a message with photos would overflow the stack.
      for (let i = 0, b = new Uint8Array(buf); i < b.length; i += 0x8000) s += String.fromCharCode(...b.subarray(i, i + 0x8000));
      return btoa(s).replace(/\+/g, "-").replace(/\//g, "_").replace(/=+$/, "");
    },
  };
  const join = (...parts) => {
    const out = new Uint8Array(parts.reduce((n, p) => n + p.length, 0));
    let at = 0;
    for (const p of parts) { out.set(p, at); at += p.length; }
    return out;
  };

  /** The associated data of each kind of message, as `Purpose` in seal.rs. */
  const aad = {
    call: () => te.encode("ludolph/call"),
    watch: () => te.encode("ludolph/watch"),
    /** The answer to the call sealed with `nonce`. */
    reply: nonce => join(te.encode("ludolph/reply"), nonce),
    /** Frame `n`, from 0, of the watch sealed with `nonce`. */
    frame: (nonce, n) => {
      const count = new Uint8Array(8);
      new DataView(count.buffer).setBigUint64(0, BigInt(n));
      return join(te.encode("ludolph/frame"), nonce, count);
    },
    notice: () => te.encode("ludolph/notice"),
  };

  /** The phone id and sealing key that `secret` (base64url) gives. */
  async function derive(secret) {
    const bytes = b64.decode(secret);
    if (bytes.length !== 32) throw new Error("This link's code is damaged. Scan it again.");
    const hmac = await crypto.subtle.importKey("raw", bytes, { name: "HMAC", hash: "SHA-256" }, false, ["sign"]);
    const phone = b64.encode(await crypto.subtle.sign("HMAC", hmac, te.encode("ludolph/phone")));
    const raw = await crypto.subtle.sign("HMAC", hmac, te.encode("ludolph/key"));
    const key = await crypto.subtle.importKey("raw", raw, "AES-GCM", false, ["encrypt", "decrypt"]);
    return { phone, key };
  }

  /** Seal `text` with `key` for associated data `ad`; returns it and its nonce. */
  async function seal(key, ad, text) {
    const nonce = crypto.getRandomValues(new Uint8Array(12));
    const body = te.encode(text);
    const plain = new Uint8Array(8 + body.length);
    new DataView(plain.buffer).setBigUint64(0, BigInt(Date.now()));
    plain.set(body, 8);
    const sealed = await crypto.subtle.encrypt({ name: "AES-GCM", iv: nonce, additionalData: ad }, key, plain);
    return { data: b64.encode(join([VERSION], nonce, new Uint8Array(sealed))), nonce };
  }

  /** Open `data`, sealed with `key` for associated data `ad`, and return its text. */
  async function open(key, ad, data) {
    const bytes = b64.decode(data);
    if (bytes[0] !== VERSION) throw new Error("The Mac runs another Ludolph version. Update it.");
    let plain;
    try {
      plain = await crypto.subtle.decrypt({ name: "AES-GCM", iv: bytes.subarray(1, 13), additionalData: ad }, key, bytes.subarray(13));
    } catch {
      throw new Error("The Mac's answer doesn't open. Link this phone again from the Mac's Settings.");
    }
    return td.decode(new Uint8Array(plain).subarray(8));
  }

  // ---- this phone's links, one per Mac, in IndexedDB ----

  let db = null;
  function store(mode) {
    db ||= new Promise((resolve, reject) => {
      const req = indexedDB.open("ludolph", 1);
      req.onupgradeneeded = () => req.result.createObjectStore("links", { keyPath: "machine" });
      req.onsuccess = () => resolve(req.result);
      req.onerror = () => reject(req.error);
    });
    return db.then(d => d.transaction("links", mode).objectStore("links"));
  }
  const request = req => new Promise((resolve, reject) => {
    req.onsuccess = () => resolve(req.result);
    req.onerror = () => reject(req.error);
  });

  return {
    aad, derive, seal, open,
    /** Keep the link to `machine` that `secret` gives, in place of an older one. */
    async link(machine, secret) {
      const { phone, key } = await derive(secret);
      await request((await store("readwrite")).put({ machine, phone, key }));
    },
    /** Every link this phone has: `{ machine, phone, key }`. */
    async links() { return request((await store("readonly")).getAll()); },
    async forget(machine) { await request((await store("readwrite")).delete(machine)); },
  };
})();
