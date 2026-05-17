/** @type {import('tailwindcss').Config} */
export default {
  content: ["./index.html", "./src/**/*.{ts,tsx}"],
  theme: {
    extend: {
      colors: {
        surface: "#1a1a1a",
        panel: "#242424",
        border: "#333",
        accent: "#ff8c00",
      },
    },
  },
  plugins: [],
};
