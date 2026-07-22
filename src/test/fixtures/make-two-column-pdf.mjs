// A single-page, two-column PDF with a real Helvetica text layer. Left and
// right columns share baselines, so it reproduces the interleaving bug.
import { writeFileSync } from "node:fs";

const LEFT = [
  "Left column first line here.",
  "Left column second line here.",
  "Left column third line here.",
  "Left column fourth line here.",
  "Left column fifth line here.",
  "Left column sixth line here.",
  "Left column seventh line here.",
  "Left column eighth line here.",
];
const RIGHT = [
  "Right column first line here.",
  "Right column second line here.",
  "Right column third line here.",
  "Right column fourth line here.",
  "Right column fifth line here.",
  "Right column sixth line here.",
  "Right column seventh line here.",
  "Right column eighth line here.",
];

const esc = (s) => s.replace(/([\\()])/g, "\\$1");

let stream = "BT\n/F1 11 Tf\n";
// Left column at x=72, right column at x=330 — same baselines.
let y = 720;
for (let i = 0; i < LEFT.length; i++) {
  stream += `1 0 0 1 72 ${y} Tm\n(${esc(LEFT[i])}) Tj\n`;
  stream += `1 0 0 1 330 ${y} Tm\n(${esc(RIGHT[i])}) Tj\n`;
  y -= 16;
}
stream += "ET";

const objects = [];
objects.push("<< /Type /Catalog /Pages 2 0 R >>");
objects.push(""); // pages, filled below
const fontId = objects.push("<< /Type /Font /Subtype /Type1 /BaseFont /Helvetica >>");
const contentId = objects.push(
  `<< /Length ${Buffer.byteLength(stream)} >>\nstream\n${stream}\nendstream`,
);
const pageId = objects.push(
  `<< /Type /Page /Parent 2 0 R /MediaBox [0 0 612 792] ` +
    `/Resources << /Font << /F1 ${fontId} 0 R >> >> /Contents ${contentId} 0 R >>`,
);
objects[1] = `<< /Type /Pages /Kids [${pageId} 0 R] /Count 1 >>`;

let pdf = "%PDF-1.7\n";
const offsets = [0];
for (let i = 0; i < objects.length; i++) {
  offsets.push(Buffer.byteLength(pdf));
  pdf += `${i + 1} 0 obj\n${objects[i]}\nendobj\n`;
}
const xref = Buffer.byteLength(pdf);
pdf += `xref\n0 ${objects.length + 1}\n0000000000 65535 f \n`;
for (let i = 1; i <= objects.length; i++) {
  pdf += `${String(offsets[i]).padStart(10, "0")} 00000 n \n`;
}
pdf += `trailer\n<< /Size ${objects.length + 1} /Root 1 0 R >>\nstartxref\n${xref}\n%%EOF\n`;

writeFileSync(process.argv[2], pdf, "latin1");
console.log(`wrote ${process.argv[2]}`);
