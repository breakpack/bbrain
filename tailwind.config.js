/** @type {import('tailwindcss').Config} */
// Token values are defined once in src/styles/tokens.css and referenced here,
// so Tailwind utilities and raw CSS never drift apart. See DESIGN.md.
export default {
  content: ["./index.html", "./src/**/*.{ts,tsx}"],
  theme: {
    extend: {
      colors: {
        primary: {
          DEFAULT: "var(--color-primary)",
          hover: "var(--color-primary-hover)",
          soft: "var(--color-primary-soft)",
        },
        canvas: {
          DEFAULT: "var(--color-canvas)",
          soft: "var(--color-canvas-soft)",
          tint: "var(--color-canvas-tint)",
        },
        ink: {
          DEFAULT: "var(--color-ink)",
          heading: "var(--color-heading)",
          deepest: "var(--color-ink-deepest)",
          body: "var(--color-body)",
          subhead: "var(--color-subhead)",
        },
        line: "var(--color-line)",
        danger: "var(--color-danger)",
        "on-primary": "var(--color-on-primary)",
      },
      fontFamily: {
        sans: "var(--font-sans)",
      },
      fontSize: {
        eyebrow: ["13px", { lineHeight: "1.54", fontWeight: "700" }],
        caption: ["14px", { lineHeight: "1.43" }],
        nav: ["14px", { lineHeight: "1.43" }],
        button: ["15px", { lineHeight: "1.33", fontWeight: "500" }],
        body: ["16px", { lineHeight: "1.5" }],
        subheading: ["24px", { lineHeight: "1.58", fontWeight: "700" }],
        section: ["42px", { lineHeight: "1.43", fontWeight: "700" }],
        hero: ["56px", { lineHeight: "1.43", fontWeight: "700" }],
      },
      spacing: {
        xs: "4px",
        sm: "7px",
        md: "14px",
        base: "16px",
        lg: "30px",
        xl: "36px",
        xxl: "48px",
        section: "64px",
      },
      borderRadius: {
        sm: "4px",
        control: "6px",
        card: "16px",
      },
      boxShadow: {
        card: "var(--shadow-card)",
      },
      transitionDuration: {
        fast: "120ms",
        standard: "200ms",
        slow: "320ms",
      },
      transitionTimingFunction: {
        standard: "cubic-bezier(0.25, 0.1, 0.25, 1)",
        enter: "cubic-bezier(0.2, 0.6, 0.25, 1)",
      },
    },
  },
  plugins: [],
};
