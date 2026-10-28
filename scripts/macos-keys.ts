/** AppKit key events for smokes that post native input through their app. */

export const commandFlag = 1 << 20;
export const controlFlag = 1 << 18;
export const optionFlag = 1 << 19;
export const shiftFlag = 1 << 17;

export type MacKeyEvent = {
  code: number;
  flags: number;
  text: string;
  plain: string;
};

const macKeyCodes: Record<string, number> = {
  a: 0,
  b: 11,
  c: 8,
  d: 2,
  e: 14,
  f: 3,
  g: 5,
  h: 4,
  i: 34,
  j: 38,
  k: 40,
  l: 37,
  m: 46,
  n: 45,
  o: 31,
  p: 35,
  q: 12,
  r: 15,
  s: 1,
  t: 17,
  u: 32,
  v: 9,
  w: 13,
  x: 7,
  y: 16,
  z: 6,
  " ": 49,
  "-": 27,
  ".": 47,
  "0": 29,
  "1": 18,
  "2": 19,
  "3": 20,
  "4": 21,
  "5": 23,
  "6": 22,
  "7": 26,
  "8": 28,
  "9": 25,
};

// Punctuation typed with Shift on a US layout, by the key's unshifted
// character.
const shiftedPunctuation: Record<string, string> = {
  "(": "9",
  ")": "0",
  "*": "8",
  _: "-",
};

export function macKeyEvents(text: string): MacKeyEvent[] {
  return [...text].map((character) => {
    const shifted = shiftedPunctuation[character];
    const plain = shifted ?? character.toLowerCase();
    const code = macKeyCodes[plain];
    if (code === undefined) {
      throw new Error(
        `unsupported macOS palette smoke character ${JSON.stringify(character)}`,
      );
    }
    return {
      code,
      flags: character === plain ? 0 : shiftFlag,
      text: character,
      plain,
    };
  });
}
