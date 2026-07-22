import { clsx, type ClassValue } from "clsx";
import { extendTailwindMerge } from "tailwind-merge";

// The design system defines custom font sizes (text-nav, text-button, …) in
// tailwind.config. tailwind-merge does not know them, so by default it mistakes
// e.g. `text-nav` for a text COLOR and drops a real color like `text-white` that
// sits alongside it — which made the dark button's label invisible. Register the
// custom sizes in the font-size group so size and color no longer collide.
const twMerge = extendTailwindMerge({
  extend: {
    classGroups: {
      "font-size": [
        {
          text: [
            "eyebrow",
            "caption",
            "nav",
            "button",
            "body",
            "subheading",
            "section",
            "hero",
          ],
        },
      ],
    },
  },
});

export function cn(...inputs: ClassValue[]): string {
  return twMerge(clsx(inputs));
}
