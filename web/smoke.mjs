// Smoke test of the WebAssembly module under Node:
//   node web/smoke.mjs rnse.wasm firmware.iso VARIANT [nav-disc.img] [seconds] [out.rgba]
// Boots the variant, runs it and reports what the screen shows.
import fs from "node:fs";
const [wasmPath, fwPath, variant, navPath, seconds = "60", out] = process.argv.slice(2);
let memory;
const files = new Map();
const env = {
  rnse_host_read(id, offset, ptr, len) {
    const fd = files.get(id);
    return fs.readSync(fd, new Uint8Array(memory.buffer, ptr, len), 0, len, offset);
  },
};
const { instance } = await WebAssembly.instantiate(fs.readFileSync(wasmPath), { env });
const x = instance.exports;
memory = x.memory;
const text = () => new TextDecoder().decode(new Uint8Array(memory.buffer, x.rnse_text(), x.rnse_text_len()));
const tell = (s) => { const b = new TextEncoder().encode(s); new Uint8Array(memory.buffer, x.rnse_text(), b.length).set(b); return b.length; };
const check = (r) => { if (r < 0) throw new Error(text()); return r; };

const fw = fs.readFileSync(fwPath);
const p = x.rnse_alloc(fw.length);
new Uint8Array(memory.buffer, p, fw.length).set(fw);
check(x.rnse_firmware(p, fw.length));
console.log("variants:\n" + text());
let settings = `variant=${variant}\n`;
if (navPath) {
  files.set(1, fs.openSync(navPath, "r"));
  settings += `nav=1\nnavlen=${fs.statSync(navPath).size}\n`;
}
check(x.rnse_boot(tell(settings)));
const t0 = Date.now();
let pressed = 0;
while (x.rnse_ms() < seconds * 1000) {
  if (x.rnse_run()) break;
  // NAV after 45 s, then the map soft key.
  if (navPath && pressed === 0 && x.rnse_ms() > 45000) { check(x.rnse_input(tell("key NAV 1"))); pressed = 1; }
  if (pressed === 1 && x.rnse_ms() > 45300) { check(x.rnse_input(tell("key NAV 0"))); pressed = 2; }
  if (pressed === 2 && x.rnse_ms() > 55000) { check(x.rnse_input(tell("key SOFT_BR 1"))); pressed = 3; }
  if (pressed === 3 && x.rnse_ms() > 55300) { check(x.rnse_input(tell("key SOFT_BR 0"))); pressed = 4; }
}
x.rnse_render();
const [w, h] = [x.rnse_width(), x.rnse_height()];
const px = new Uint8Array(memory.buffer, x.rnse_pixels(), w * h * 4);
let lit = 0, light = 0;
for (let i = 0; i < px.length; i += 4) {
  if (px[i] > 0x40 || px[i + 1] > 0x40 || px[i + 2] > 0x40) lit++;
  if (px[i] > 0xc0 && px[i + 1] > 0xc0 && px[i + 2] > 0xc0) light++;
}
x.rnse_status();
console.log(`${w}x${h}, ${lit} lit pixels, ${light} light; ${(x.rnse_ms() / 1000).toFixed(1)} s emulated in ${((Date.now() - t0) / 1000).toFixed(1)} s`);
console.log(text());
if (out) fs.writeFileSync(out, px);
