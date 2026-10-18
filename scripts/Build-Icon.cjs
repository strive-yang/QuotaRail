// Optional asset tooling: Node + sharp. Rust builds use the committed ICO.
const fs = require('node:fs');
const path = require('node:path');
const sharp = require('sharp');
const root = path.resolve(__dirname, '..', 'assets');
const source = fs.readFileSync(path.join(root, 'quotarail.svg'));

(async () => {
  await sharp(source).png().toFile(path.join(root, 'quotarail.png'));
  const sizes = [16, 20, 24, 32, 40, 48, 64];
  const images = [];
  for (const size of sizes) {
    const rgba = await sharp(source).resize(size, size).ensureAlpha().raw().toBuffer();
    const maskStride = Math.ceil(size / 32) * 4;
    const dib = Buffer.alloc(40 + size * size * 4 + maskStride * size);
    dib.writeUInt32LE(40, 0);
    dib.writeInt32LE(size, 4);
    dib.writeInt32LE(size * 2, 8); // ICO stores color + mask height.
    dib.writeUInt16LE(1, 12);
    dib.writeUInt16LE(32, 14);
    dib.writeUInt32LE(dib.length - 40, 20);
    for (let y = 0; y < size; y++) {
      for (let x = 0; x < size; x++) {
        const src = (y * size + x) * 4;
        const dst = 40 + ((size - y - 1) * size + x) * 4;
        dib[dst] = rgba[src + 2];
        dib[dst + 1] = rgba[src + 1];
        dib[dst + 2] = rgba[src];
        dib[dst + 3] = rgba[src + 3];
      }
    }
    images.push(dib);
  }
  const directory = Buffer.alloc(6 + sizes.length * 16);
  directory.writeUInt16LE(1, 2);
  directory.writeUInt16LE(sizes.length, 4);
  let offset = directory.length;
  sizes.forEach((size, i) => {
    const at = 6 + i * 16;
    directory[at] = size;
    directory[at + 1] = size;
    directory.writeUInt16LE(1, at + 4);
    directory.writeUInt16LE(32, at + 6);
    directory.writeUInt32LE(images[i].length, at + 8);
    directory.writeUInt32LE(offset, at + 12);
    offset += images[i].length;
  });
  fs.writeFileSync(path.join(root, 'quotarail.ico'), Buffer.concat([directory, ...images]));
  console.log('Generated PNG preview and multi-size ICO from the original SVG.');
})().catch(error => { console.error(error.message); process.exitCode = 1; });
