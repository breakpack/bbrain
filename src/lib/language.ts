/**
 * Whether a page of text should be translated into `targetLanguage` — i.e., the
 * text is predominantly in a different script than the target. Used to decide
 * whether to auto-translate a foreign paper's first page.
 *
 * Script ratio, not language ID: enough to tell "this Korean reader is looking
 * at an English (or other non-Korean) paper" without a language model.
 */
export function needsTranslation(text: string, targetLanguage: string): boolean {
  const letters = Array.from(text).filter((char) => /\p{L}/u.test(char));
  if (letters.length < 20) return false; // too little text to judge

  const ratio = (test: RegExp) =>
    letters.filter((char) => test.test(char)).length / letters.length;

  const hangul = ratio(/[가-힯ᄀ-ᇿ㄰-㆏]/);
  const latin = ratio(/[A-Za-z]/);

  // The reader's target script; if the paper is mostly some *other* script, it
  // needs translating.
  if (targetLanguage.startsWith("ko")) return hangul < 0.3;
  if (targetLanguage.startsWith("en")) return latin < 0.3;

  // Unknown target: only auto-translate when the page is clearly not Latin and
  // not Korean (e.g., a CJK paper), which is the safe, low-surprise default.
  return hangul < 0.3 && latin < 0.3;
}
