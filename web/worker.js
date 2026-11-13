// The emulator proper: the WebAssembly module, run in a worker so that it
// can read the user's disc images synchronously and keep the page free.
// Nothing here talks to a server; the files stay on the user's machine.
"use strict";

let x = null, memory = null, running = false;
const files = new Map();          // number -> File, for rnse_host_read
const reader = new FileReaderSync();
const encoder = new TextEncoder(), decoder = new TextDecoder();

const env = {
  // Read part of a file the user picked (the navigation disc) into the
  // module's memory.
  rnse_host_read(id, offset, ptr, len) {
    const file = files.get(id);
    if (!file || offset >= file.size) return 0;
    const part = new Uint8Array(reader.readAsArrayBuffer(file.slice(offset, offset + len)));
    new Uint8Array(memory.buffer, ptr, part.length).set(part);
    return part.length;
  },
};

const text = () => decoder.decode(new Uint8Array(memory.buffer, x.rnse_text(), x.rnse_text_len()));
const tell = (s) => {
  const bytes = encoder.encode(s);
  new Uint8Array(memory.buffer, x.rnse_text(), bytes.length).set(bytes);
  return bytes.length;
};
const check = (r) => { if (r < 0) throw new Error(text()); return r; };

async function load() {
  if (x) return;
  const response = await fetch("rnse.wasm");
  const { instance } = await WebAssembly.instantiateStreaming(response, { env })
    .catch(async () => WebAssembly.instantiate(await (await fetch("rnse.wasm")).arrayBuffer(), { env }));
  x = instance.exports;
  memory = x.memory;
}

// Emulated time is held to wall-clock time; what cannot be kept up with
// is given up rather than caught up later.
let wallStart = 0, emuStart = 0, speedMark = 0, speedEmu = 0, speed = 0, lastStatus = 0;

function tick() {
  if (!running) return;
  const now = performance.now();
  let stopped = false;
  try {
    const budget = now + 30;
    while (x.rnse_ms() - emuStart < performance.now() - wallStart && performance.now() < budget) {
      if (x.rnse_run()) { stopped = true; break; }
    }
    const behind = (performance.now() - wallStart) - (x.rnse_ms() - emuStart);
    if (behind > 500) wallStart += behind;
    if (x.rnse_render()) {
      const [w, h] = [x.rnse_width(), x.rnse_height()];
      const pixels = new Uint8ClampedArray(memory.buffer, x.rnse_pixels(), w * h * 4).slice();
      postMessage({ type: "frame", w, h, pixels }, [pixels.buffer]);
    }
    const n = x.rnse_log();
    if (n > 0) postMessage({ type: "log", bytes: new Uint8Array(memory.buffer, x.rnse_text(), n).slice() });
    if (now - lastStatus > 400 || stopped) {
      if (now - speedMark >= 1000) {
        speed = 100 * (x.rnse_ms() - speedEmu) / (now - speedMark);
        speedMark = now; speedEmu = x.rnse_ms();
      }
      lastStatus = now;
      x.rnse_status();
      postMessage({ type: "status", status: JSON.parse(text()), speed });
    }
  } catch (e) {
    running = false;
    postMessage({ type: "error", message: "the emulator stopped: " + e.message });
    return;
  }
  if (stopped) {
    // Keep answering, slowly; a restart begins again.
    wallStart = performance.now(); emuStart = x.rnse_ms();
    setTimeout(tick, 200);
  } else {
    setTimeout(tick, 0);
  }
}

function resync() {
  wallStart = performance.now(); emuStart = x.rnse_ms();
  speedMark = wallStart; speedEmu = emuStart;
}

onmessage = async (e) => {
  const m = e.data;
  try {
    await load();
    if (m.type === "firmware") {
      // The update disc image, whole: the module parses it and keeps it.
      if (m.file.size > 1 << 30) throw new Error("that file is too large for an update disc; a navigation disc goes in the other field");
      const bytes = new Uint8Array(reader.readAsArrayBuffer(m.file));
      const p = x.rnse_alloc(bytes.length);
      new Uint8Array(memory.buffer, p, bytes.length).set(bytes);
      check(x.rnse_firmware(p, bytes.length));
      const variants = text().trim().split("\n").map((l) => { const [name, version, model] = l.split(" "); return { name, version, model }; });
      postMessage({ type: "variants", variants });
    } else if (m.type === "boot") {
      let settings = "";
      for (const [k, v] of Object.entries(m.settings)) settings += `${k}=${v}\n`;
      if (m.nav) {
        files.set(1, m.nav);
        settings += `nav=1\nnavlen=${m.nav.size}\n`;
      }
      check(x.rnse_boot(tell(settings)));
      resync();
      postMessage({ type: "started" });
      if (!running) { running = true; tick(); }
    } else if (m.type === "input") {
      check(x.rnse_input(tell(m.text)));
      if (m.text === "reset") resync();
    }
  } catch (err) {
    postMessage({ type: "error", message: err.message });
  }
};
