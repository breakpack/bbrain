// Builds a small, real PDF with a Helvetica text layer, so PDF.js has genuine
// text items (with transforms) to extract — a fixture, not a mock.
import { writeFileSync } from "node:fs";

const PAGES = [
  [
    ["/F2 18 Tf", "Attention Is All You Need"],
    ["/F1 11 Tf", "Ashish Vaswani, Noam Shazeer, Niki Parmar"],
    ["/F2 13 Tf", "Abstract"],
    ["/F1 11 Tf", "The dominant sequence transduction models are based on complex"],
    ["/F1 11 Tf", "recurrent or convolutional neural networks. We propose the Transformer,"],
    ["/F1 11 Tf", "a new simple network architecture based solely on attention mechanisms."],
    ["/F1 11 Tf", "Experiments on two machine translation tasks show these models to be"],
    ["/F1 11 Tf", "superior in quality while being more parallelizable."],
    ["/F2 13 Tf", "Introduction"],
    ["/F1 11 Tf", "Recurrent neural networks have been firmly established as state of the"],
    ["/F1 11 Tf", "art approaches in sequence modeling. This inherently sequential nature"],
    ["/F1 11 Tf", "precludes parallelization within training examples."],
  ],
  [
    ["/F2 13 Tf", "Results"],
    ["/F1 11 Tf", "On the WMT 2014 English-to-German translation task, the Transformer"],
    ["/F1 11 Tf", "achieves a BLEU score of 28.4, improving over the best previously"],
    ["/F1 11 Tf", "reported models by over 2 BLEU."],
    ["/F2 13 Tf", "Conclusion"],
    ["/F1 11 Tf", "We presented the Transformer, the first sequence transduction model"],
    ["/F1 11 Tf", "based entirely on attention. It can be trained significantly faster than"],
    ["/F1 11 Tf", "architectures based on recurrent or convolutional layers."],
  ],
];

const esc = (s) => s.replace(/([\\()])/g, "\\$1");

const streams = PAGES.map((lines) => {
  let y = 740;
  let out = "BT\n";
  for (const [font, text] of lines) {
    out += `${font}\n1 0 0 1 72 ${y} Tm\n(${esc(text)}) Tj\n`;
    y -= font.startsWith("/F2") ? 26 : 16;
  }
  out += "ET";
  return out;
});

const objects = [];
const add = (body) => objects.push(body) && objects.length;

const catalogId = 1;
const pagesId = 2;
objects.push(`<< /Type /Catalog /Pages ${pagesId} 0 R >>`); // 1
objects.push(""); // 2 — filled in below once page ids are known

const fontRegular = add("<< /Type /Font /Subtype /Type1 /BaseFont /Helvetica >>");
const fontBold = add("<< /Type /Font /Subtype /Type1 /BaseFont /Helvetica-Bold >>");

const pageIds = [];
for (const stream of streams) {
  const contentId = add(
    `<< /Length ${Buffer.byteLength(stream)} >>\nstream\n${stream}\nendstream`,
  );
  pageIds.push(
    add(
      `<< /Type /Page /Parent ${pagesId} 0 R /MediaBox [0 0 612 792] ` +
        `/Resources << /Font << /F1 ${fontRegular} 0 R /F2 ${fontBold} 0 R >> >> ` +
        `/Contents ${contentId} 0 R >>`,
    ),
  );
}

objects[pagesId - 1] =
  `<< /Type /Pages /Kids [${pageIds.map((id) => `${id} 0 R`).join(" ")}] /Count ${pageIds.length} >>`;

let pdf = "%PDF-1.7\n";
const offsets = [0];
for (let i = 0; i < objects.length; i++) {
  offsets.push(Buffer.byteLength(pdf));
  pdf += `${i + 1} 0 obj\n${objects[i]}\nendobj\n`;
}

const xrefStart = Buffer.byteLength(pdf);
pdf += `xref\n0 ${objects.length + 1}\n0000000000 65535 f \n`;
for (let i = 1; i <= objects.length; i++) {
  pdf += `${String(offsets[i]).padStart(10, "0")} 00000 n \n`;
}
pdf += `trailer\n<< /Size ${objects.length + 1} /Root ${catalogId} 0 R >>\nstartxref\n${xrefStart}\n%%EOF\n`;

writeFileSync(process.argv[2], pdf, "latin1");
console.log(`wrote ${process.argv[2]} (${Buffer.byteLength(pdf)} bytes, ${pageIds.length} pages)`);
