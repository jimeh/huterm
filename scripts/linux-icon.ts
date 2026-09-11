/** Derive and verify the committed 512-pixel Linux icon without Apple tools. */
import { createHash } from "node:crypto";
import { deflateSync, inflateSync } from "node:zlib";
import { mkdirSync, readFileSync, writeFileSync } from "node:fs";
import { dirname, join, resolve } from "node:path";

const repository = resolve(import.meta.dir, "..");
const source = "assets/Huterm.png";
const output = "assets/Huterm-512.png";
const manifestPath = "assets/linux/icon.json";
const signature = Buffer.from("89504e470d0a1a0a", "hex");

interface PngChunk { type: string; data: Buffer }

function hash(bytes: Uint8Array): string {
  return createHash("sha256").update(bytes).digest("hex");
}

function crc32(bytes: Uint8Array): number {
  let crc = 0xffffffff;
  for (const byte of bytes) {
    crc ^= byte;
    for (let bit = 0; bit < 8; bit++) crc = (crc >>> 1) ^ (0xedb88320 & -(crc & 1));
  }
  return (crc ^ 0xffffffff) >>> 0;
}

function chunks(png: Buffer): PngChunk[] {
  if (!png.subarray(0, 8).equals(signature)) throw new Error("Linux icon input is not a PNG");
  const result: PngChunk[] = [];
  let offset = 8;
  while (offset + 12 <= png.length) {
    const length = png.readUInt32BE(offset);
    const type = png.toString("ascii", offset + 4, offset + 8);
    const end = offset + 12 + length;
    if (end > png.length) throw new Error("Linux icon input has a truncated PNG chunk");
    const data = png.subarray(offset + 8, offset + 8 + length);
    const expectedCrc = png.readUInt32BE(offset + 8 + length);
    if (crc32(Buffer.concat([Buffer.from(type, "ascii"), data])) !== expectedCrc) throw new Error(`Linux icon input has an invalid ${type} checksum`);
    result.push({ type, data });
    offset = end;
    if (type === "IEND") break;
  }
  if (result.at(-1)?.type !== "IEND" || offset !== png.length) throw new Error("Linux icon input has an invalid PNG structure");
  return result;
}

export function pngDimensions(png: Buffer): { width: number; height: number } {
  const header = chunks(png).find(chunk => chunk.type === "IHDR")?.data;
  if (!header || header.length !== 13) throw new Error("Linux icon input is missing IHDR");
  return { width: header.readUInt32BE(0), height: header.readUInt32BE(4) };
}

function paeth(left: number, above: number, upperLeft: number): number {
  const prediction = left + above - upperLeft;
  const leftDistance = Math.abs(prediction - left);
  const aboveDistance = Math.abs(prediction - above);
  const upperLeftDistance = Math.abs(prediction - upperLeft);
  return leftDistance <= aboveDistance && leftDistance <= upperLeftDistance ? left : aboveDistance <= upperLeftDistance ? above : upperLeft;
}

function decodeRows(png: Buffer): { header: Buffer; rows: Buffer[] } {
  const parsed = chunks(png);
  const header = Buffer.from(parsed.find(chunk => chunk.type === "IHDR")?.data ?? []);
  if (header.length !== 13 || header.readUInt32BE(0) !== 1024 || header.readUInt32BE(4) !== 1024 || header[8] !== 16 || header[9] !== 6 || header[12] !== 0) {
    throw new Error("Linux icon source must be a non-interlaced 1024x1024 16-bit RGBA PNG");
  }
  const width = 1024;
  const bytesPerPixel = 8;
  const stride = width * bytesPerPixel;
  const raw = inflateSync(Buffer.concat(parsed.filter(chunk => chunk.type === "IDAT").map(chunk => chunk.data)));
  if (raw.length !== (stride + 1) * 1024) throw new Error("Linux icon source has unexpected decompressed size");
  const rows: Buffer[] = [];
  for (let y = 0; y < 1024; y++) {
    const filter = raw[y * (stride + 1)]!;
    const encoded = raw.subarray(y * (stride + 1) + 1, (y + 1) * (stride + 1));
    const row = Buffer.alloc(stride);
    const previous = rows[y - 1];
    for (let x = 0; x < stride; x++) {
      const value = encoded[x]!;
      const left = x >= bytesPerPixel ? row[x - bytesPerPixel]! : 0;
      const above = previous?.[x] ?? 0;
      const upperLeft = x >= bytesPerPixel ? previous?.[x - bytesPerPixel] ?? 0 : 0;
      const predictor = filter === 0 ? 0
        : filter === 1 ? left
        : filter === 2 ? above
        : filter === 3 ? Math.floor((left + above) / 2)
        : filter === 4 ? paeth(left, above, upperLeft)
        : undefined;
      if (predictor === undefined) throw new Error(`Linux icon source has unsupported PNG filter ${filter}`);
      row[x] = (value + predictor) & 0xff;
    }
    rows.push(row);
  }
  return { header, rows };
}

function chunk(type: string, data: Buffer): Buffer {
  const name = Buffer.from(type, "ascii");
  const result = Buffer.alloc(data.length + 12);
  result.writeUInt32BE(data.length, 0);
  name.copy(result, 4);
  data.copy(result, 8);
  result.writeUInt32BE(crc32(Buffer.concat([name, data])), data.length + 8);
  return result;
}

export function resizeLinuxIcon(sourcePng: Buffer): Buffer {
  const decoded = decodeRows(sourcePng);
  const stride = 512 * 8;
  const raw = Buffer.alloc((stride + 1) * 512);
  for (let y = 0; y < 512; y++) {
    const outputRow = raw.subarray(y * (stride + 1) + 1, (y + 1) * (stride + 1));
    for (let x = 0; x < 512; x++) {
      for (let channel = 0; channel < 4; channel++) {
        let total = 0;
        for (let dy = 0; dy < 2; dy++) for (let dx = 0; dx < 2; dx++) total += decoded.rows[y * 2 + dy]!.readUInt16BE((x * 2 + dx) * 8 + channel * 2);
        outputRow.writeUInt16BE(Math.round(total / 4), x * 8 + channel * 2);
      }
    }
  }
  const header = Buffer.from(decoded.header);
  header.writeUInt32BE(512, 0);
  header.writeUInt32BE(512, 4);
  return Buffer.concat([signature, chunk("IHDR", header), chunk("IDAT", deflateSync(raw, { level: 9 })), chunk("IEND", Buffer.alloc(0))]);
}

function inputHashes(root: string): Record<string, string> {
  return { [source]: hash(readFileSync(join(root, source))), "scripts/linux-icon.ts": hash(readFileSync(join(root, "scripts/linux-icon.ts"))) };
}

export function generateLinuxIcon(root = repository): void {
  const resized = resizeLinuxIcon(readFileSync(join(root, source)));
  mkdirSync(dirname(join(root, output)), { recursive: true });
  writeFileSync(join(root, output), resized);
  const manifest = { version: 1, algorithm: "rgba16-box-2x", inputs: inputHashes(root), output: { file: output, sha256: hash(resized) } };
  writeFileSync(join(root, manifestPath), `${JSON.stringify(manifest, null, 2)}\n`);
  checkLinuxIcon(root);
}

export function checkLinuxIcon(root = repository): void {
  const manifest = JSON.parse(readFileSync(join(root, manifestPath), "utf8"));
  if (manifest.version !== 1 || manifest.algorithm !== "rgba16-box-2x") throw new Error("unsupported Linux icon manifest");
  const actualInputs = inputHashes(root);
  if (JSON.stringify(actualInputs) !== JSON.stringify(manifest.inputs)) throw new Error("Linux icon input changed; run mise run icons:generate on macOS and include the generated Linux icon");
  const bytes = readFileSync(join(root, output));
  if (manifest.output?.file !== output || manifest.output?.sha256 !== hash(bytes)) throw new Error("Linux icon output changed; run mise run icons:generate");
  const dimensions = pngDimensions(bytes);
  if (dimensions.width !== 512 || dimensions.height !== 512) throw new Error("generated Linux icon must be 512x512");
}

if (import.meta.main) {
  try {
    const command = Bun.argv[2];
    if (command === "generate") { generateLinuxIcon(); console.log("Generated Huterm-512.png and Linux icon manifest"); }
    else if (command === "check") { checkLinuxIcon(); console.log("Verified 512-pixel Linux icon"); }
    else throw new Error("expected generate or check");
  } catch (error) {
    console.error(`Linux icon failed: ${error instanceof Error ? error.message : String(error)}`);
    process.exitCode = 1;
  }
}
